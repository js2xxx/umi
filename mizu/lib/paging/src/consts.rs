use core::ptr::NonNull;

use crate::{LAddr, PAddr, Table};

/// also PA_OFFSET, only fixed when fixed pgsize
pub const PAGE_SHIFT: u32 = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

pub const ENTRY_SIZE_SHIFT: u32 = 3;
pub const NR_ENTRIES_SHIFT: u32 = PAGE_SHIFT - ENTRY_SIZE_SHIFT;
pub const NR_ENTRIES: usize = 1 << NR_ENTRIES_SHIFT;

pub const CANONICAL_PREFIX: usize = 0xffff_ffc0_0000_0000;
pub const ID_OFFSET: usize = CANONICAL_PREFIX;

pub const BLANK_BEGIN: usize = (1 << 38) - 1;
pub const BLANK_END: usize = CANONICAL_PREFIX - 1;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Error {
    OutOfMemory,
    AddrMisaligned {
        vstart: Option<LAddr>,
        vend: Option<LAddr>,
        phys: Option<PAddr>,
    },
    RangeEmpty,
    EntryExistent(bool),
    PermissionDenied,
}

/// # Safety
///
/// The `alloc` function must return a pointer that has its ownership,
/// representing a zeroed `Table`.
pub unsafe trait PageAlloc {
    fn alloc(&self) -> Option<NonNull<Table>>;

    /// # Safety
    ///
    /// `ptr` must has its ownership and must be previously returned by the
    /// `alloc` function.
    unsafe fn dealloc(&self, ptr: NonNull<Table>);
}
