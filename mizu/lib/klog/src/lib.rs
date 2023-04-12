#![cfg_attr(not(feature = "test"), no_std)]

mod imp;
mod logger;

pub use ksync_core::critical;

pub use self::{imp::Stdout, logger::init as init_logger};
