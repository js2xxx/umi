use core::{
    fmt,
    ops::{Deref, DerefMut},
};

use bitflags::bitflags;
use static_assertions::const_assert;

use crate::{
    Error, LAddr, Level, PAddr, PageAlloc, ENTRY_SIZE_SHIFT, ID_OFFSET, NR_ENTRIES, PAGE_SIZE,
};

bitflags! {
    pub struct Attr: usize {
        const VALID = 1 << 0;
        const READABLE = 1 << 1;
        const WRITABLE = 1 << 2;
        const EXECUTABLE = 1 << 3;
        const USER_ACCESS = 1 << 4;
        const GLOBAL = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY = 1 << 7;

        const KERNEL_R = Self::VALID.bits | Self::READABLE.bits | Self::GLOBAL.bits;
        const KERNEL_RW = Self::KERNEL_R.bits | Self::WRITABLE.bits;
        const KERNEL_RWX = Self::KERNEL_RW.bits | Self::EXECUTABLE.bits;
    }
}

/// attribute of page based on
impl Attr {
    #[inline]
    pub const fn builder() -> AttrBuilder {
        AttrBuilder::new()
    }

    pub const fn has_table(&self) -> bool {
        self.contains(Attr::READABLE.union(Attr::WRITABLE).union(Attr::EXECUTABLE))
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
#[derive(Copy, Clone)]
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
        Self(((*addr & level.paddr_mask()) >> 2) | attr.bits)
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

    /// Retrun corresponding pa with given la and flags
    ///
    /// # Error
    /// if not found, `Error::EntryExistent(false)`
    ///
    /// if la is illegal, `Error::OutOfMemory`
    pub fn la2pa(&self, la: LAddr, is_kernel: bool) -> Result<PAddr, Error> {
        if la < LAddr::from(config::KERNEL_START) {
            return Err(Error::OutOfMemory);
        }
        if is_kernel {
            // kernel addr has to be mapped high
            if la < PAddr::new(config::KERNEL_START).to_laddr(ID_OFFSET) {
                return Err(Error::OutOfMemory);
            }
            return Ok(la.to_paddr(ID_OFFSET));
        }
        let mut pte: Entry;
        let mut t: &Table = self;
        for l in (0..2u8).rev() {
            let level = Level::new(l);
            pte = t[level.addr_idx(la.val(), false)];
            t = match pte.table(level) {
                None => return Err(Error::EntryExistent(false)),
                Some(tb) => tb,
            };
        }
        pte = t[Level::new(3).addr_idx(la.val(), false)];
        let (pa, attr) = pte.get(Level::pt());
        if attr.contains(Attr::VALID) {
            Ok(pa)
        } else {
            Err(Error::EntryExistent(false))
        }
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
