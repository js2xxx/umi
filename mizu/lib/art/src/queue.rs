//! Work-stealing queues based on [`Tokio`]'s implementation.
//!
//! [`Tokio`]: https://tokio.rs/

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU32, AtomicU64, Ordering::*},
};

use arsc_rs::Arsc;

struct Inner<T, const N: usize> {
    head: AtomicU64,
    tail: AtomicU32,
    buffer: [UnsafeCell<MaybeUninit<T>>; N],
}

impl<T, const N: usize> Inner<T, N> {
    fn len(&self) -> u32 {
        let (_, real) = unpack(self.head.load(Acquire));
        let tail = self.tail.load(Acquire);
        tail.wrapping_sub(real)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T, const N: usize> Inner<T, N> {
    const ASSERT_POWER_OF_2: usize = 0 - (N & (N - 1));
    const ASSERT_GREATER_THAN_1: usize = N - 2;
    const ASSERT_LESS_OR_EQUAL_THAN_U8_MAX: usize = u8::MAX as usize + 1 - N;

    const ASSERT: usize = Self::ASSERT_POWER_OF_2
        & Self::ASSERT_GREATER_THAN_1
        & Self::ASSERT_LESS_OR_EQUAL_THAN_U8_MAX;

    const MASK: usize = N - 1;
}

/// Note:
///
/// `N` must satisfy the conditions above:
///
/// 1. Power of 2;
/// 2. Greater than 1;
/// 3. Less or equal to 256.
///
/// As such, `N` only exists in the following sequence:
///
///     2, 4, 8, 16, 32, 64, 128, 256
pub struct Local<T, const N: usize>(Arsc<Inner<T, N>>);

unsafe impl<T: Send, const N: usize> Send for Local<T, N> {}

#[derive(Clone)]
pub struct Stealer<T, const N: usize>(Arsc<Inner<T, N>>);

unsafe impl<T: Send, const N: usize> Send for Stealer<T, N> {}
unsafe impl<T: Send, const N: usize> Sync for Stealer<T, N> {}

impl<T, const N: usize> Local<T, N> {
    const UNINIT: UnsafeCell<MaybeUninit<T>> = UnsafeCell::new(MaybeUninit::uninit());

    const MASK: usize = Inner::<T, N>::MASK;
    const BATCH_TO_BACKUP: u32 = (N / 2) as u32;

    pub fn new() -> Self {
        let _ = Inner::<T, N>::ASSERT;

        let inner = Arsc::new(Inner {
            head: Default::default(),
            tail: Default::default(),
            buffer: [Self::UNINIT; N],
        });

        Local(inner)
    }

    pub fn push(&mut self, mut value: T, mut inject: impl FnMut(T)) {
        let tail = loop {
            let head = self.0.head.load(Acquire);
            let (steal, real) = unpack(head);

            let tail = self.0.tail.load(Relaxed);

            if tail.wrapping_sub(steal) < N as u32 {
                break tail;
            }
            if steal != real {
                inject(value);
                return;
            }
            match self.push_contended(value, real, tail, &mut inject) {
                Ok(()) => return,
                Err(v) => value = v,
            }
        };

        let index = tail as usize & Self::MASK;
        // SAFETY: Only one producer.
        unsafe { self.0.buffer[index].get().write(MaybeUninit::new(value)) };
        self.0.tail.store(tail.wrapping_add(1), Release);
    }

    fn push_contended(
        &mut self,
        value: T,
        head: u32,
        tail: u32,
        mut inject: impl FnMut(T),
    ) -> Result<(), T> {
        assert_eq!(
            tail.wrapping_sub(head) as usize,
            N,
            "queue should be full! head: {head}; tail: {tail}"
        );

        let prev = pack(head, head);
        let next_head = head.wrapping_add(Self::BATCH_TO_BACKUP);
        let next = pack(next_head, next_head);

        let res = self.0.head.compare_exchange(prev, next, Release, Relaxed);
        if res.is_err() {
            return Err(value);
        }

        inject(value);
        let batch = (head..next_head).map(|head| {
            let index = head as usize & Self::MASK;
            // SAFETY: Successful CAS assumed ownership of these values.
            unsafe { self.0.buffer[index].get().read().assume_init() }
        });
        batch.for_each(inject);

        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        let mut head = self.0.head.load(Acquire);
        let index = loop {
            let (steal, real) = unpack(head);

            let tail = self.0.tail.load(Relaxed);

            if real == tail {
                return None;
            }

            let next_real = real.wrapping_add(1);
            let next = if steal == real {
                pack(next_real, next_real)
            } else {
                assert_ne!(steal, next_real);
                pack(steal, next_real)
            };

            match self.0.head.compare_exchange(head, next, AcqRel, Acquire) {
                Ok(_) => break real as usize & Self::MASK,
                Err(h) => head = h,
            }
        };
        // SAFETY: Successful CAS assumed ownership of the value.
        Some(unsafe { self.0.buffer[index].get().read().assume_init() })
    }

    pub fn stealer(&self) -> Stealer<T, N> {
        Stealer(self.0.clone())
    }
}

impl<T, const N: usize> Default for Local<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Stealer<T, N> {
    const MASK: usize = Inner::<T, N>::MASK;

    pub fn len(&self) -> usize {
        self.0.len() as _
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn steals_from(&self, queue: &Local<T, N>) -> bool {
        Arsc::ptr_eq(&self.0, &queue.0)
    }

    pub fn steal_into_and_pop(&self, dst: &mut Local<T, N>) -> Option<T> {
        let dst_tail = dst.0.tail.load(Relaxed);

        let (steal, _) = unpack(dst.0.head.load(Acquire));
        if dst_tail.wrapping_sub(steal) > N as u32 / 2 {
            return None;
        }

        let mut n = self.steal_n(dst, dst_tail);
        if n == 0 {
            return None;
        }
        n -= 1;
        let index = dst_tail.wrapping_add(n) as usize & Self::MASK;
        // SAFETY: Successful CAS assumed ownership of the value.
        let value = unsafe { dst.0.buffer[index].get().read().assume_init() };

        if n != 0 {
            dst.0.tail.store(dst_tail.wrapping_add(n), Release);
        }

        Some(value)
    }

    pub fn steal_into(&self, dst: &mut Local<T, N>) {
        let dst_tail = dst.0.tail.load(Relaxed);

        let (steal, _) = unpack(dst.0.head.load(Acquire));
        if dst_tail.wrapping_sub(steal) > N as u32 / 2 {
            return;
        }

        let n = self.steal_n(dst, dst_tail);
        if n != 0 {
            dst.0.tail.store(dst_tail.wrapping_add(n), Release);
        }
    }

    fn steal_n(&self, dst: &mut Local<T, N>, dst_tail: u32) -> u32 {
        let mut head = self.0.head.load(Acquire);
        let mut next;

        let n = loop {
            let (steal, real) = unpack(head);
            let tail = self.0.tail.load(Acquire);

            if steal != real {
                return 0;
            }

            let n = tail.wrapping_sub(real);
            let n = n - n / 2;
            if n == 0 {
                return 0;
            }

            let steal_to = real.wrapping_add(n);
            assert_ne!(steal, steal_to);
            next = pack(steal, steal_to);

            match self.0.head.compare_exchange(head, next, AcqRel, Acquire) {
                Ok(_) => break n,
                Err(h) => head = h,
            }
        };

        assert!(n <= N as u32 / 2, "too much ({n}) to steal");

        let (first, _) = unpack(next);
        for i in 0..n {
            let src_index = first.wrapping_add(i) as usize & Self::MASK;
            let dst_index = dst_tail.wrapping_add(i) as usize & Self::MASK;

            // SAFETY: Successful CAS assumed ownership of these values.
            unsafe {
                let value = self.0.buffer[src_index].get().read();
                dst.0.buffer[dst_index].get().write(value)
            }
        }

        let mut head = next;
        loop {
            let real = unpack(head).1;
            next = pack(real, real);

            match self.0.head.compare_exchange(head, next, AcqRel, Acquire) {
                Ok(_) => break n,
                Err(h) => {
                    let (s, r) = unpack(h);
                    assert_ne!(s, r);
                    head = h;
                }
            }
        }
    }
}

fn unpack(num: u64) -> (u32, u32) {
    let lower = num & u32::MAX as u64;
    let upper = num >> u32::BITS;
    (lower as u32, upper as u32)
}

fn pack(lower: u32, upper: u32) -> u64 {
    (lower as u64) | ((upper as u64) << u32::BITS)
}
