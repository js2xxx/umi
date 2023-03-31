use core::ops::Range;

use num_rational::Ratio;

pub const KERNEL_START_PHYS: usize = 0x80200000;
pub const KERNEL_START: usize = 0xffff_ffc0_8020_0000;

pub const TIME_FREQ: u128 = 12_500_000;
pub const TIME_FREQ_M: Ratio<u128> = Ratio::new_raw(2, 25); // 10^6 / FREQ

pub const MAX_HARTS: usize = 4;
pub const HART_RANGE: Range<usize> = 0..MAX_HARTS;
