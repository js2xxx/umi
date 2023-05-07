use alloc::sync::Arc;
use core::{
    borrow::Borrow,
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::*},
    task::{ready, Context, Poll},
    time::Duration,
};

use arsc_rs::Arsc;
use event_listener::{Event, EventListener};
use futures_lite::Future;
use ktime::Instant;

/// Async mutexes, based on the implementation of [`async-lock`].
///
/// [`async-lock`]: https://github.com/smol-rs/async-lock
pub struct Mutex<T: ?Sized> {
    state: AtomicUsize,
    lock_ops: Event,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Mutex {
            state: AtomicUsize::new(0),
            lock_ops: Event::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    pub fn as_ptr(&self) -> *mut T {
        self.data.get()
    }

    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.state.compare_exchange(0, 1, Acquire, Acquire) {
            Ok(_) => Some(MutexGuard(self)),
            Err(_) => None,
        }
    }
}

#[clippy::has_significant_drop]
pub struct MutexGuard<'a, T: ?Sized>(&'a Mutex<T>);

unsafe impl<T: ?Sized + Send> Send for MutexGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub fn lock(&self) -> Lock<'_, T> {
        Lock {
            mutex: self,
            acquire_slow: None,
        }
    }
}

#[must_use = "futures do nothing unless you 'await' or poll them"]
pub struct Lock<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    acquire_slow: Option<AcquireSlow<&'a Mutex<T>, T>>,
}

impl<'a, T: ?Sized> Unpin for Lock<'a, T> {}

impl<T: ?Sized> fmt::Debug for Lock<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Lock { .. }")
    }
}

impl<'a, T: ?Sized> Future for Lock<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match this.acquire_slow.as_mut() {
                None => {
                    // Try the fast path before trying to register slowly.
                    match this.mutex.try_lock() {
                        Some(guard) => return Poll::Ready(guard),
                        None => {
                            this.acquire_slow = Some(AcquireSlow::new(this.mutex));
                        }
                    }
                }

                Some(acquire_slow) => {
                    // Continue registering slowly.
                    let value = ready!(Pin::new(acquire_slow).poll(cx));
                    return Poll::Ready(MutexGuard(value));
                }
            }
        }
    }
}

impl<T: ?Sized> MutexGuard<'_, T> {
    pub fn source(guard: &Self) -> &'_ Mutex<T> {
        guard.0
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.0.state.fetch_sub(1, Release);
        self.0.lock_ops.notify(1);
    }
}

macro_rules! shared_ptr {
    ($ptr:ident:: $lock_fut:ident => $guard:ident, $lock:ident, $try_lock:ident) => {
        #[clippy::has_significant_drop]
        pub struct $guard<T: ?Sized>($ptr<Mutex<T>>);

        #[must_use = "futures do nothing unless you `await` or poll them"]
        pub enum $lock_fut<T: ?Sized> {
            /// We have not tried to poll the fast path yet.
            Unpolled($ptr<Mutex<T>>),

            /// We are acquiring the mutex through the slow path.
            AcquireSlow(AcquireSlow<$ptr<Mutex<T>>, T>),

            /// Empty hole to make taking easier.
            Empty,
        }

        impl<T: ?Sized> Unpin for $lock_fut<T> {}

        impl<T: ?Sized> fmt::Debug for $lock_fut<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LockArc { .. }")
            }
        }

        impl<T: ?Sized> Mutex<T> {
            #[inline]
            pub fn $try_lock(self: &$ptr<Self>) -> Option<$guard<T>> {
                if self.state.compare_exchange(0, 1, Acquire, Acquire).is_ok() {
                    Some($guard(self.clone()))
                } else {
                    None
                }
            }

            #[inline]
            pub fn $lock(self: &$ptr<Self>) -> $lock_fut<T> {
                $lock_fut::Unpolled(self.clone())
            }
        }

        impl<T: ?Sized> Future for $lock_fut<T> {
            type Output = $guard<T>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let this = self.get_mut();

                loop {
                    match mem::replace(this, Self::Empty) {
                        Self::Unpolled(mutex) => {
                            // Try the fast path before trying to register slowly.
                            match mutex.$try_lock() {
                                Some(guard) => return Poll::Ready(guard),
                                None => {
                                    *this = Self::AcquireSlow(AcquireSlow::new(mutex.clone()));
                                }
                            }
                        }

                        Self::AcquireSlow(mut acquire_slow) => {
                            // Continue registering slowly.
                            let value = match Pin::new(&mut acquire_slow).poll(cx) {
                                Poll::Pending => {
                                    *this = Self::AcquireSlow(acquire_slow);
                                    return Poll::Pending;
                                }
                                Poll::Ready(value) => value,
                            };
                            return Poll::Ready($guard(value));
                        }

                        Self::Empty => panic!("future polled after completion"),
                    }
                }
            }
        }

        impl<T: ?Sized> $guard<T> {
            pub fn source(guard: &Self) -> &'_ Mutex<T> {
                &guard.0
            }
        }

        impl<T: ?Sized + fmt::Debug> fmt::Debug for $guard<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + fmt::Display> fmt::Display for $guard<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                (**self).fmt(f)
            }
        }

        impl<T: ?Sized> Deref for $guard<T> {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                unsafe { &*self.0.data.get() }
            }
        }

        impl<T: ?Sized> DerefMut for $guard<T> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                unsafe { &mut *self.0.data.get() }
            }
        }

        impl<T: ?Sized> Drop for $guard<T> {
            fn drop(&mut self) {
                self.0.state.fetch_sub(1, Release);
                self.0.lock_ops.notify(1);
            }
        }
    };
}

shared_ptr!(Arc::LockArc => MutexGuardArc, lock_arc, try_lock_arc);
shared_ptr!(Arsc::LockArsc => MutexGuardArsc, lock_arsc, try_lock_arsc);

pub struct AcquireSlow<B: Borrow<Mutex<T>>, T: ?Sized> {
    /// Reference to the mutex.
    mutex: Option<B>,

    /// The event listener waiting on the mutex.
    listener: Option<EventListener>,

    /// The point at which the mutex lock was started.
    start: Option<Instant>,

    /// This lock operation is starving.
    starved: bool,

    /// Capture the `T` lifetime.
    _marker: PhantomData<T>,
}

impl<B: Borrow<Mutex<T>> + Unpin, T: ?Sized> Unpin for AcquireSlow<B, T> {}

impl<T: ?Sized, B: Borrow<Mutex<T>>> AcquireSlow<B, T> {
    /// Create a new `AcquireSlow` future.
    #[cold]
    fn new(mutex: B) -> Self {
        AcquireSlow {
            mutex: Some(mutex),
            listener: None,
            start: None,
            starved: false,
            _marker: PhantomData,
        }
    }

    /// Take the mutex reference out, decrementing the counter if necessary.
    fn take_mutex(&mut self) -> Option<B> {
        let mutex = self.mutex.take();

        if self.starved {
            if let Some(mutex) = mutex.as_ref() {
                // Decrement this counter before we exit.
                mutex.borrow().state.fetch_sub(2, Release);
            }
        }

        mutex
    }
}

impl<T: ?Sized, B: Unpin + Borrow<Mutex<T>>> Future for AcquireSlow<B, T> {
    type Output = B;

    #[cold]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let start = *this.start.get_or_insert_with(Instant::now);
        let mutex = this
            .mutex
            .as_ref()
            .expect("future polled after completion")
            .borrow();

        // Only use this hot loop if we aren't currently starved.
        if !this.starved {
            loop {
                // Start listening for events.
                match &mut this.listener {
                    listener @ None => {
                        // Start listening for events.
                        *listener = Some(mutex.lock_ops.listen());

                        // Try locking if nobody is being starved.
                        match mutex
                            .state
                            .compare_exchange(0, 1, Acquire, Acquire)
                            .unwrap_or_else(|x| x)
                        {
                            // Lock acquired!
                            0 => return Poll::Ready(this.take_mutex().unwrap()),

                            // Lock is held and nobody is starved.
                            1 => {}

                            // Somebody is starved.
                            _ => break,
                        }
                    }
                    Some(ref mut listener) => {
                        // Wait for a notification.
                        ready!(Pin::new(listener).poll(cx));
                        this.listener = None;

                        // Try locking if nobody is being starved.
                        match mutex
                            .state
                            .compare_exchange(0, 1, Acquire, Acquire)
                            .unwrap_or_else(|x| x)
                        {
                            // Lock acquired!
                            0 => return Poll::Ready(this.take_mutex().unwrap()),

                            // Lock is held and nobody is starved.
                            1 => {}

                            // Somebody is starved.
                            _ => {
                                // Notify the first listener in line because we probably received a
                                // notification that was meant for a starved task.
                                mutex.lock_ops.notify(1);
                                break;
                            }
                        }

                        // If waiting for too long, fall back to a fairer locking strategy that will
                        // prevent newer lock operations from starving us
                        // forever.
                        if start.elapsed() > Duration::from_micros(500) {
                            break;
                        }
                    }
                }
            }

            // Increment the number of starved lock operations.
            if mutex.state.fetch_add(2, Release) > usize::MAX / 2 {
                // In case of potential overflow, abort.
                panic!("Potential overflow");
            }

            // Indicate that we are now starving and will use a fairer locking strategy.
            this.starved = true;
        }

        // Fairer locking loop.
        loop {
            match &mut this.listener {
                listener @ None => {
                    // Start listening for events.
                    *listener = Some(mutex.lock_ops.listen());

                    // Try locking if nobody else is being starved.
                    match mutex
                        .state
                        .compare_exchange(2, 2 | 1, Acquire, Acquire)
                        .unwrap_or_else(|x| x)
                    {
                        // Lock acquired!
                        2 => return Poll::Ready(this.take_mutex().unwrap()),

                        // Lock is held by someone.
                        s if s % 2 == 1 => {}

                        // Lock is available.
                        _ => {
                            // Be fair: notify the first listener and then go wait in line.
                            mutex.lock_ops.notify(1);
                        }
                    }
                }
                Some(ref mut listener) => {
                    // Wait for a notification.
                    ready!(Pin::new(listener).poll(cx));
                    this.listener = None;

                    // Try acquiring the lock without waiting for others.
                    if mutex.state.fetch_or(1, Acquire) % 2 == 0 {
                        return Poll::Ready(this.take_mutex().unwrap());
                    }
                }
            }
        }
    }
}

impl<T: ?Sized, B: Borrow<Mutex<T>>> Drop for AcquireSlow<B, T> {
    fn drop(&mut self) {
        // Make sure the starvation counter is decremented.
        self.take_mutex();
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Locked;
        impl fmt::Debug for Locked {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<locked>")
            }
        }

        match self.try_lock() {
            None => f.debug_struct("Mutex").field("data", &Locked).finish(),
            Some(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
        }
    }
}

impl<T> From<T> for Mutex<T> {
    fn from(val: T) -> Mutex<T> {
        Mutex::new(val)
    }
}

impl<T: Default + ?Sized> Default for Mutex<T> {
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
