use alloc::collections::BTreeMap;
use core::{
    mem,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll, Waker},
    time::Duration,
};

use futures_lite::{Future, Stream};
use heapless::mpmc::MpMcQueue;
use ktime_core::Instant;
use spin::{Mutex, MutexGuard};

/// Async timer, based on [`async-io`]'s implementation.
///
/// [`async-io`]: https://doc.rs/async-io/latest/async_io/struct.Timer.html
#[derive(Debug)]
pub struct Timer {
    deadline: Instant,
    done: bool,
    period: Duration,
    handle: Option<(usize, Waker)>,
}

impl Timer {
    pub fn deadline(deadline: Instant) -> Self {
        Timer {
            deadline,
            done: false,
            period: Duration::MAX,
            handle: None,
        }
    }

    pub fn after(duration: Duration) -> Self {
        Timer::deadline(Instant::now() + duration)
    }

    pub fn period(period: Duration) -> Self {
        Timer {
            deadline: Instant::now() + period,
            done: false,
            period,
            handle: None,
        }
    }

    pub fn set_deadline(&mut self, deadline: Instant) {
        self.clear();

        self.deadline = deadline;
        self.done = false;
        self.period = Duration::MAX;
    }

    pub fn set_after(&mut self, duration: Duration) {
        self.set_deadline(Instant::now() + duration);
    }

    pub fn set_period(&mut self, period: Duration) {
        self.clear();

        self.deadline = Instant::now() + period;
        self.done = false;
        self.period = period;
    }

    fn clear(&mut self) {
        if let Some((id, _)) = self.handle.take() {
            TIMER_QUEUE.remove(self.deadline, id)
        }
    }
}

impl Future for Timer {
    type Output = Instant;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.poll_next(cx) {
            Poll::Ready(Some(item)) => Poll::Ready(item),
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => unreachable!(),
        }
    }
}

impl Stream for Timer {
    type Item = Instant;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        fn register(deadline: Instant, cx: &mut Context<'_>) -> (usize, Waker) {
            let waker = cx.waker().clone();
            let id = TIMER_QUEUE.insert(deadline, waker.clone());
            (id, waker)
        }

        if self.done {
            return Poll::Pending;
        }
        let now = Instant::now();
        if now >= self.deadline {
            if let Some((id, _)) = self.handle.take() {
                TIMER_QUEUE.remove(self.deadline, id);
            }
            if let Some(new) = self.deadline.checked_add(self.period) {
                self.deadline = new;

                self.handle = Some(register(self.deadline, cx));
            } else {
                self.done = true;
            }
            return Poll::Ready(Some(now));
        }
        match self.handle {
            Some((id, ref mut waker)) => {
                if !waker.will_wake(cx.waker()) {
                    TIMER_QUEUE.remove(self.deadline, id);
                    self.handle = Some(register(self.deadline, cx));
                }
            }
            None => self.handle = Some(register(self.deadline, cx)),
        }
        Poll::Pending
    }
}

enum TimerBatch {
    Insert {
        deadline: Instant,
        id: usize,
        waker: Waker,
    },
    Remove {
        deadline: Instant,
        id: usize,
    },
}

pub static TIMER_QUEUE: TimerQueue = TimerQueue {
    heap: Mutex::new(BTreeMap::new()),
    pending: MpMcQueue::new(),
};

const PENDING_CAP: usize = 128;

pub struct TimerQueue {
    heap: Mutex<BTreeMap<(Instant, usize), Waker>>,
    pending: MpMcQueue<TimerBatch, PENDING_CAP>,
}

impl TimerQueue {
    fn insert(&self, deadline: Instant, waker: Waker) -> usize {
        static ID: AtomicUsize = AtomicUsize::new(1);
        let id = ID.fetch_add(1, core::sync::atomic::Ordering::SeqCst);

        let batch = TimerBatch::Insert {
            deadline,
            id,
            waker,
        };
        if let Err(batch) = self.pending.enqueue(batch) {
            ksync_core::critical(|| {
                let mut heap = self.heap.lock();
                self.proc_batch(Some(batch), &mut heap);
            });
        }
        id
    }

    fn remove(&self, deadline: Instant, id: usize) {
        let batch = TimerBatch::Remove { deadline, id };
        if let Err(batch) = self.pending.enqueue(batch) {
            ksync_core::critical(|| {
                let mut heap = self.heap.lock();
                self.proc_batch(Some(batch), &mut heap);
            });
        }
    }

    fn proc_batch(
        &self,
        more: Option<TimerBatch>,
        heap: &mut MutexGuard<BTreeMap<(Instant, usize), Waker>>,
    ) {
        if let Some(batch) = more {
            match batch {
                TimerBatch::Insert {
                    deadline,
                    id,
                    waker,
                } => heap.insert((deadline, id), waker),
                TimerBatch::Remove { deadline, id } => heap.remove(&(deadline, id)),
            };
        }
        for _ in 0..PENDING_CAP {
            let batch = match self.pending.dequeue() {
                Some(data) => data,
                None => break,
            };
            match batch {
                TimerBatch::Insert {
                    deadline,
                    id,
                    waker,
                } => heap.insert((deadline, id), waker),
                TimerBatch::Remove { deadline, id } => heap.remove(&(deadline, id)),
            };
        }
    }

    pub fn tick(&self) {
        let wakers = ksync_core::critical(|| {
            let mut heap = self.heap.lock();
            self.proc_batch(None, &mut heap);

            let now = Instant::now();

            let pending = heap.split_off(&(now + Duration::from_micros(1), 0));

            mem::replace(&mut *heap, pending)
        });
        wakers.into_values().for_each(|w| w.wake())
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::{sync::mpsc, thread};

    use futures_lite::StreamExt;
    use ktime_core::Instant;

    use crate::{timer_tick, Timer};

    #[test]
    fn test_timer() {
        let (tx, rx) = mpsc::channel();
        let notify = thread::spawn(move || loop {
            let try_recv = rx.try_recv();
            if try_recv.is_ok() {
                break;
            }
            timer_tick()
        });
        smol::block_on(async {
            let start = Instant::now();

            let dur = Duration::from_millis(10);
            let mut timer = Timer::after(dur);

            let delta = timer.next().await.unwrap() - (start + dur);
            assert!(delta < Duration::from_millis(1));

            let deadline = start + dur * 2;
            timer.set_deadline(deadline);

            let delta = timer.next().await.unwrap() - deadline;
            assert!(delta < Duration::from_millis(1));

            let start = Instant::now();
            crate::sleep(dur).await;
            assert!(start.elapsed() >= dur);
        });

        tx.send(()).unwrap();
        notify.join().unwrap();
    }
}
