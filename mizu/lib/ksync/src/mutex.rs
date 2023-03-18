use alloc::sync::Arc;
use core::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicUsize, Ordering::*},
    time::Duration,
};

use arsc_rs::Arsc;
use event_listener::Event;
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
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        self.acquire_slow().await;
        MutexGuard(self)
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
    ($ptr:ident => $guard:ident, $lock:ident, $try_lock:ident) => {
        #[clippy::has_significant_drop]
        pub struct $guard<T: ?Sized>($ptr<Mutex<T>>);

        impl<T: ?Sized> Mutex<T> {
            #[inline]
            pub async fn $lock(self: &$ptr<Self>) -> $guard<T> {
                async fn lock_impl<T: ?Sized>(this: $ptr<Mutex<T>>) -> $guard<T> {
                    if let Some(guard) = this.$try_lock() {
                        return guard;
                    }
                    this.acquire_slow().await;
                    $guard(this)
                }
                lock_impl(self.clone()).await
            }

            #[inline]
            pub fn $try_lock(self: &$ptr<Self>) -> Option<$guard<T>> {
                if self.state.compare_exchange(0, 1, Acquire, Acquire).is_ok() {
                    Some($guard(self.clone()))
                } else {
                    None
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

shared_ptr!(Arc => MutexGuardArc, lock_arc, try_lock_arc);
shared_ptr!(Arsc => MutexGuardArsc, lock_arsc, try_lock_arsc);

impl<T: ?Sized> Mutex<T> {
    #[cold]
    async fn acquire_slow(&self) {
        // Get the current time.
        let start = Instant::now();

        loop {
            // Start listening for events.
            let listener = self.lock_ops.listen();

            // Try locking if nobody is being starved.
            match self
                .state
                .compare_exchange(0, 1, Acquire, Acquire)
                .unwrap_or_else(|x| x)
            {
                // Lock acquired!
                0 => return,

                // Lock is held and nobody is starved.
                1 => {}

                // Somebody is starved.
                _ => break,
            }

            // Wait for a notification.
            listener.await;

            // Try locking if nobody is being starved.
            match self
                .state
                .compare_exchange(0, 1, Acquire, Acquire)
                .unwrap_or_else(|x| x)
            {
                // Lock acquired!
                0 => return,

                // Lock is held and nobody is starved.
                1 => {}

                // Somebody is starved.
                _ => {
                    // Notify the first listener in line because we probably received a
                    // notification that was meant for a starved task.
                    self.lock_ops.notify(1);
                    break;
                }
            }

            // If waiting for too long, fall back to a fairer locking strategy that will
            // prevent newer lock operations from starving us forever.
            if start.elapsed() > Duration::from_micros(500) {
                break;
            }
        }

        // Increment the number of starved lock operations.
        if self.state.fetch_add(2, Release) > usize::MAX / 2 {
            // In case of potential overflow, abort.
            panic!("Potential overflow");
        }

        // Decrement the counter when exiting this function.
        let _call = CallOnDrop(|| {
            self.state.fetch_sub(2, Release);
        });

        loop {
            // Start listening for events.
            let listener = self.lock_ops.listen();

            // Try locking if nobody else is being starved.
            match self
                .state
                .compare_exchange(2, 2 | 1, Acquire, Acquire)
                .unwrap_or_else(|x| x)
            {
                // Lock acquired!
                2 => return,

                // Lock is held by someone.
                s if s % 2 == 1 => {}

                // Lock is available.
                _ => {
                    // Be fair: notify the first listener and then go wait in line.
                    self.lock_ops.notify(1);
                }
            }

            // Wait for a notification.
            listener.await;

            // Try acquiring the lock without waiting for others.
            if self.state.fetch_or(1, Acquire) % 2 == 0 {
                return;
            }
        }
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
