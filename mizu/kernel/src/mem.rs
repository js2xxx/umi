use alloc::boxed::Box;
use core::ops::Range;

use kmem::Virt;
use rv39_paging::{Table, CANONICAL_PREFIX};
use spin::Lazy;

use crate::rxx::BOOT_PAGES;

const USER_RANGE: Range<usize> = 0x100000..((!CANONICAL_PREFIX) + 1);

pub fn kernel_table() -> &'static Table {
    static KERNEL_TABLE: Lazy<Box<Table>> = Lazy::new(|| Box::new(BOOT_PAGES));
    &KERNEL_TABLE
}

pub fn new_virt() -> Virt {
    Virt::new(
        USER_RANGE.start.into()..USER_RANGE.end.into(),
        Box::new(*kernel_table()),
    )
}
