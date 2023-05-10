#![cfg_attr(not(feature = "test"), no_std)]
#![feature(const_trait_impl)]

#[cfg(not(feature = "test"))]
mod instant;

#[cfg(feature = "test")]
pub use std::time::Instant;

#[cfg(not(feature = "test"))]
pub use self::instant::Instant;

pub trait InstantExt {
    fn to_su(&self) -> (u64, u64);
}

#[cfg(feature = "test")]
impl InstantExt for Instant {
    fn to_su(&self) -> (u64, u64) {
        (0, 0)
    }
}
