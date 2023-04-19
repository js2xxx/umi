#![cfg_attr(not(test), no_std)]
#![feature(result_option_inspect)]

extern crate alloc;

mod frame;
mod lru;
mod phys;
mod virt;

pub use self::{
    frame::{init_frames, Arena},
    phys::*,
};
