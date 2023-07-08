//! Inspired by [`embassy-net`](https://crates.io/crates/embassy-net).

mod config;
mod driver;
mod socket;
mod stack;
#[allow(dead_code)]
mod time;

pub use self::{
    config::*,
    driver::{Features, Net, NetRx, NetTx},
    socket::*,
    stack::{LOOPBACK_IPV4, LOOPBACK_IPV6, Stack, StackBackground},
};
