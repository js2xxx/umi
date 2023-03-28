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
        match this.fut.poll(cx) {
            Poll::Ready(data) => Poll::Ready(data),
            Poll::Pending => match this.timer.poll(cx) {
                Poll::Ready(_) => Poll::Ready((this.out.take().unwrap())().into()),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

pub trait TimeOutExt: Future + Sized {
    fn on_timeout<G>(self, timer: impl Into<Timer>, out: G) -> OnTimeout<Self, G> {
        OnTimeout {
            fut: self,
            timer: timer.into(),
            out: Some(out),
        }
    }
}
impl<F: Future + Sized> TimeOutExt for F {}
