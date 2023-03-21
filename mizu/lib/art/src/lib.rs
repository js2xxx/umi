//! Asynchorous RunTime.
#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

pub mod queue;
mod rand;
mod sched;

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub use self::sched::Executor;

pub fn yield_now() -> YieldNow {
    YieldNow(false)
}

pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;

        let ret = sched::CX.try_with(|s| s.deferred.borrow_mut().push(cx.waker().clone()));
        if ret.is_none() {
            cx.waker().wake_by_ref()
        }

        Poll::Pending
    }
}
