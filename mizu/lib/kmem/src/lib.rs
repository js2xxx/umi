#![no_std]
#![feature(pointer_byte_offsets)]
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
