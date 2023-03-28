//! [Official manual](https://www.man7.org/linux/man-pages/man7/signal.7.html)

#![cfg_attr(not(test), no_std)]
#![feature(const_bool_to_option)]
#![feature(const_trait_impl)]

extern crate alloc;

mod action;
mod queue;
mod types;

pub use self::{action::*, queue::*, types::*};
