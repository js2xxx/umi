use crate::{LAddr, PAddr};

pub const PAGE_SHIFT: usize = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

pub const ENTRY_SIZE_SHIFT: usize = 3;
pub const NR_ENTRIES_SHIFT: usize = PAGE_SHIFT - ENTRY_SIZE_SHIFT;
pub const NR_ENTRIES: usize = 1 << NR_ENTRIES_SHIFT;

pub const CANONICAL_PREFIX: usize = 0xffff_ffc0_0000_0000;
pub const ID_OFFSET: usize = CANONICAL_PREFIX;

#[derive(Copy, Clone, Debug)]
pub enum Error {
    OutOfMemory,
    AddrMisaligned {
        vstart: Option<LAddr>,
        vend: Option<LAddr>,
        phys: Option<PAddr>,
    },
    RangeEmpty,
    EntryExistent(bool),
}
