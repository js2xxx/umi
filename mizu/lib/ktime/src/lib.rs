#![cfg_attr(not(feature = "test"), no_std)]

extern crate alloc;

mod timer;

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_lite::FutureExt;
pub use ktime_core::*;
use pin_project::pin_project;

pub use self::timer::{Period, Timer};

pub fn timer_tick() {
    timer::TIMER_QUEUE.tick();
}

pub async fn sleep(duration: Duration) {
    Timer::after(duration).await;
}

#[must_use = "futures do nothing unless polled"]
#[pin_project]
pub struct OnTimeout<F, G> {
    #[pin]
    fut: F,
    timer: Timer,
    out: Option<G>,
}

impl<F, G, T> Future for OnTimeout<F, G>
where
    F: Future,
    G: FnOnce() -> T,
    F::Output: From<T>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(data) = this.fut.poll(cx) {
            Poll::Ready(data)
        } else if this.timer.poll(cx).is_ready() {
            Poll::Ready((this.out.take().unwrap())().into())
        } else {
            Poll::Pending
        }
    }
}

#[must_use = "futures do nothing unless polled"]
#[pin_project]
pub struct OkOrTimeout<F, G> {
    #[pin]
    fut: F,
    timer: Timer,
    out: Option<G>,
}

impl<F, G, E> Future for OkOrTimeout<F, G>
where
    F: Future,
    G: FnOnce() -> E,
{
    type Output = Result<F::Output, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(data) = this.fut.poll(cx) {
            Poll::Ready(Ok(data))
        } else if this.timer.poll(cx).is_ready() {
            Poll::Ready(Err((this.out.take().unwrap())()))
        } else {
            Poll::Pending
        }
    }
}

pub trait TimeOutExt: Future + Sized {
    fn on_timeout<T, G: FnOnce() -> T>(
        self,
        timer: impl Into<Timer>,
        out: G,
    ) -> OnTimeout<Self, G> {
        OnTimeout {
            fut: self,
            timer: timer.into(),
            out: Some(out),
        }
    }

    fn ok_or_timeout<E, G: FnOnce() -> E>(
        self,
        timer: impl Into<Timer>,
        err: G,
    ) -> OkOrTimeout<Self, G> {
        OkOrTimeout {
            fut: self,
            timer: timer.into(),
            out: Some(err),
        }
    }
}
impl<F: Future + Sized> TimeOutExt for F {}
