#![cfg_attr(not(test), no_std)]

pub mod imp;
mod logger;

pub use ksync::critical;

pub use self::logger::init as init_logger;
