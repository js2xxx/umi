//! Inspired by [`embassy-net`](https://crates.io/crates/embassy-net).

mod config;
mod driver;
mod socket;
mod stack;
mod time;

pub use self::{
    driver::{Features, Net, NetRx, NetTx},
    socket::*,
    stack::Stack,
};
