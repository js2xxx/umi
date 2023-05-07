pub mod elf;
mod future;

use alloc::sync::{Arc, Weak};
use core::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use co_trap::TrapFrame;
use hashbrown::HashMap;
use kmem::Virt;
use rand_riscv::RandomState;
use spin::{Lazy, Mutex};
use sygnal::{ActionSet, SigSet, Signals};

use crate::{
    executor,
    task::future::{user_loop, TaskFut},
};

pub struct TaskState {
    task: Arc<Task>,
    sig_mask: SigSet,
}

pub struct Task {
    main: Weak<Task>,
    tid: usize,
    virt: Pin<Arsc<Virt>>,

    sig: Signals,
    sig_actions: ActionSet,
}

impl Task {
    pub fn main(&self) -> Option<Arc<Task>> {
        self.main.upgrade()
    }

    pub fn tid(&self) -> usize {
        self.tid
    }
}

static TASKS: Lazy<Mutex<HashMap<usize, Arc<Task>, RandomState>>> =
    Lazy::new(|| Mutex::new(HashMap::with_hasher(RandomState::new())));

pub fn task(id: usize) -> Option<Arc<Task>> {
    ksync::critical(|| TASKS.lock().get(&id).cloned())
}

pub fn process(id: usize) -> Option<Arc<Task>> {
    let task = self::task(id)?;
    Some(match task.main.upgrade() {
        Some(main) => main,
        None => task,
    })
}

pub struct InitTask {
    main: Weak<Task>,
    virt: Pin<Arsc<Virt>>,
    tf: TrapFrame,
}

impl InitTask {
    pub fn spawn(self) -> Result<Arc<Task>, ksc::Error> {
        static TID: AtomicUsize = AtomicUsize::new(0);

        let tid = TID.fetch_add(1, SeqCst);
        let task = Arc::new(Task {
            main: self.main,
            tid,
            virt: self.virt,

            sig: Signals::new(),
            sig_actions: ActionSet::new(),
        });

        let ts = TaskState {
            task: task.clone(),
            sig_mask: SigSet::EMPTY,
        };

        let fut = TaskFut::new(task.virt.clone(), user_loop(ts, self.tf));
        executor().spawn(fut).detach();
        ksync::critical(|| TASKS.lock().insert(tid, task.clone()));

        Ok(task)
    }
}
