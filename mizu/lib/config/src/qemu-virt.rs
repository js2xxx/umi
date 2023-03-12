use num_rational::Ratio;

pub const KERNEL_START: usize = 0x80200000;

pub const TIME_FREQ: u128 = 12_500_000;
pub const TIME_FREQ_M: Ratio<u128> = Ratio::new_raw(2, 25); // 10^6 / FREQ

pub const MAX_HARTS: usize = 4;
