#![cfg_attr(not(test), no_std)]
#![feature(const_trait_impl)]

#[cfg(not(test))]
mod instant;

#[cfg(test)]
pub use std::time::Instant;

#[cfg(not(test))]
pub use self::instant::Instant;
