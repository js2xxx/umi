//! Work-stealing async executor, based on [`Tokio`]'s implementation.
//!
//! [`Tokio`]: https://tokio.rs/

use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::RefCell,
    future::Future,
    hint,
    sync::atomic::{
        AtomicBool,
        Ordering::{Acquire, Release},
    },
};

use arsc_rs::Arsc;
use async_task::{Runnable, Task};
use crossbeam_queue::SegQueue;
use rand_chacha::{
    rand_core::{RngCore, SeedableRng},
    ChaChaRng,
};
use scoped_tls::scoped_thread_local;

use crate::queue::{Local, Stealer};

const WORKER_CAP: usize = 64;
const WORKER_TICK_INTERVAL: u32 = 17;

struct Context {
    worker: RefCell<Local<Runnable, WORKER_CAP>>,
    executor: Arsc<Executor>,
}

pub struct Executor {
    injector: SegQueue<Runnable>,
    stealers: Box<[Stealer<Runnable, WORKER_CAP>]>,
    shutdown: AtomicBool,
}

scoped_thread_local!(static CX: Context);

impl Executor {
    /// Create a new executor with `num` runners and a `init` future.
    ///
    /// The caller should iterate over the returned startup functions and run
    /// them concurrently.
    pub fn new<G, F>(num: usize, init: G) -> (Arsc<Self>, impl Iterator<Item = impl FnOnce()>)
    where
        G: FnOnce(Arsc<Executor>) -> F,
        F: Future<Output = ()> + Send + 'static,
    {
        let workers = (0..num).map(|_| Local::new()).collect::<Vec<_>>();

        let stealers = workers
            .iter()
            .map(|w| w.stealer())
            .collect::<Vec<_>>()
            .into_boxed_slice();

        let executor = Arsc::new(Executor {
            injector: SegQueue::new(),
            stealers,
            shutdown: AtomicBool::new(false),
        });

        let e2 = executor.clone();
        let schedule = move |task| e2.injector.push(task);
        let (init, handle) = async_task::spawn(init(executor.clone()), schedule);
        init.schedule();
        handle.detach();

        let e2 = executor.clone();
        let startup = workers.into_iter().map(move |worker| {
            let e = e2.clone();
            || Self::startup(worker, e)
        });
        (executor, startup)
    }

    pub fn spawn<F, T>(&self, fut: F) -> Task<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (task, handle) = async_task::spawn(fut, Context::enqueue);
        task.schedule();
        handle
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Release)
    }

    fn startup(worker: Local<Runnable, WORKER_CAP>, executor: Arsc<Executor>) {
        let cx = Context {
            worker: RefCell::new(worker),
            executor,
        };
        CX.set(&cx, || cx.run())
    }
}

impl Context {
    fn next_task(&self, tick: u32, worker: &mut Local<Runnable, WORKER_CAP>) -> Option<Runnable> {
        if tick % WORKER_TICK_INTERVAL == 0 {
            self.executor.injector.pop().or_else(|| worker.pop())
        } else {
            worker.pop().or_else(|| self.executor.injector.pop())
        }
    }

    fn steal_task(
        &self,
        rand: &mut ChaChaRng,
        worker: &mut Local<Runnable, WORKER_CAP>,
    ) -> Option<Runnable> {
        let stealers = &self.executor.stealers;

        let len = stealers.len();
        let offset = (rand.next_u64().wrapping_mul(len as u64) >> 32) as usize;

        let mut iter = stealers.iter().cycle().skip(offset).take(len);
        let task = iter.find_map(|stealer| stealer.steal_into_and_pop(worker));
        task.or_else(|| self.executor.injector.pop())
    }

    fn run(&self) {
        let mut tick = 0u32;
        let mut rng = ChaChaRng::from_seed({
            let mut s = [0; 32];
            crate::rand::seed(&mut s);
            s
        });
        loop {
            tick = tick.wrapping_add(1);

            if self.executor.shutdown.load(Acquire) {
                break;
            }

            let next = self.next_task(tick, &mut self.worker.borrow_mut());
            if let Some(task) = next {
                task.run();
                continue;
            }

            let stealed = self.steal_task(&mut rng, &mut self.worker.borrow_mut());
            if let Some(task) = stealed {
                task.run();
                continue;
            }

            hint::spin_loop()
        }
    }

    fn enqueue(task: Runnable) {
        CX.with(|cx| {
            if let Ok(mut worker) = cx.worker.try_borrow_mut() {
                worker.push(task, |task| cx.executor.injector.push(task));
            } else {
                cx.executor.injector.push(task)
            }
        })
    }
}
