use core::{ops::Range, pin::Pin};

use arsc_rs::Arsc;
use kmem::Virt;
use rv39_paging::CANONICAL_PREFIX;

use crate::rxx::KERNEL_PAGES;

const USER_RANGE: Range<usize> = 0x1000..((!CANONICAL_PREFIX) + 1);

pub fn new_virt() -> Pin<Arsc<Virt>> {
    Virt::new(USER_RANGE.start.into()..USER_RANGE.end.into(), KERNEL_PAGES)
}
