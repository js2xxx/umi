#![cfg_attr(not(feature = "test"), no_std)]
#![feature(const_convert)]
#![feature(const_mut_refs)]
#![feature(const_option_ext)]
#![feature(const_trait_impl)]
#![cfg_attr(test, feature(allocator_api))]
#[cfg(test)]
extern crate alloc;

mod addr;
mod consts;
mod entry;
mod level;

pub use self::{addr::*, consts::*, entry::*, level::*};
