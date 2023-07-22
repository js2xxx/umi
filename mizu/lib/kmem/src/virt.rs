mod tlb;

use alloc::vec::Vec;
use core::{
    marker::PhantomPinned,
    mem,
    num::NonZeroUsize,
    ops::{Deref, DerefMut, Range},
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use ksc_core::Error::{self, EFAULT, EINVAL, ENOSPC, EPERM};
use ksync::{Mutex, RwLock, RwLockUpgradableReadGuard, RwLockWriteGuard};
use range_map::{AslrKey, RangeMap};
use rv39_paging::{Attr, LAddr, Table, ID_OFFSET, PAGE_LAYOUT, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use static_assertions::const_assert_eq;

pub use self::tlb::unset_virt;
use crate::{frame::frames, Frame, Phys};

const ASLR_BIT: u32 = 30;

struct Mapping {
    phys: Arsc<Phys>,
    start_index: usize,
    attr: Attr,
}

#[derive(Debug)]
#[repr(C)]
struct SliceRepr {
    _ptr: *mut u8,
    _len: usize,
}
const_assert_eq!(mem::size_of::<SliceRepr>(), mem::size_of::<&'static [u8]>());
const_assert_eq!(
    mem::size_of::<SliceRepr>(),
    mem::size_of::<&'static mut [u8]>()
);

pub struct VirtCommitGuard<'a> {
    map: Option<RwLockUpgradableReadGuard<'a, RangeMap<LAddr, Mapping>>>,
    virt: &'a Virt,
    attr: Attr,
    range: Vec<SliceRepr>,
}
unsafe impl Send for VirtCommitGuard<'_> {}

impl<'a> VirtCommitGuard<'a> {
    pub async fn push(&mut self, range: Range<LAddr>) -> Result<(), Error> {
        struct Guard<'a, 'b>(
            &'b mut VirtCommitGuard<'a>,
            Option<RwLockWriteGuard<'a, RangeMap<LAddr, Mapping>>>,
        );
        impl Drop for Guard<'_, '_> {
            fn drop(&mut self) {
                self.0.map = Some(RwLockWriteGuard::downgrade_to_upgradable(
                    self.1.take().unwrap(),
                ))
            }
        }
        impl Deref for Guard<'_, '_> {
            type Target = RangeMap<LAddr, Mapping>;

            fn deref(&self) -> &Self::Target {
                self.1.as_deref().unwrap()
            }
        }
        impl DerefMut for Guard<'_, '_> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                self.1.as_deref_mut().unwrap()
            }
        }

        let aligned_range = LAddr::from(range.start.val() & !PAGE_MASK)
            ..LAddr::from((range.end.val() + PAGE_MASK) & !PAGE_MASK);
        log::trace!("Virt::commit_range {range:?} => {aligned_range:?}");

        let map = self.map.take().unwrap();
        let mut table = self.virt.root.lock().await;

        let is_committed =
            map.intersection(aligned_range.clone())
                .try_fold(true, |acc, (addr, mapping)| {
                    let start = aligned_range.start.max(*addr.start);
                    let end = aligned_range.end.min(*addr.end);
                    let len = end.val() - start.val();
                    let count = len >> PAGE_SHIFT;

                    let is_committed =
                        mapping.is_committed(start, count, table.as_table(), self.attr)?;
                    Ok::<_, Error>(acc && is_committed)
                })?;

        log::trace!(
            "Virt::commit_range: {}",
            if is_committed {
                "pages are all committed"
            } else {
                "has uncommitted pages"
            }
        );

        if is_committed {
            self.map = Some(map);
            self.range.push(SliceRepr {
                _ptr: *range.start,
                _len: range.end.val() - range.start.val(),
            });
            return Ok(());
        }

        let mut guard = Guard(self, Some(RwLockUpgradableReadGuard::upgrade(map).await));
        let map = guard.1.as_mut().unwrap();
        let this = &mut guard.0;

        for (addr, mapping) in map.intersection_mut(aligned_range.clone()) {
            log::trace!("Virt::commit_range found {addr:?}");
            let start = aligned_range.start.max(*addr.start);
            let end = aligned_range.end.min(*addr.end);
            let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
            let len = end.val() - start.val();
            let count = len >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = this.virt.cpu_mask.load(SeqCst);
                mapping
                    .commit(start, offset, count, table.as_table(), cpu_mask, this.attr)
                    .await?;
            }
        }

        this.range.push(SliceRepr {
            _ptr: *range.start,
            _len: range.end.val() - range.start.val(),
        });
        Ok(())
    }

    pub fn as_slice(&mut self) -> &mut [&'a [u8]] {
        assert!(self.attr.contains(Attr::READABLE));
        unsafe { mem::transmute(self.range.as_mut_slice()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [&'a mut [u8]] {
        assert!(self.attr.contains(Attr::WRITABLE));
        unsafe { mem::transmute(self.range.as_mut_slice()) }
    }
}

pub struct Virt {
    root: Mutex<Frame>,
    map: RwLock<RangeMap<LAddr, Mapping>>,
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
    fn is_committed(
        &self,
        addr: LAddr,
        count: usize,
        table: &mut Table,
        expect_attr: Attr,
    ) -> Result<bool, Error> {
        for addr in (0..count).map(|c| addr + (c << PAGE_SHIFT)) {
            if !self.attr.contains(expect_attr | Attr::USER_ACCESS) {
                return Err(EPERM);
            }

            if !table.la2pte(addr, ID_OFFSET).map_or(false, |e| e.is_set()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn commit(
        &mut self,
        addr: LAddr,
        offset: usize,
        count: NonZeroUsize,
        table: &mut Table,
        cpu_mask: usize,
        expect_attr: Attr,
    ) -> Result<(), Error> {
        let writable = self.attr.contains(Attr::WRITABLE);

        let mut flush = TlbFlushOnDrop::new(cpu_mask, addr);

        for (index, addr) in
            (0..count.get()).map(|c| (c + self.start_index + offset, addr + (c << PAGE_SHIFT)))
        {
            if !self.attr.contains(expect_attr | Attr::USER_ACCESS) {
                return Err(EPERM);
            }

            let entry = table.la2pte_alloc(addr, frames(), ID_OFFSET)?;
            if !entry.is_set() {
                let writable = writable.then_some(PAGE_SIZE);
                let (frame, _) = self.phys.commit(index, writable).await?;
                let base = frame.base();
                *entry = rv39_paging::Entry::new(base, self.attr, rv39_paging::Level::pt());
                flush.count += 1;
            }
        }
        Ok(())
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
                self.phys.flush(index, Some(dirty)).await?;
                entry.reset();
                flush.count += 1;
            } else {
                flush = TlbFlushOnDrop::new(cpu_mask, addr + PAGE_SIZE);
            }
        }
        Ok(())
    }

    fn deep_fork(&self) -> Mapping {
        Mapping {
            phys: Arsc::new(self.phys.clone_as(self.phys.is_cow(), 0, None)),
            start_index: self.start_index,
            attr: self.attr,
        }
    }
}

impl Virt {
    pub fn new(range: Range<LAddr>, init_root: Table) -> Arsc<Self> {
        Arsc::new(Virt {
            root: Mutex::new(init_root.into()),
            map: RwLock::new(RangeMap::new(range)),
            cpu_mask: AtomicUsize::new(0),
            _marker: PhantomPinned,
        })
    }

    /// # Safety
    ///
    /// The caller must ensure that the current executing address is mapped
    /// correctly.
    #[inline]
    pub unsafe fn load(self: Arsc<Self>) {
        tlb::set_virt(self)
    }

    pub async fn map(
        &self,
        addr: Option<LAddr>,
        phys: Phys,
        start_index: usize,
        count: usize,
        attr: Attr,
    ) -> Result<LAddr, Error> {
        log::trace!(
            "Virt::map at {addr:?}, start_index = {start_index}, count = {count}, attr = {attr:?}"
        );

        let mut map = self.map.write().await;
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
                    phys: Arsc::new(phys),
                    start_index,
                    attr: attr | Attr::VALID,
                };
                log::trace!("Virt::map result = {start:?}..{end:?}");
                map.try_insert(start..end, mapping).map_err(|_| ENOSPC)?;
                Ok(start)
            }
            None => {
                let layout = PAGE_LAYOUT.repeat(count)?.0;
                let aslr_key = AslrKey::new(ASLR_BIT, rand_riscv::rng(), layout);

                let ent = map.allocate_with_aslr(aslr_key, LAddr::val).ok_or(ENOSPC)?;
                let addr = *ent.key().start;
                log::trace!("Virt::map result = {:?}", ent.key());
                ent.insert(Mapping {
                    phys: Arsc::new(phys),
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

        let map = self.map.read().await;
        match start {
            None => map.find_free_with_aslr(aslr_key, LAddr::val),
            Some(start) => {
                let range = start..(start + (count << PAGE_SHIFT));
                (!map.intersects(range.clone())).then_some(range)
            }
        }
        .ok_or(ENOSPC)
    }

    pub async fn start_commit(&self, expect_attr: Attr) -> VirtCommitGuard {
        VirtCommitGuard {
            map: Some(self.map.upgradable_read().await),
            virt: self,
            attr: expect_attr,
            range: Vec::new(),
        }
    }

    pub async fn commit(&self, addr: LAddr, expect_attr: Attr) -> Result<(), Error> {
        let aligned_range = LAddr::from(addr.val() & !PAGE_MASK)
            ..LAddr::from((addr.val() + PAGE_SIZE) & !PAGE_MASK);

        log::trace!("Virt::commit {addr:?} => {aligned_range:?}");

        let mut map = self.map.write().await;
        let mut table = self.root.lock().await;

        if let Some((addr, mapping)) = map.intersection_mut(aligned_range.clone()).next() {
            log::trace!("Virt::commit found {addr:?}");
            let start = aligned_range.start.max(*addr.start);
            let end = aligned_range.end.min(*addr.end);
            let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
            let len = end.val() - start.val();
            let count = len >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .commit(
                        start,
                        offset,
                        count,
                        table.as_table(),
                        cpu_mask,
                        expect_attr,
                    )
                    .await?;
            }
            return Ok(());
        }
        Err(EFAULT)
    }

    pub async fn decommit_range(&self, range: Range<LAddr>) -> Result<(), Error> {
        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let mut map = self.map.write().await;
        let mut table = self.root.lock().await;

        for (addr, mapping) in map.intersection_mut(range.clone()) {
            let start = range.start.max(*addr.start);
            let end = range.end.min(*addr.end);
            let offset = (start.val() - addr.start.val()) >> PAGE_SHIFT;
            let count = (end.val() - start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(start, offset, count, table.as_table(), cpu_mask)
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

        let mut map = self.map.write().await;
        let mut table = self.root.lock().await;

        for (addr, mapping) in map.range_mut(range.clone()) {
            let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(*addr.start, 0, count, table.as_table(), cpu_mask)
                    .await?;
            }
            mapping.attr = attr;
        }

        if let Some((mut mapping, mut entry)) = map.split_entry(range.start) {
            let addr = entry.old_key();
            let offset = (range.start.val() - addr.start.val()) >> PAGE_SHIFT;
            let count = (addr.end.val() - range.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.start, offset, count, table.as_table(), cpu_mask)
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
            let addr = entry.old_key();
            let count = (range.end.val() - addr.start.val()) >> PAGE_SHIFT;

            if let Some(count) = NonZeroUsize::new(count) {
                let cpu_mask = self.cpu_mask.load(SeqCst);
                mapping
                    .decommit(range.end, 0, count, table.as_table(), cpu_mask)
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
        log::trace!("Virt::unmap {range:?}");

        if range.start.val() & PAGE_MASK != 0 || range.end.val() & PAGE_MASK != 0 {
            return Err(EINVAL);
        }
        let mut map = self.map.write().await;
        let mut table = self.root.lock().await;

        for (addr, mut mapping) in map.drain(range.clone()) {
            let count = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;
            if let Some(count) = NonZeroUsize::new(count) {
                mapping
                    .decommit(
                        addr.start,
                        0,
                        count,
                        table.as_table(),
                        self.cpu_mask.load(SeqCst),
                    )
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
                    .decommit(range.start, offset, count, table.as_table(), cpu_mask)
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
                    .decommit(range.end, 0, count, table.as_table(), cpu_mask)
                    .await?;
            }
            mapping.start_index += count;
            entry.set_latter(mapping);
        }
        Ok(())
    }

    pub async fn clear(&self) {
        log::trace!("Virt::clear table = {:p}", self.root.as_ptr());

        let mut map = self.map.write().await;
        let mut table = self.root.lock().await;

        let range = map.root_range();
        let range = *range.start..*range.end;
        let old = mem::replace(&mut *map, RangeMap::new(range.clone()));

        let count = (range.end.val() - range.start.val()) >> PAGE_SHIFT;
        table.as_table().unmap(range.clone(), frames(), ID_OFFSET);
        tlb::flush(self.cpu_mask.load(SeqCst), range.start, count);

        for (addr, mapping) in old {
            let count: usize = (addr.end.val() - addr.start.val()) >> PAGE_SHIFT;
            for index in 0..count {
                let dirty = mapping.attr.contains(Attr::DIRTY);
                let _ = mapping.phys.flush(index, Some(dirty)).await;
            }
        }
    }

    pub async fn deep_fork(&self, init_root: Table) -> Result<Arsc<Virt>, Error> {
        let mut map = self.map.write().await;
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
                        .decommit(*addr.start, 0, count, table.as_table(), cpu_mask)
                        .await?;
                }
            }
            let new_mapping = mapping.deep_fork();
            let _ = new_map.try_insert(*addr.start..*addr.end, new_mapping);
        }

        Ok(Arsc::new(Virt {
            root: Mutex::new(init_root.into()),
            map: RwLock::new(new_map),
            cpu_mask: AtomicUsize::new(0),
            _marker: PhantomPinned,
        }))
    }
}

impl Drop for Virt {
    fn drop(&mut self) {
        log::trace!("Virt::drop table = {:p}", self.root.as_ptr());

        let range = self.map.get_mut().root_range();
        let count = (range.end.val() - range.start.val()) >> PAGE_SHIFT;
        self.root
            .get_mut()
            .as_table()
            .unmap(*range.start..*range.end, frames(), ID_OFFSET);
        tlb::flush(self.cpu_mask.load(SeqCst), *range.start, count);
    }
}
