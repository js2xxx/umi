use num_rational::Ratio;

pub const KERNEL_START: usize = 0x80200000;

pub const TIME_FREQ: Ratio<u128> = Ratio::new_raw(1, 12_500_000);
