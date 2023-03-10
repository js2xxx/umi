use core::{
    fmt,
    ops::{Deref, DerefMut},
};

use bitflags::bitflags;
use static_assertions::const_assert;

use crate::{Level, PAddr, ENTRY_SIZE_SHIFT, NR_ENTRIES};

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
    }
}

impl Attr {
    #[inline]
    pub const fn builder() -> AttrBuilder {
        AttrBuilder::new()
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

#[derive(Copy, Clone)]
pub struct Entry(usize);
const_assert!(core::mem::size_of::<Entry>() == 1 << ENTRY_SIZE_SHIFT);

impl Entry {
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
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (addr, attr) = self.get(Level::pt());
        write!(f, "Entry(addr={:?}, attr={:?})", addr, attr)
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
