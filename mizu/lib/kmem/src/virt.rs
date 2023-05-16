mod tlb;

use alloc::{sync::Arc, vec::Vec};
use core::{
    marker::PhantomPinned,
    mem,
    num::NonZeroUsize,
    ops::Range,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use futures_util::Future;
use ksc_core::Error::{self, EFAULT, EINVAL, ENOSPC, EPERM};
use ksync::Mutex;
use range_map::{AslrKey, RangeMap};
use rv39_paging::{
    Attr, LAddr, PAddr, Table, ID_OFFSET, PAGE_LAYOUT, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE,
};

use crate::{frame::frames, Phys};

const ASLR_BIT: u32 = 30;

struct Mapping {
    phys: Arc<Phys>,
    start_index: usize,
    attr: Attr,
}

pub struct Virt {
    root: Mutex<Table>,
    map: Mutex<RangeMap<LAddr, Mapping>>,
    cpu_mask: AtomicUsize,

    _marker: PhantomPinned,
}

unsafe impl Send for Virt {}
unsafe impl Sync for Virt {}

struct TlbFlushOnDrop {
    cpu_mask: usize,
    addr: LAddr,
    count: usize,
}

impl TlbFlushOnDrop {
    fn new(cpu_mask: usize, addr: LAddr) -> Self {
        TlbFlushOnDrop {
            cpu_mask,
            addr,
            count: 0,
        }
    }
}

impl Drop for TlbFlushOnDrop {
    fn drop(&mut self) {
        tlb::flush(self.cpu_mask, self.addr, self.count)
    }
}

impl Mapping {
    async fn commit(
        &mut self,
        addr: LAddr,
        offset: usize,
        count: NonZeroUsize,
        table: &mut Table,
        cpu_mask: usize,
    ) -> Result<Vec<Range<PAddr>>, Error> {
        let writable = self.attr.contains(Attr::WRITABLE);
        let mut p = Vec::new();

        let mut flush = TlbFlushOnDrop::new(cpu_mask, addr);

        for (index, addr) in
            (0..count.get()).map(|c| (c + self.start_index + offset, addr + (c << PAGE_SHIFT)))
        {
            let entry = table.la2pte_alloc(addr, frames(), ID_OFFSET)?;
            let base = if !entry.is_set() {
                let writable = writable.then_some(PAGE_SIZE);
                let (frame, _) = self.phys.commit(index, writable, true).await?;
                let base = frame.base();
                *entry = rv39_paging::Entry::new(base, self.attr, rv39_paging::Level::pt());
                flush.count += 1;
                base
            } else {
                entry.addr(rv39_paging::Level::pt())
            };
            p.push(base..base + PAGE_SIZE);
        }
        Ok(p)
    }

    async fn decommit(
        &mut self,
        addr: LAddr,
        offset: usize,
        count: NonZeroUsize,
        table: &mut Table,
        cpu_mask: usize,
    ) -> Result<(), Error> {
        let mut flush = TlbFlushOnDrop::new(cpu_mask, addr);

        for (index, addr) in
            (0..count.get()).map(|c| (c + self.start_index + offset, addr + (c << PAGE_SHIFT)))
        {
            if let Ok(entry) = table.la2pte(addr, ID_OFFSET) {
                let dirty = entry.get(rv39_paging::Level::pt()).1.contains(Attr::DIRTY);
                self.phys.flush(index, Some(dirty), true).await?;
                entry.reset();
                flush.count += 1;
            } else {
                flush = TlbFlushOnDrop::new(cpu_mask, addr + PAGE_SIZE);
            }
        }
        Ok(())
    }

    async fn deep_fork(&mut self) -> Mapping {
        Mapping {
            phys: Arc::new(self.phys.clone_as(self.phys.is_cow()).await),
            start_index: self.start_index,
            attr: self.attr,
        }
    }
}

impl Virt {
    pub fn new(range: Range<LAddr>, init_root: Table) -> Pin<Arsc<Self>> {
        Arsc::pin(Virt {
            root: Mutex::new(init_root),
            map: Mutex::new(RangeMap::new(range)),
            cpu_mask: AtomicUsize::new(0),
            _marker: PhantomPinned,
        })
    }

    /// # Safety
    ///
    /// The caller must ensure that the current executing address is mapped
    /// correctly.
    #[inline]
    pub unsafe fn load(self: Pin<Arsc<Self>>) -> Option<impl Future<Output = ()> + Send + 'static> {
        tlb::set_virt(self)
    }

    pub async fn map(
        &self,
        addr: Option<LAddr>,
        phys: Arc<Phys>,
        start_index: usize,
        count: usize,
        attr: Attr,
    ) -> Result<LAddr, Error> {
        log::trace!(
            "Virt::map at {addr:?}, start_index = {start_index}, count = {count}, attr = {attr:?}"
        );

        let mut map = self.map.lock().await;
        match addr {
            Some(start) => {
                if start.val() & PAGE_MASK != 0 {
                    return Err(EINVAL);
                }
                let len = count
                    .checked_shl(PAGE_SHIFT)
                    .filter(|&l| l != 0)
                    .ok_or(EINVAL)?;
                let end = LAddr::from(start.val().checked_add(len).ok_or(EINVAL)?);
                let mapping = Mapping {
                    phys,
                    start_index,
                    attr: attr | Attr::VALID,
                };
                map.try_insert(start..end, mapping).map_err(|_| ENOSPC)?;
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
                    attr: attr | Attr::VALID,
                });
                Ok(addr)
            }
        }
    }

    pub async fn find_free(
        &self,
        start: Option<LAddr>,
        count: usize,
    ) -> Result<Range<LAddr>, Error> {
        let layout = PAGE_LAYOUT.repeat(count)?.0;
        let aslr_key = AslrKey::new(ASLR_BIT, rand_riscv::rng(), layout);

        let map = self.map.lock().await;
        match start {
            None => map.find_free_with_aslr(aslr_key, LAddr::val),
            Some(start) => {
                let range = start..(start + (count << PAGE_SHIFT));
                (!map.intersects(range.clone())).then_some(range)
            }
        }
        .ok_or(ENOSPC)
    }

    pub async fn commit_range(&self, range: Range<LAddr>) -> Result<Vec<Range<PAddr>>, Error> {
        let aligned_range = LAddr::from(range.start.val() & !PAGE_MASK)
            ..LAddr::from((range.end.val() + PAGE_MASK) & !PAGE_MASK);

        log::trace!("Virt::commit_range {range:?} => {aligned_range:?}");

        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        let mut paddr = Vec::new();
        for (addr, mapping) in map.intersection_mut(aligned_range.clone()) {
            log::trace!("Virt::commit found {addr:?}");
            let start = aligned_range.start.max(*addr.start);
            let end = aligned_range.end.min(*addr.end);
            let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
            let len = end.val() - start.val();
            let count = len >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                let mut p = mapping
                    .commit(start, offset, count, &mut table, cpu_mask)
                    .await?;
                if let Some(first) = p.first_mut() {
                    first.start += range.start.val().saturating_sub(start.val())
                }
                if let Some(last) = p.last_mut() {
                    last.end -= end.val().saturating_sub(range.end.val())
                }
                paddr.extend(p.into_iter().rev())
            }
        }
        paddr.reverse();
        log::trace!("Virt::commit_range result: {paddr:?}");
        Ok(paddr)
    }

    pub async fn commit(&self, addr: LAddr) -> Result<PAddr, Error> {
        let paddr = self.commit_range(addr..(addr + 1)).await?;
        paddr.first().cloned().ok_or(EFAULT).map(|r| r.start)
    }

    pub async fn decommit_range(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        for (addr, mapping) in map.intersection_mut(range.clone()) {
            let start = range.start.max(*addr.start);
            let end = range.end.min(*addr.end);
            let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
            let count = (end.val() - start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(start, offset, count, &mut table, cpu_mask)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn reprotect(&self, range: Range<LAddr>, attr: Attr) -> Result<(), Error> {
        log::trace!("Virt::reprotect {range:?}");

        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let attr = attr | Attr::VALID;

        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        for (addr, mapping) in map.range_mut(range.clone()) {
            if !mapping.attr.contains(attr) {
                return Err(EPERM);
            }
            let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(*addr.start, 0, count, &mut table, cpu_mask)
                    .await?;
            }
            mapping.attr = attr;
        }

        if let Some((mut mapping, mut entry)) = map.split_entry(range.start) {
            if !mapping.attr.contains(attr) {
                return Err(EPERM);
            }
            let addr = entry.old_key();
            let offset = (range.start.val() - addr.start.val()) >> PAGE_SHIFT;
            let count = (addr.end.val() - range.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.start, offset, count, &mut table, cpu_mask)
                    .await?;
            }

            let latter = Mapping {
                phys: mapping.phys.clone(),
                start_index: mapping.start_index + offset,
                attr,
            };

            entry.set_former(mapping);
            entry.set_latter(latter);
        }
        if let Some((mut mapping, mut entry)) = map.split_entry(range.end) {
            if !mapping.attr.contains(attr) {
                return Err(EPERM);
            }
            let addr = entry.old_key();
            let count = (range.end.val() - addr.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.end, 0, count, &mut table, cpu_mask)
                    .await?;
            }

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
    }

    pub async fn unmap(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        for (addr, mut mapping) in map.drain(range.clone()) {
            let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;
            if let Some(count) = NonZeroUsize::new(count) {
                mapping
                    .decommit(addr.start, 0, count, &mut table, self.cpu_mask.load(SeqCst))
                    .await?;
            }
        }

        if let Some((mut mapping, mut entry)) = map.split_entry(range.start) {
            let addr = entry.old_key();
            let offset = (range.start.val() - addr.start.val()) >> PAGE_SHIFT;
            let count = (addr.end.val() - range.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.start, offset, count, &mut table, cpu_mask)
                    .await?;
            }
            entry.set_former(mapping);
        }
        if let Some((mut mapping, mut entry)) = map.split_entry(range.end) {
            let addr = entry.old_key();
            let count = (range.end.val() - addr.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.end, 0, count, &mut table, cpu_mask)
                    .await?;
            }
            mapping.start_index += count;
            entry.set_latter(mapping);
        }
        Ok(())
    }

    pub async fn clear(&self) {
        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        let range = map.root_range();
        let range = *range.start..*range.end;
        let old = mem::replace(&mut *map, RangeMap::new(range.clone()));

        let count = (range.end.val() - range.start.val()) >> PAGE_SHIFT;
        let _ = table.user_unmap_npages(range.start, count, frames(), ID_OFFSET);
        tlb::flush(self.cpu_mask.load(SeqCst), range.start, count);

        for (addr, mapping) in old {
            let count: usize = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;
            for index in 0..count {
                let dirty = mapping.attr.contains(Attr::WRITABLE);
                let _ = mapping.phys.flush(index, Some(dirty), true).await;
            }
        }
    }

    pub async fn deep_fork(self: Pin<&Self>, init_root: Table) -> Result<Pin<Arsc<Virt>>, Error> {
        let mut map = self.map.lock().await;
        let mut table = self.root.lock().await;

        let range = map.root_range();
        let mut new_map = RangeMap::new(*range.start..*range.end);

        for (addr, mapping) in map.iter_mut() {
            log::trace!("Virt::deep_fork: cloning mapping {addr:?}");
            if mapping.attr.contains(Attr::WRITABLE) {
                let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;
                if let Some(count) = NonZeroUsize::new(count) {
                    let cpu_mask = self.cpu_mask.load(SeqCst);
                    mapping
                        .decommit(*addr.start, 0, count, &mut table, cpu_mask)
                        .await?;
                }
            }
            let new_mapping = mapping.deep_fork().await;
            let _ = new_map.try_insert(*addr.start..*addr.end, new_mapping);
        }

        Ok(Arsc::pin(Virt {
            root: Mutex::new(init_root),
            map: Mutex::new(new_map),
            cpu_mask: AtomicUsize::new(0),
            _marker: PhantomPinned,
        }))
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
        tlb::flush(self.cpu_mask.load(SeqCst), *range.start, count);
    }
}
