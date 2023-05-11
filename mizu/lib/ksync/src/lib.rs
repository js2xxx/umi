#![cfg_attr(not(test), no_std)]
#![feature(once_cell)]
#![feature(thread_local)]

extern crate alloc;

mod broadcast;
pub mod epoch;
mod mpmc;
mod mutex;
mod rcu;
mod rw_lock;
mod semaphore;

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll},
};

pub use event_listener as event;
use futures_util::task::noop_waker;
pub use ksync_core::*;

pub use self::{broadcast::*, mpmc::*, mutex::*, rcu::*, rw_lock::*, semaphore::*};

pub fn poll_once<F: Future>(f: F) -> Option<F::Output> {
    let noop = noop_waker();
    let mut cx = Context::from_waker(&noop);
    match pin!(f).poll(&mut cx) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}
