//! This crate contains what aims to be the simplest possible implementation of a valid executor.
//! Instead of nicely parking the thread and waiting for the future to wake it up, it continuously
//! polls the future until the future is ready. This will probably use a lot of CPU, so be careful
//! when you use it.
//!
//! ```rust
//! assert_eq!(12, spin_on::spin_on(async {3 * 4}))
//! ```
//!
//! The advantages of this crate are:
//!
//! - It is really simple
//! - It should work on basically any platform
//! - It has no dependency on `std` or on an allocator
//! - It only has one dependency
//!
//! ## The Design
//!
//! This crate intentionally violates one of the guidelines of `Future`: as of Rust 1.46, the
//! [runtime characteristics](https://doc.rust-lang.org/1.46.0/core/future/trait.Future.html#runtime-characteristics)
//! of `core::future::Future` says:
//!
//! > The `poll` function is not called repeatedly in a tight loop -- instead, it should only be
//! called when the future indicates that it is ready to make progress (by calling `wake()`).
//!
//! When no Future can make progress, a well-behaved executor should suspend execution and
//! wait until an external event resumes execution. As far as I know, though, there is not a
//! cross-platform way to suspend a thread. With Rust's `std`, this would be done by using
//! `thread::park`. But, for example, if you're on an embedded board with an ARM Cortex M processor,
//! you would instead use a WFE or WFI instruction. So, an execution-suspending executor would need
//! to be adapted for each different platform.
//!
//! What price do we pay for violating this guideline? This executor is a "resource hog," since it
//! continually runs the CPU at 100%. On an embedded system, this could cause increased power usage.
//! In a situation where many programs are running, this could make your application waste CPU
//! resources that could otherwise be put to good use by other applications.
//!
//! When might this be useful?
//!
//! - Running async applications on a platform that doesn't have an executor
//! - Testing that an async crate works with `no_std`
//! - Educational purposes?
//! - Implementing an application where you don't care about performance
#![no_std]

use core::future::Future;
use core::sync::atomic::spin_loop_hint;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

// TODO audit that this noop waker implementations aren't doing anything bad
unsafe fn rwclone(_p: *const ()) -> RawWaker {
    noop_waker()
}

unsafe fn rwwake(_p: *const ()) {}

unsafe fn rwwakebyref(_p: *const ()) {}

unsafe fn rwdrop(_p: *const ()) {}

static VTABLE: RawWakerVTable = RawWakerVTable::new(rwclone, rwwake, rwwakebyref, rwdrop);

/// The simplest way to create a noop waker in Rust. You would only ever want to use this with
/// an executor that polls continuously. Thanks to user 2e71828 on
/// [this Rust forum post](https://users.rust-lang.org/t/simplest-possible-block-on/48364/2).
fn noop_waker() -> RawWaker {
    static DATA: () = ();
    RawWaker::new(&DATA, &VTABLE)
}

/// Continuously poll a future until it returns `Poll::Ready`. This is not normally how an
/// executor should work, because it runs the CPU at 100%.
pub fn spin_on<F: Future>(future: F) -> F::Output {
    pin_utils::pin_mut!(future);
    let waker = &unsafe { Waker::from_raw(noop_waker()) };
    let mut cx = Context::from_waker(waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        spin_loop_hint();
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    struct CountFuture(usize);

    impl Future for CountFuture {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0 > 0 {
                self.0 -= 1;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        }
    }

    #[test]
    fn ready() {
        crate::spin_on(async {});
    }

    #[test]
    fn count() {
        crate::spin_on(CountFuture(10));
    }
}
