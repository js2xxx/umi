use alloc::{boxed::Box, sync::Arc};
use core::{ops::Range, ptr};

use ksc_core::Error::{self, EEXIST, EINVAL, ENOSPC};
use range_map::{AslrKey, RangeMap};
use riscv::register::satp::{self, Mode::Sv39};
use rv39_paging::{Attr, LAddr, Table, ID_OFFSET, PAGE_LAYOUT, PAGE_MASK, PAGE_SHIFT};
use spin::lock_api::Mutex;

use crate::{frame::frames, Phys};

struct Mapping {
    phys: Arc<Phys>,
    start_index: usize,
    attr: Attr,
}

pub struct Virt {
    root: Mutex<Box<Table>>,
    map: Mutex<RangeMap<LAddr, Mapping>>,
}

impl Mapping {
    async fn commit(
        &mut self,
        addr: LAddr,
        offset: usize,
        count: usize,
        table: &mut Table,
    ) -> Result<(), Error> {
        let writable = self.attr.contains(Attr::WRITABLE);
        for (index, addr) in
            (0..count).map(|c| (c + self.start_index + offset, addr + (c << PAGE_SHIFT)))
        {
            let entry = table.la2pte_alloc(addr, frames(), ID_OFFSET)?;
            if !entry.is_set() {
                let frame = self.phys.commit(index, writable).await?;
                *entry = rv39_paging::Entry::new(
                    frame.base(),
                    self.attr | Attr::VALID,
                    rv39_paging::Level::pt(),
                );
            }
        }
        Ok(())
    }

    async fn decommit(
        &mut self,
        addr: LAddr,
        offset: usize,
        count: usize,
        table: &mut Table,
    ) -> Result<(), Error> {
        for (index, addr) in
            (0..count).map(|c| (c + self.start_index + offset, addr + (c << PAGE_SHIFT)))
        {
            if let Ok(entry) = table.la2pte(addr, ID_OFFSET) {
                let dirty = entry.get(rv39_paging::Level::pt()).1.contains(Attr::DIRTY);
                self.phys.release(index, dirty).await?;
                entry.reset();
            }
        }
        Ok(())
    }
}

impl Virt {
    pub fn new(range: Range<LAddr>, init_root: Box<Table>) -> Self {
        Virt {
            root: Mutex::new(init_root),
            map: Mutex::new(RangeMap::new(range)),
        }
    }

    /// # Safety
    ///
    /// The caller must ensure that the current executing address is mapped
    /// correctly, and the virt object outlives its existence in `satp`.
    pub unsafe fn load(&self, asid: usize) {
        let addr = unsafe { ptr::addr_of_mut!(**self.root.data_ptr()) };
        let paddr = *LAddr::from(addr).to_paddr(ID_OFFSET);
        satp::set(Sv39, asid, paddr >> PAGE_SHIFT);
    }

    pub fn map(
        &self,
        addr: Option<LAddr>,
        phys: Arc<Phys>,
        start_index: usize,
        count: usize,
        attr: Attr,
    ) -> Result<LAddr, Error> {
        const ASLR_BIT: u32 = 30;

        ksync::critical(|| {
            let mut map = self.map.lock();
            match addr {
                Some(start) => {
                    if start.val() & PAGE_MASK != 0 {
                        return Err(EINVAL);
                    }
                    let len = count.checked_shl(PAGE_SHIFT.try_into()?).ok_or(EINVAL)?;
                    let end = LAddr::from(start.val().checked_add(len).ok_or(EINVAL)?);
                    let mapping = Mapping {
                        phys,
                        start_index,
                        attr,
                    };
                    map.try_insert(start..end, mapping).map_err(|_| EEXIST)?;
                    Ok(start)
                }
                None => {
                    let layout = PAGE_LAYOUT.repeat(count)?.0;
                    let aslr_key = AslrKey::new(ASLR_BIT, rand_riscv::rng(), layout);

                    let ent = map.allocate_with_aslr(aslr_key, LAddr::val).ok_or(ENOSPC)?;
                    let addr = *ent.key().start;
                    ent.insert(Mapping {
                        phys,
                        start_index,
                        attr,
                    });
                    Ok(addr)
                }
            }
        })
    }

    pub async fn commit(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        ksync::critical(|| async {
            let mut map = self.map.lock();
            let mut table = self.root.lock();

            for (addr, mapping) in map.intersection_mut(range.clone()) {
                let start = range.start.max(*addr.start);
                let end = range.end.min(*addr.end);
                let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
                let count = (end.val() - start.val()) >> PAGE_SHIFT;

                mapping.commit(start, offset, count, &mut table).await?;
            }
            Ok(())
        })
        .await
    }

    pub async fn decommit(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        ksync::critical(|| async {
            let mut map = self.map.lock();
            let mut table = self.root.lock();

            for (addr, mapping) in map.intersection_mut(range.clone()) {
                let start = range.start.max(*addr.start);
                let end = range.end.min(*addr.end);
                let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
                let count = (end.val() - start.val()) >> PAGE_SHIFT;

                mapping.decommit(start, offset, count, &mut table).await?;
            }
            Ok(())
        })
        .await
    }

    pub async fn reprotect(&self, range: Range<LAddr>, attr: Attr) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let attr = attr | Attr::VALID;
        ksync::critical(|| async {
            let mut map = self.map.lock();
            let mut table = self.root.lock();

            for (addr, mapping) in map.range_mut(range.clone()) {
                let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;

                mapping.decommit(*addr.start, 0, count, &mut table).await?;
                mapping.attr = attr;
            }

            if let Some((mut mapping, mut entry)) = map.split_entry(range.start) {
                let addr = entry.old_key();
                let offset = (range.start.val() - addr.start.val()) >> PAGE_SHIFT;
                let count = (addr.end.val() - range.start.val()) >> PAGE_SHIFT;

                mapping
                    .decommit(range.start, offset, count, &mut table)
                    .await?;

                let latter = Mapping {
                    phys: mapping.phys.clone(),
                    start_index: mapping.start_index + offset,
                    attr,
                };

                entry.set_former(mapping);
                entry.set_latter(latter);
            }
            if let Some((mut mapping, mut entry)) = map.split_entry(range.end) {
                let addr = entry.old_key();
                let count = (range.end.val() - addr.start.val()) >> PAGE_SHIFT;

                mapping.decommit(range.end, 0, count, &mut table).await?;

                let former = Mapping {
                    phys: mapping.phys.clone(),
                    start_index: mapping.start_index,
                    attr,
                };

                mapping.start_index += count;
                entry.set_former(former);
                entry.set_latter(mapping);
            }
            Ok(())
        })
        .await
    }

    pub async fn unmap(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        ksync::critical(|| async {
            let mut map = self.map.lock();
            let mut table = self.root.lock();

            for (addr, mut mapping) in map.drain(range.clone()) {
                let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;

                mapping.decommit(addr.start, 0, count, &mut table).await?;
            }

            if let Some((mut mapping, mut entry)) = map.split_entry(range.start) {
                let addr = entry.old_key();
                let offset = (range.start.val() - addr.start.val()) >> PAGE_SHIFT;
                let count = (addr.end.val() - range.start.val()) >> PAGE_SHIFT;
                mapping
                    .decommit(range.start, offset, count, &mut table)
                    .await?;
                entry.set_former(mapping);
            }
            if let Some((mut mapping, mut entry)) = map.split_entry(range.end) {
                let addr = entry.old_key();
                let count = (range.end.val() - addr.start.val()) >> PAGE_SHIFT;
                mapping.decommit(range.end, 0, count, &mut table).await?;
                mapping.start_index += count;
                entry.set_latter(mapping);
            }
            Ok(())
        })
        .await
    }
}

impl Drop for Virt {
    fn drop(&mut self) {
        let range = self.map.get_mut().root_range();
        let count = (range.end.val() - range.start.val()) >> PAGE_SHIFT;
        let _ = self
            .root
            .get_mut()
            .user_unmap_npages(*range.start, count, frames(), ID_OFFSET);
    }
}
