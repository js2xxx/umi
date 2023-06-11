use core::{
    alloc::Layout,
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
    #[derive(Debug, Clone, Copy, Default)]
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

        const USER_R = Self::VALID.bits() | Self::READABLE.bits() | Self::USER_ACCESS.bits();
        const USER_RW = Self::USER_R.bits() | Self::WRITABLE.bits();
        const USER_RX = Self::USER_R.bits() | Self::EXECUTABLE.bits();
        const USER_RWX = Self::USER_RW.bits() | Self::EXECUTABLE.bits();
    }
}

/// attribute of page based on
impl Attr {
    #[inline]
    pub const fn builder() -> AttrBuilder {
        AttrBuilder::new()
    }

    pub const fn has_table(&self) -> bool {
        !self.intersects(Attr::READABLE.union(Attr::WRITABLE).union(Attr::EXECUTABLE))
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
    pub const fn readable(mut self, readable: bool) -> Self {
        if readable {
            self.attr = self.attr.union(Attr::READABLE);
        }
        self
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
    pub const UNSET: Entry = Entry(0);

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

    pub fn is_set(&self) -> bool {
        self.get(Level::pt()).1.contains(Attr::VALID)
    }

    #[inline]
    pub fn reset(&mut self) {
        *self = Entry::UNSET;
    }

    pub fn table(&self, level: Level, id_offset: usize) -> Option<&Table> {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(id_offset);
            Some(unsafe { &*ptr.cast() })
        } else {
            None
        }
    }

    pub fn table_mut(&mut self, level: Level, id_offset: usize) -> Option<&mut Table> {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(id_offset);
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
        id_offset: usize,
    ) -> Result<&mut Table, Error> {
        let (addr, attr) = self.get(Level::pt());
        if !attr.has_table() || level == Level::pt() {
            return Err(Error::EntryExistent(true));
        }
        Ok(if attr.contains(Attr::VALID) {
            let ptr = addr.to_laddr(id_offset);
            unsafe { &mut *ptr.cast() }
        } else {
            let mut ptr = alloc.alloc().ok_or(Error::OutOfMemory)?;
            let addr = LAddr::from(ptr).to_paddr(id_offset);
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
        id_offset: usize,
    ) {
        let (addr, attr) = self.get(Level::pt());
        if attr.contains(Attr::VALID) && attr.has_table() && level != Level::pt() {
            let ptr = addr.to_laddr(id_offset);
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

pub const PAGE_LAYOUT: Layout = Layout::new::<Table>();

impl Table {
    pub const fn new() -> Self {
        Table([Default::default(); NR_ENTRIES])
    }

    pub fn la2pte(&mut self, la: LAddr, id_offset: usize) -> Result<&mut Entry, Error> {
        let mut pte: &mut Entry;
        let mut t: &mut Table = self;
        for l in (1..=2u8).rev() {
            let level = Level::new(l);
            pte = &mut t[level.addr_idx(la.val(), false)];
            t = match pte.table_mut(level, id_offset) {
                None => return Err(Error::EntryExistent(false)),
                Some(tb) => tb,
            };
        }
        Ok(&mut t[Level::pt().addr_idx(la.val(), false)])
    }

    pub fn la2pte_alloc(
        &mut self,
        la: LAddr,
        alloc_func: &impl PageAlloc,
        id_offset: usize,
    ) -> Result<&mut Entry, Error> {
        let mut pte: &mut Entry;
        let mut t: &mut Table = self;
        for l in (1..=2u8).rev() {
            let level = Level::new(l);
            pte = &mut t[level.addr_idx(la.val(), false)];
            t = match pte.table_or_create(level, alloc_func, id_offset) {
                Ok(tb) => tb,
                Err(e) => return Err(e),
            };
        }
        Ok(&mut t[Level::pt().addr_idx(la.val(), false)])
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
    pub fn la2pa(&mut self, la: LAddr, is_kernel: bool, id_offset: usize) -> Result<PAddr, Error> {
        if la.val() >= BLANK_BEGIN && la.val() <= BLANK_END {
            return Err(Error::PermissionDenied);
        }
        let pte: Entry = match self.la2pte(la, id_offset) {
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

    pub fn reprotect(
        &mut self,
        la: LAddr,
        npages: usize,
        flag: Attr,
        id_offset: usize,
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
        for _ in 0..npages {
            pte = match self.la2pte(latmp, id_offset) {
                Err(e) => return Err(e),
                Ok(et) => et,
            };
            let (pa, _) = pte.get(Level::pt());
            *pte = Entry::new(pa, flag | Attr::VALID, Level::pt());
            latmp += PAGE_SIZE;
        }
        Ok(*pte)
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
        id_offset: usize,
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
        for i in 1..=npages {
            pte = match self.la2pte_alloc(latmp, alloc_func, id_offset) {
                Ok(e) => e,
                Err(e) => {
                    // ignore the last one, which may be 2kB at most.
                    let _ = self.user_unmap_npages(la, i, alloc_func, id_offset);
                    return Err(e);
                }
            };
            let (_, attr) = pte.get(Level::pt());
            if attr.contains(Attr::VALID) {
                return Err(Error::EntryExistent(true));
            } else {
                *pte = Entry::new(patmp, flags | Attr::VALID, Level::pt());
            }
            latmp += PAGE_SIZE;
            patmp += PAGE_SIZE;
        }
        Ok(*pte)
    }

    /// Unmap `npages` from `begin_la` to `end_la` and free empty pgtbl
    /// for LAddr starting at `la` and `free` corresponding pa if `need_free`
    ///
    /// `begin_la` and `end_la` should be page-aligned.
    ///
    /// # Return
    ///
    /// Return the last unmapped pte or `Error`
    pub fn user_unmap(
        &mut self,
        begin_la: LAddr,
        end_la: LAddr,
        level: Level,
        alloc: &impl PageAlloc,
        id_offset: usize,
    ) -> Result<Entry, Error> {
        if begin_la.in_page_offset() != 0 || end_la.in_page_offset() != 0 {
            return Err(Error::AddrMisaligned {
                vstart: Some(begin_la),
                vend: Some(end_la),
                phys: None,
            });
        }

        let begin_index = level.addr_idx(begin_la.val(), false);
        let end_index = level.addr_idx(end_la.val(), false);

        let mut pg_end: LAddr =
            LAddr::from(begin_la.val() | (level.page_mask() & !Level::pt().page_mask()));
        for index in begin_index..=end_index {
            let et = &mut self[index];
            if level == Level::pt() {
                let unreset = *et;
                et.reset();
                if index == end_index {
                    return Ok(unreset);
                }
            } else {
                let t = et.table_mut(level, id_offset);
                let tb = match t {
                    Some(tb) => tb,
                    None => continue,
                };
                if begin_index != end_index {
                    let a;
                    let b;
                    if index == begin_index {
                        a = begin_la;
                        b = pg_end;
                    } else if index == end_index {
                        a = pg_end + Level::pt().page_size();
                        b = end_la;
                    } else {
                        a = pg_end + Level::pt().page_size();
                        b = pg_end + level.page_size();
                        pg_end = b;
                    }
                    match tb.user_unmap(a, b, level.decrease().unwrap(), alloc, id_offset) {
                        Ok(mut et) => {
                            et.destroy_table(
                                level,
                                |table: &mut Table| {
                                    table.iter().all(|&e| !e.get(level).1.contains(Attr::VALID))
                                },
                                alloc,
                                id_offset,
                            );
                            if index == end_index {
                                return Ok(et);
                            }
                        }
                        Err(e) => return Err(e),
                    }
                } else {
                    match tb.user_unmap(
                        begin_la,
                        end_la,
                        level.decrease().unwrap(),
                        alloc,
                        id_offset,
                    ) {
                        Ok(mut et) => {
                            et.destroy_table(
                                level,
                                |table: &mut Table| {
                                    table.iter().all(|&e| !e.get(level).1.contains(Attr::VALID))
                                },
                                alloc,
                                id_offset,
                            );
                            return Ok(et);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Err(Error::RangeEmpty)
    }

    pub fn user_unmap_npages(
        &mut self,
        begin_la: LAddr,
        npages: usize,
        alloc: &impl PageAlloc,
        id_offset: usize,
    ) -> Result<Entry, Error> {
        self.user_unmap(
            begin_la,
            begin_la + (npages - 1) * PAGE_SIZE,
            Level::max(),
            alloc,
            id_offset,
        )
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

    extern crate alloc;
    use alloc::alloc::Global;
    use core::{
        alloc::{Allocator, Layout},
        ptr::NonNull,
    };

    use crate::{Attr, Entry, Error, LAddr, Level, PAddr, PageAlloc, Table, ID_OFFSET, PAGE_SIZE};

    #[test]
    fn test_la2pa() {
        assert_eq!(
            Err(Error::PermissionDenied),
            Table::new().la2pa(LAddr::from(0xffff_ff00_0000_0000u64), false, ID_OFFSET)
        );
    }

    const PAGE_LAYOUT: Layout = unsafe { Layout::from_size_align_unchecked(PAGE_SIZE, PAGE_SIZE) };

    struct Alloc();
    unsafe impl PageAlloc for Alloc {
        fn alloc(&self) -> Option<NonNull<Table>> {
            Global.allocate_zeroed(PAGE_LAYOUT).ok().map(NonNull::cast)
        }

        unsafe fn dealloc(&self, ptr: NonNull<Table>) {
            Global.deallocate(ptr.cast(), PAGE_LAYOUT)
        }
    }

    #[test]
    fn test_mapfuncs() {
        let a = Alloc();
        let mut tb = Table::new();
        let la_start = LAddr::from(0x0000_0000_9000_0000usize);
        let pa_start = PAddr::new(0x0000_0001_8000_0000);

        assert_eq!(
            tb.mappages(la_start, pa_start, Attr::empty(), 2, &a, 0),
            Ok(Entry::new(pa_start + 0x1000, Attr::VALID, Level::pt()))
        );

        assert_eq!(
            tb.reprotect(la_start, 2, Attr::KERNEL_R, 0),
            Ok(Entry::new(pa_start + 0x1000, Attr::KERNEL_R, Level::pt()))
        );

        assert_eq!(
            // tb.user_unmap(la_start + 0x1000, 1, &a, 0),
            tb.user_unmap(la_start, la_start + 0x1000, Level::max(), &a, 0),
            Ok(Entry::new(pa_start + 0x1000, Attr::KERNEL_R, Level::pt()))
        );

        assert_eq!(
            tb.la2pa(la_start, true, 0),
            Err(Error::EntryExistent(false))
        )
    }
}
