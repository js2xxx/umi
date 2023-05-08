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
use async_task::{Runnable, ScheduleInfo, Task, WithInfo};
use crossbeam_queue::SegQueue;
use rand_riscv::{rand_core::RngCore, Rng};
use scoped_tls::scoped_thread_local;

use crate::queue::{Local, Stealer};

const WORKER_CAP: usize = 64;
const WORKER_TICK_INTERVAL: u32 = 17;

struct Worker {
    rq: Local<Runnable, WORKER_CAP>,
    preempt_slot: Option<Runnable>,
}

pub(crate) struct Context {
    worker: RefCell<Worker>,
    executor: Arsc<Executor>,
}

pub struct Executor {
    injector: SegQueue<Runnable>,
    stealers: Box<[Stealer<Runnable, WORKER_CAP>]>,
    shutdown: AtomicBool,
}

scoped_thread_local!(pub(crate) static CX: Context);

impl Executor {
    /// Create a new executor with `num` runners and a `init` future.
    ///
    /// The caller should iterate over the returned startup functions and run
    /// them concurrently.
    pub fn start<G, F>(num: usize, init: G) -> impl Iterator<Item = impl FnOnce() + Send>
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

        workers.into_iter().map(move |worker| {
            let e = executor.clone();
            || Self::startup(worker, e)
        })
    }

    pub fn spawn<F, T>(&self, fut: F) -> Task<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (task, handle) = async_task::spawn(fut, WithInfo(Context::enqueue));
        task.schedule();
        handle
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Release)
    }

    fn startup(rq: Local<Runnable, WORKER_CAP>, executor: Arsc<Executor>) {
        let cx = Context {
            worker: RefCell::new(Worker {
                rq,
                preempt_slot: None,
            }),
            executor,
        };
        CX.set(&cx, || cx.run())
    }
}

impl Worker {
    #[inline]
    fn pop(&mut self) -> Option<Runnable> {
        self.preempt_slot.take().or_else(|| self.rq.pop())
    }

    fn push(&mut self, task: Runnable, injector: &SegQueue<Runnable>, yielded: bool) {
        if !yielded {
            if let Some(last) = self.preempt_slot.replace(task) {
                self.rq.push(last, |task| injector.push(task))
            }
        } else {
            self.rq.push(task, |task| injector.push(task))
        }
    }
}

impl Context {
    fn next_task(&self, tick: u32, worker: &mut Worker) -> Option<Runnable> {
        if tick % WORKER_TICK_INTERVAL == 0 {
            self.executor.injector.pop().or_else(|| worker.pop())
        } else {
            worker.pop().or_else(|| self.executor.injector.pop())
        }
    }

    fn steal_task(&self, rand: &mut Rng, worker: &mut Worker) -> Option<Runnable> {
        let stealers = &self.executor.stealers;

        let len = stealers.len();
        let offset = (rand.next_u64() % len as u64) as usize;

        let mut iter = stealers.iter().cycle().skip(offset).take(len);
        let task = iter.find_map(|stealer| stealer.steal_into_and_pop(&mut worker.rq));
        task.or_else(|| self.executor.injector.pop())
    }

    fn run(&self) {
        let mut tick = 0u32;
        let mut rng = rand_riscv::rng();
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

            hint::spin_loop();
        }
    }

    fn enqueue(task: Runnable, sched_info: ScheduleInfo) {
        let ret = CX.try_with(|cx| {
            if let Ok(mut worker) = cx.worker.try_borrow_mut() {
                worker.push(task, &cx.executor.injector, sched_info.woken_while_running);
            } else {
                cx.executor.injector.push(task)
            }
        });
        if ret.is_none() {
            log::warn!("executor exited while scheduling");
        }
    }
}
