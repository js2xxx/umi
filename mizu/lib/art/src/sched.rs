use core::{
    future::Future,
    hint,
    sync::atomic::{AtomicUsize, Ordering},
};

use array_macro::array;
use arsc_rs::Arsc;
use async_task::{FallibleTask, Runnable};
use config::MAX_HARTS;
use crossbeam_queue::SegQueue;
use heapless::mpmc::MpMcQueue;

use crate::task::SchedInfo;

static BACKUP: SegQueue<RunTask> = SegQueue::new();
pub static SCHED: [Scheduler; MAX_HARTS] = array![
    i => Scheduler {
        cpu: i,
        queue: MpMcQueue::new(),
        count: AtomicUsize::new(0),
    };
    MAX_HARTS
];

struct RunTask {
    inner: Runnable,
    info: Arsc<SchedInfo>,
}

pub struct Scheduler {
    cpu: usize,
    queue: MpMcQueue<RunTask, 128>,
    count: AtomicUsize,
}

impl Scheduler {
    pub fn run(&'static self) -> ! {
        loop {
            let task = self.queue.dequeue().or_else(|| BACKUP.pop());
            match task {
                Some(task) => {
                    task.info.last_cpu.store(self.cpu, Ordering::Relaxed);
                    task.inner.run();

                    self.count.fetch_sub(1, Ordering::AcqRel);
                }
                None => hint::spin_loop(),
            }
        }
    }

    pub fn spawn<F>(&self, task: F, info: Arsc<SchedInfo>) -> FallibleTask<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let schedule = move |task| Self::enqueue(task, info.clone());
        let (task, handle) = async_task::spawn(task, schedule);
        task.schedule();
        handle.fallible()
    }

    fn enqueue(inner: Runnable, info: Arsc<SchedInfo>) {
        let nr_harts = unsafe { crate::NR_HARTS };
        let result: Option<(&Scheduler, usize)> =
            SCHED.iter().take(nr_harts).fold(None, |out, sched| {
                let count = sched.count.load(Ordering::Acquire);
                Some(if let Some((s, c)) = out {
                    let minimize_count = c < count;
                    // Relaxed because it cannot be spawned while running.
                    let stick_to_last = s.cpu == info.last_cpu.load(Ordering::Relaxed);
                    if minimize_count || stick_to_last {
                        (s, c)
                    } else {
                        (sched, count)
                    }
                } else {
                    (sched, count)
                })
            });
        if let Some((sched, _)) = result {
            if let Err(next) = sched.queue.enqueue(RunTask { inner, info }) {
                BACKUP.push(next)
            }
        }
    }
}
