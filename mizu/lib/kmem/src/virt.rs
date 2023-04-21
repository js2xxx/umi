use alloc::sync::Arc;
use core::ops::Range;

use ksc_core::Error::{self, EEXIST, ENOSPC};
use range_map::{AslrKey, RangeMap};
use rv39_paging::{Attr, LAddr, Table, ID_OFFSET, PAGE_LAYOUT, PAGE_SHIFT};
use spin::Mutex;

use crate::{frame::frames, Phys};

struct Mapping {
    phys: Arc<Phys>,
    start_index: usize,
    attr: Attr,
}

pub struct Virt {
    root: Mutex<Table>,
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
    pub fn new(range: Range<LAddr>) -> Self {
        Virt {
            root: Mutex::new(Default::default()),
            map: Mutex::new(RangeMap::new(range)),
        }
    }

    pub fn map(
        &self,
        addr: Option<LAddr>,
        phys: Arc<Phys>,
        start_index: usize,
        len: usize,
        attr: Attr,
    ) -> Result<LAddr, Error> {
        const ASLR_BIT: u32 = 30;
        let aslr_key = AslrKey::new(ASLR_BIT, rand_riscv::rng(), PAGE_LAYOUT);

        ksync::critical(|| {
            let mut map = self.map.lock();
            match addr {
                Some(addr) => {
                    let range = addr..(addr + len);
                    let mapping = Mapping {
                        phys,
                        start_index,
                        attr,
                    };
                    map.try_insert(range, mapping).map_err(|_| EEXIST)?;
                    Ok(addr)
                }
                None => {
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
