#![no_std]

#[cfg_attr(feature = "qemu-virt", path = "qemu-virt.rs")]
mod imp;

pub use imp::*;
