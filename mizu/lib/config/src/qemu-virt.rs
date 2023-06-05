use core::ops::Range;

use num_rational::Ratio;

use crate::{RAM_START, VIRT_START};

pub const RAM_SIZE: usize = 4 * 1024 * 1024 * 1024;

pub const KERNEL_OFFSET: usize = 0x200000;
pub const KERNEL_START_PHYS: usize = RAM_START + KERNEL_OFFSET;
pub const KERNEL_START: usize = VIRT_START + KERNEL_OFFSET;

pub const TIME_FREQ: u128 = 10_000_000;
pub const TIME_FREQ_M: Ratio<u128> = Ratio::new_raw(1, 10); // 10^6 / FREQ

pub const MAX_HARTS: usize = 4;
pub const HART_RANGE: Range<usize> = 0..MAX_HARTS;
