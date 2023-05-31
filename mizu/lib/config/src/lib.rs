#![no_std]

#[cfg_attr(feature = "qemu-virt", path = "qemu-virt.rs")]
mod imp;

use core::time::Duration;

pub use imp::*;

pub const RAM_START: usize = 0x8000_0000;
pub const RAM_END: usize = RAM_START + RAM_SIZE;

pub const VIRT_START: usize = 0xffff_ffc0_0000_0000 + RAM_START;
pub const VIRT_END: usize = VIRT_START + RAM_SIZE;

pub fn to_duration(raw: u64) -> Duration {
    let micros = TIME_FREQ_M.numer() * raw as u128 / TIME_FREQ_M.denom();
    Duration::from_secs((micros / 1_000_000) as u64)
        + Duration::from_micros((micros % 1_000_000) as u64)
}
