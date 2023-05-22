use core::{
    mem,
    pin::Pin,
    ptr,
    sync::atomic::{AtomicIsize, AtomicPtr, AtomicUsize, Ordering::SeqCst},
    task::{Context, Poll, Waker},
};

use arsc_rs::Arsc;
use futures_util::Future;
use hashbrown::{hash_map::Entry, HashMap};
use rand_riscv::RandomState;
use spin::Mutex;

use super::{user::FutexKey, InOut, UserPtr};

#[derive(Debug, Clone)]
enum FutexState {
    Waiting(Waker),
    Notified,
    Requed(Pin<Arsc<FutexQueue>>),
}
static ID_ALLOC: AtomicUsize = AtomicUsize::new(1);

#[derive(Debug)]
struct FutexQueue {
    key: FutexKey,
    prewoken: AtomicIsize,
    wakers: Mutex<HashMap<usize, FutexState, RandomState>>,
}

impl FutexQueue {
    fn new(key: FutexKey) -> Pin<Arsc<Self>> {
        Arsc::pin(FutexQueue {
            key,
            prewoken: AtomicIsize::new(0),
            wakers: Default::default(),
        })
    }

    fn poll(&self, id: &mut Option<usize>, waker: &Waker) -> Result<Poll<()>, Pin<Arsc<Self>>> {
        let id = *id.get_or_insert_with(|| ID_ALLOC.fetch_add(1, SeqCst));
        let ret = ksync::critical(|| {
            let mut wakers = self.wakers.lock();
            match wakers.entry(id) {
                Entry::Occupied(mut ent) => match ent.get_mut() {
                    FutexState::Waiting(w) => {
                        if !w.will_wake(waker) {
                            *w = waker.clone();
                        }
                        Ok(Poll::Pending)
                    }
                    FutexState::Notified => {
                        ent.remove();
                        Ok(Poll::Ready(()))
                    }
                    FutexState::Requed(_) => {
                        let new = ent.remove();
                        let FutexState::Requed(new) = new else {
                            unreachable!()
                        };
                        Err(new)
                    }
                },
                Entry::Vacant(ent) => {
                    if self.prewoken.fetch_sub(1, SeqCst) >= 0 {
                        return Ok(Poll::Ready(()));
                    }
                    ent.insert(FutexState::Waiting(waker.clone()));
                    Ok(Poll::Pending)
                }
            }
        });
        log::trace!("Futex: polling waker {id}");
        ret
    }

    fn discard(&self, id: usize) -> Option<Pin<Arsc<Self>>> {
        match ksync::critical(|| self.wakers.lock().remove(&id)) {
            Some(FutexState::Requed(new)) => Some(new),
            _ => None,
        }
    }

    fn wake(&self, n: usize) -> usize {
        let count = ksync::critical(|| {
            let mut wakers = self.wakers.lock();
            let mut count = 0;
            for (id, state) in wakers.iter_mut() {
                if let FutexState::Waiting(w) = mem::replace(state, FutexState::Notified) {
                    log::trace!("Futex: wake waker {id}");
                    w.wake();
                    count += 1;

                    if count == n {
                        break;
                    }
                }
            }
            count
        });
        self.prewoken.fetch_add(count as isize, SeqCst);
        count
    }

    fn requeue(self: Pin<&Self>, other: Pin<Arsc<Self>>, notify: usize, reque: usize) -> usize {
        if self.key == other.key {
            return 0;
        }

        let order = {
            let this = self.as_ref().get_ref() as *const _;
            let dst = other.as_ref().get_ref() as *const _;
            this < dst
        };

        ksync::critical(|| {
            let (mut src, mut dst) = if order {
                let src = self.wakers.lock();
                let dst = other.wakers.lock();
                (src, dst)
            } else {
                let other = other.wakers.lock();
                let this = self.wakers.lock();
                (this, other)
            };

            let mut notified = 0;
            for state in src.values_mut() {
                if notified == notify {
                    break;
                }
                if let FutexState::Waiting(w) = mem::replace(state, FutexState::Notified) {
                    w.wake();
                    notified += 1;
                }
            }

            let mut requed = 0;
            src.retain(|&id, state| {
                if requed >= reque {
                    return false;
                }
                if let FutexState::Waiting(w) = mem::replace(state, FutexState::Notified) {
                    dst.insert(id, FutexState::Waiting(w));
                    *state = FutexState::Requed(other.clone());
                    requed += 1;
                    return true;
                }
                false
            });
            self.prewoken.fetch_add(notified as isize, SeqCst);

            notified + requed
        })
    }

    fn deep_fork(&self) -> Pin<Arsc<Self>> {
        let wakers = ksync::critical(|| {
            let wakers = self.wakers.lock();
            let iter = wakers.iter().map(|(id, state)| (*id, state.clone()));
            iter.collect()
        });
        Arsc::pin(FutexQueue {
            key: self.key,
            prewoken: AtomicIsize::new(self.prewoken.load(SeqCst)),
            wakers: Mutex::new(wakers),
        })
    }
}

impl Drop for FutexQueue {
    fn drop(&mut self) {
        let wakers = mem::take(self.wakers.get_mut());
        wakers.into_values().for_each(|state| {
            if let FutexState::Waiting(waker) = state {
                waker.wake()
            }
        })
    }
}

#[derive(Debug, Default)]
pub struct Futexes {
    map: Mutex<HashMap<FutexKey, Pin<Arsc<FutexQueue>>, RandomState>>,
    robust_list: AtomicPtr<RobustListHead>,
}

impl Futexes {
    fn queue(&self, key: FutexKey) -> Pin<Arsc<FutexQueue>> {
        ksync::critical(|| match self.map.lock().entry(key) {
            Entry::Occupied(ent) => ent.get().clone(),
            Entry::Vacant(ent) => ent.insert(FutexQueue::new(key)).clone(),
        })
    }

    pub fn new() -> Self {
        Futexes {
            map: Default::default(),
            robust_list: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub fn notify(&self, key: FutexKey, n: usize) -> usize {
        self.queue(key).wake(n)
    }

    pub fn wait(&self, key: FutexKey) -> FutexWait {
        FutexWait {
            queue: self.queue(key),
            id: None,
        }
    }

    pub fn robust_list(&self) -> UserPtr<RobustListHead, InOut> {
        UserPtr::new(self.robust_list.load(SeqCst).into())
    }

    pub fn set_robust_list(&self, ptr: UserPtr<RobustListHead, InOut>) {
        self.robust_list.store(ptr.addr().cast(), SeqCst)
    }

    pub fn requeue(&self, from: FutexKey, to: FutexKey, notify: usize, reque: usize) -> usize {
        let from = self.queue(from);
        from.as_ref().requeue(self.queue(to), notify, reque)
    }

    pub fn deep_fork(&self) -> Self {
        Futexes {
            map: Mutex::new(ksync::critical(|| {
                let queue = self.map.lock();
                let iter = queue.iter().map(|(key, queue)| (*key, queue.deep_fork()));
                iter.collect()
            })),
            robust_list: AtomicPtr::new(self.robust_list.load(SeqCst)),
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct FutexWait {
    queue: Pin<Arsc<FutexQueue>>,
    id: Option<usize>,
}

impl Unpin for FutexWait {}

impl Future for FutexWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        loop {
            match this.queue.poll(&mut this.id, cx.waker()) {
                Ok(poll) => break poll,
                Err(queue) => this.queue = queue,
            }
        }
    }
}

impl Drop for FutexWait {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            while let Some(new) = self.queue.discard(id) {
                self.queue = new;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct RobustListHead {
    list: usize,
    futex_offset: usize,
    list_op_pending: usize,
}
