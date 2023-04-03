/// TODO:
///
/// This PAddr and Entry should be fixed after kernel laddr layout,
/// such as guard pages, is clear.
extern crate alloc;
use alloc::collections::BTreeSet;
use core::{
    fmt,
    ops::{Deref, DerefMut},
};

use bitflags::bitflags;
use static_assertions::const_assert;

use crate::{
    Error, LAddr, Level, PAddr, PageAlloc, BLANK_BEGIN, BLANK_END, ENTRY_SIZE_SHIFT, ID_OFFSET,
    NR_ENTRIES, PAGE_SIZE,
};

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct Attr: usize {
        const VALID = 1 << 0;
        const READABLE = 1 << 1;
        const WRITABLE = 1 << 2;
        const EXECUTABLE = 1 << 3;
        const USER_ACCESS = 1 << 4;
        const GLOBAL = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY = 1 << 7;

        const KERNEL_R = Self::VALID.bits() | Self::READABLE.bits() | Self::GLOBAL.bits();
        const KERNEL_RW = Self::KERNEL_R.bits() | Self::WRITABLE.bits();
        const KERNEL_RWX = Self::KERNEL_RW.bits() | Self::EXECUTABLE.bits();
    }
}

/// attribute of page based on
impl Attr {
    #[inline]
    pub const fn builder() -> AttrBuilder {
        AttrBuilder::new()
    }

    pub const fn has_table(&self) -> bool {
        !self.contains(Attr::READABLE.union(Attr::WRITABLE).union(Attr::EXECUTABLE))
    }
}

impl const From<Entry> for Attr {
    fn from(value: Entry) -> Self {
        Self::from_bits_truncate(value.0)
    }
}

pub struct AttrBuilder {
    attr: Attr,
}

impl AttrBuilder {
    pub const fn new() -> AttrBuilder {
        AttrBuilder {
            attr: Attr::empty(),
        }
    }

    #[inline]
    pub const fn writable(mut self, writable: bool) -> Self {
        if writable {
            self.attr = self.attr.union(Attr::WRITABLE);
        }
        self
    }

    #[inline]
    pub const fn user_access(mut self, user_access: bool) -> Self {
        if user_access {
            self.attr = self.attr.union(Attr::USER_ACCESS);
        }
        self
    }

    #[inline]
    pub const fn executable(mut self, executable: bool) -> Self {
        if executable {
            self.attr = self.attr.union(Attr::EXECUTABLE);
        }
        self
    }

    #[inline]
    pub const fn build(self) -> Attr {
        self.attr
    }
}

impl const Default for AttrBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// pgtbl entries: 44 bit PPN | 12 bit offset
#[derive(Copy, Clone, Ord, Eq, PartialEq, PartialOrd)]
pub struct Entry(usize);
const_assert!(core::mem::size_of::<Entry>() == 1 << ENTRY_SIZE_SHIFT);

impl Entry {
    // get entry's pa (no offset) and its attribute
    #[inline]
    pub const fn get(self, level: Level) -> (PAddr, Attr) {
        let addr = (self.0 << 2) & level.paddr_mask();
        (PAddr::new(addr), self.into())
    }

    #[inline]
    pub const fn addr(self, level: Level) -> PAddr {
        self.get(level).0
    }

    #[inline]
    pub const fn new(addr: PAddr, attr: Attr, level: Level) -> Self {
        Self(((*addr & level.paddr_mask()) >> 2) | attr.bits())
    }

    #[inline]
    pub fn reset(&mut self) {
        *self = Entry(0);
    }

    pub fn table(&self, level: Level) -> Option<&Table> {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(ID_OFFSET);
            Some(unsafe { &*ptr.cast() })
        } else {
            None
        }
    }

    pub fn table_mut(&mut self, level: Level) -> Option<&mut Table> {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(ID_OFFSET);
            Some(unsafe { &mut *ptr.cast() })
        } else {
            None
        }
    }

    /// Get the page table stored in the entry, or create a new empty one if the
    /// entry is empty.
    ///
    /// This function is usually used when creating new mappings.
    ///
    /// # Errors
    ///
    /// Return an error if the entry is already a valid **leaf entry**, or
    /// memory exhaustion when creating a new table.
    pub fn table_or_create(
        &mut self,
        level: Level,
        alloc: &impl PageAlloc,
    ) -> Result<&mut Table, Error> {
        let (addr, attr) = self.get(Level::pt());
        if !attr.has_table() || level == Level::pt() {
            return Err(Error::EntryExistent(true));
        }
        Ok(if attr.contains(Attr::VALID) {
            let ptr = addr.to_laddr(ID_OFFSET);
            unsafe { &mut *ptr.cast() }
        } else {
            let mut ptr = alloc.alloc().ok_or(Error::OutOfMemory)?;
            let addr = LAddr::from(ptr).to_paddr(ID_OFFSET);
            *self = Self::new(addr, Attr::VALID, Level::pt());
            unsafe { ptr.as_mut() }
        })
    }

    /// Get the page table stored in the entry, or split it if it's a larger
    /// leaf entry.
    ///
    /// This function is usually used when reprotecting mappings.
    ///
    /// # Errors
    ///
    /// Return an error if the entry has no valid mapping, or memory exhaustion
    /// when creating a new table.
    pub fn table_or_split(
        &mut self,
        level: Level,
        alloc: &impl PageAlloc,
    ) -> Result<&mut Table, Error> {
        let (addr, attr) = self.get(Level::pt());
        if !attr.contains(Attr::VALID) || level == Level::pt() {
            return Err(Error::EntryExistent(false));
        }
        Ok(if attr.has_table() {
            let ptr = addr.to_laddr(ID_OFFSET);
            unsafe { &mut *ptr.cast() }
        } else {
            let mut ptr = alloc.alloc().ok_or(Error::OutOfMemory)?;

            let item_level = level.decrease().expect("Item level");
            let table = unsafe { ptr.as_mut() };
            let addrs = (0..NR_ENTRIES).map(|n| PAddr::new(*addr + n * PAGE_SIZE));
            for (item, addr) in table.iter_mut().zip(addrs) {
                *item = Self::new(addr, attr, item_level);
            }

            let table_addr = LAddr::from(ptr).to_paddr(ID_OFFSET);
            *self = Self::new(table_addr, Attr::VALID, Level::pt());
            table
        })
    }

    /// Destroy the table stored in the entry, if any. Doesn't have any effect
    /// on the data if not the case.
    ///
    /// This function is usually used when destroying mappings.
    ///
    /// # Arguments
    ///
    /// - `drop`: The drop function of the table, using it to destroy data in
    ///   the table. Returns whether the table should be destroyed.
    pub fn destroy_table(
        &mut self,
        level: Level,
        drop: impl FnOnce(&mut Table) -> bool,
        alloc: &impl PageAlloc,
    ) {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(ID_OFFSET);
            let table = unsafe { &mut *ptr.cast::<Table>() };
            if drop(table) {
                self.reset();
                unsafe { alloc.dealloc(table.into()) }
            }
        }
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (addr, attr) = self.get(Level::pt());
        write!(f, "Entry({addr:?}: {attr:?})")
    }
}

// Currently derived impl is not const, so implement it manually.
#[allow(clippy::derivable_impls)]
impl const Default for Entry {
    fn default() -> Self {
        Self(0)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(align(4096))]
pub struct Table([Entry; NR_ENTRIES]);
const_assert!(core::mem::size_of::<Table>() == crate::PAGE_SIZE);

impl Table {
    pub const fn new() -> Self {
        Table([Default::default(); NR_ENTRIES])
    }

    pub fn la2pte(&mut self, la: LAddr) -> (Result<&mut Entry, Error>, BTreeSet<(Entry, Level)>) {
        let mut touch_mark: BTreeSet<(Entry, Level)> = BTreeSet::new();
        let mut pte: &mut Entry;
        let mut t: &mut Table = self;
        for l in (0..2u8).rev() {
            let level = Level::new(l);
            pte = &mut t[level.addr_idx(la.val(), false)];
            touch_mark.insert((*pte, level));
            t = match pte.table_mut(level) {
                None => return (Err(Error::EntryExistent(false)), touch_mark),
                Some(tb) => tb,
            };
        }
        (
            Ok(&mut t[Level::pt().addr_idx(la.val(), false)]),
            touch_mark,
        )
    }

    pub fn la2pte_alloc(
        &mut self,
        la: LAddr,
        alloc_func: &impl PageAlloc,
    ) -> Result<&mut Entry, Error> {
        let mut pte: &mut Entry;
        let mut t: &mut Table = self;
        for l in (0..2u8).rev() {
            let level = Level::new(l);
            pte = &mut t[level.addr_idx(la.val(), false)];
            t = match pte.table_or_create(level, alloc_func) {
                Ok(tb) => tb,
                Err(e) => return Err(e),
            };
        }
        return Ok(&mut t[Level::pt().addr_idx(la.val(), false)]);
    }

    /// Look up in the pgtbl and retrun corresponding `pa` with given `la`.
    ///
    /// # Arguments
    ///
    /// - `is_kernel`: the type of pgtbl, If `is_kernel = false`, `la` shoud be
    ///   user accessable.
    ///
    /// # Error
    ///
    /// If not found, `Error::EntryExistent(false)`.
    ///
    /// If `la` is illegal, `Error::PerssionDenied`.
    pub fn la2pa(&mut self, la: LAddr, is_kernel: bool) -> Result<PAddr, Error> {
        if la.val() >= BLANK_BEGIN && la.val() <= BLANK_END {
            return Err(Error::PermissionDenied);
        }
        let pte: Entry = match self.la2pte(la).0 {
            Ok(et) => *et,
            Err(e) => return Err(e),
        };
        let (pa, attr) = pte.get(Level::pt());
        if attr.contains(Attr::VALID) {
            if attr.contains(Attr::USER_ACCESS) || is_kernel {
                Ok(pa)
            } else {
                Err(Error::PermissionDenied)
            }
        } else {
            Err(Error::EntryExistent(false))
        }
    }

    pub fn reprotect(&mut self, la: LAddr, npages: usize, flag: Attr) -> Result<&Entry, Error> {
        if la.in_page_offset() != 0 {
            return Err(Error::AddrMisaligned {
                vstart: (Some(la)),
                vend: (Some(LAddr::from(la.val() + npages + PAGE_SIZE))),
                phys: (None),
            });
        }
        let pte = match self.la2pte(la).0 {
            Err(e) => return Err(e),
            Ok(et) => et,
        };
        let (pa, _) = pte.get(Level::pt());
        *pte = Entry::new(pa, flag, Level::pt());
        Ok(pte)
    }

    /// Create `npages` sequential PTEs with `alloc_func` and set their Attr
    /// as **`flag | VALID`** for LAddr starting at `la` that refer to
    /// physical addresses starting at `pa`.
    ///
    /// `la` should be page-aligned.
    ///
    /// # Return
    ///
    /// Return the last allocated pte or `Error`
    pub fn mappages(
        &mut self,
        la: LAddr,
        pa: PAddr,
        flags: Attr,
        npages: usize,
        alloc_func: &impl PageAlloc,
    ) -> Result<Entry, Error> {
        if la.in_page_offset() != 0 || pa.in_page_offset() != 0 {
            return Err(Error::AddrMisaligned {
                vstart: (Some(la)),
                vend: (Some(LAddr::from(la.val() + npages + PAGE_SIZE))),
                phys: (Some(pa)),
            });
        }
        if npages == 0 {
            return Err(Error::RangeEmpty);
        }
        let mut latmp = la;
        let mut patmp = pa;
        let mut pte = &mut Entry(0);
        for _i in 0..npages {
            pte = match self.la2pte_alloc(latmp, alloc_func) {
                Ok(e) => e,
                Err(Error::OutOfMemory) => {
                    // ignore the last one, which may be 2kB at most.
                    return self.user_unmap(la, _i - 1, alloc_func, true);
                }
                Err(e) => return Err(e),
            };
            let (pa, attr) = pte.get(Level::pt());
            if attr.contains(Attr::VALID) {
                return Err(Error::EntryExistent(true));
            } else {
                *pte = Entry::new(pa, flags | Attr::VALID, Level::pt());
            }
            latmp = latmp + PAGE_SIZE;
            patmp += PAGE_SIZE;
        }
        Ok(*pte)
    }

    /// Unmap `npages` sequential PTEs as 0
    /// for LAddr starting at `la` and `free` corresponding pa if `need_free`
    ///
    /// `la` should be page-aligned.
    ///
    /// # Return
    ///
    /// Return the last unmapped pte or `Error`
    pub fn user_unmap(
        &mut self,
        la: LAddr,
        npages: usize,
        alloc_func: &impl PageAlloc,
    ) -> Result<Entry, Error> {
        if la.in_page_offset() != 0 {
            return Err(Error::AddrMisaligned {
                vstart: (Some(la)),
                vend: (Some(LAddr::from(la.val() + npages + PAGE_SIZE))),
                phys: (None),
            });
        }
        let mut latmp = la;
        let mut pte = &mut Entry(0);
        let mut entry_set: BTreeSet<(Entry, Level)> = BTreeSet::new();
        for _i in 0..npages {
            let (res, mut bs) = self.la2pte(latmp);
            match res {
                Ok(et) => pte = et,
                Err(e) => return Err(e),
            };
            entry_set.append(&mut bs);
            let (_pa, attr) = pte.get(Level::pt());
            if attr.contains(Attr::VALID) {
                if attr.has_table() {
                    return Err(Error::PermissionDenied);
                } else {
                    pte.reset();
                }
            } else {
                return Err(Error::PermissionDenied);
            }
            latmp = latmp + PAGE_SIZE;
        }
        // check unused table
        for i in entry_set {
            let mut et = i.0;
            let l = i.1;
            et.destroy_table(
                l,
                |table: &mut Table| table.0.iter().all(|&e| !e.get(l).1.contains(Attr::VALID)),
                alloc_func,
            )
        }
        Ok(*pte)
    }
}

impl const Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

impl const Deref for Table {
    type Target = [Entry; crate::NR_ENTRIES];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl const DerefMut for Table {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[macro_export]
macro_rules! table_1g {
    (@ITEM $obj:ident, $virt:expr, $phys:expr, $attr:expr) => {
        {
            let index = Level::max().addr_idx($virt, false);
            $obj[index] = Entry::new(PAddr::new($phys), $attr, Level::pt());
        }
    };
    [$($virt:expr => $phys:expr, $attr:expr);+$(;)?] => {
        {
            let mut table = Table::new();
            $(
                table_1g!(@ITEM table, $virt, $phys, $attr);
            )+
            table
        }
    };
    [] => { Table::new() };
}

#[cfg(test)]
mod tests {
    use crate::{Error, LAddr, Table};

    #[test]
    fn test_la2pa() {
        assert_eq!(
            Err(Error::PermissionDenied),
            Table::new().la2pa(LAddr::from(0xffff_ff00_0000_0000u64), false)
        );
    }
}
