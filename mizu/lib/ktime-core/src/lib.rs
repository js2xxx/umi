#![cfg_attr(not(feature = "test"), no_std)]
#![feature(const_trait_impl)]

#[cfg(not(feature = "test"))]
mod instant;

#[cfg(feature = "test")]
pub use std::time::Instant;

#[cfg(not(feature = "test"))]
pub use self::instant::Instant;
