mod elf;
pub mod fd;
mod future;
mod init;

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};
use core::{
    ops::ControlFlow::{Break, Continue},
    pin::Pin,
};

use arsc_rs::Arsc;
use co_trap::UserCx;
use hashbrown::HashMap;
use kmem::Virt;
use ksc::{async_handler, Error};
use ksync::Broadcast;
use rand_riscv::RandomState;
use rv39_paging::{Attr, PAGE_SIZE};
use spin::{Lazy, Mutex};
use sygnal::{ActionSet, Sig, SigSet, Signals};

use self::fd::Files;
pub use self::{future::yield_now, init::InitTask};
use crate::{
    mem::{Out, UserPtr},
    syscall::ScRet,
};

const DEFAULT_STACK_SIZE: usize = PAGE_SIZE * 32;
const DEFAULT_STACK_ATTR: Attr = Attr::USER_ACCESS
    .union(Attr::READABLE)
    .union(Attr::WRITABLE);

pub struct TaskState {
    pub(crate) task: Arc<Task>,
    sig_mask: SigSet,
    pub(crate) brk: usize,

    system_times: u64,
    user_times: u64,
}

#[derive(Clone)]
pub enum TaskEvent {
    Exited(i32),
    Signaled(Sig),
    Suspended(Sig),
    Continued,
}

pub struct Task {
    main: Weak<Task>,
    parent: Weak<Task>,
    tid: usize,
    virt: Pin<Arsc<Virt>>,

    sig: Signals,
    sig_actions: ActionSet,
    event: Broadcast<TaskEvent>,
    files: Arsc<Files>,
}

impl Task {
    pub fn main(&self) -> Option<Arc<Task>> {
        self.main.upgrade()
    }

    pub fn tid(&self) -> usize {
        self.tid
    }

    pub fn event(&self) -> Broadcast<TaskEvent> {
        self.event.clone()
    }

    pub fn virt(&self) -> Pin<&Virt> {
        self.virt.as_ref()
    }

    pub async fn wait(&self) -> i32 {
        let event = self.event();
        loop {
            if let Ok(TaskEvent::Exited(code)) = event.recv().await {
                break code;
            }
        }
    }
}

static TASKS: Lazy<Mutex<HashMap<usize, Arc<Task>, RandomState>>> =
    Lazy::new(|| Mutex::new(HashMap::with_hasher(RandomState::new())));

pub fn task(id: usize) -> Option<Arc<Task>> {
    ksync::critical(|| TASKS.lock().get(&id).cloned())
}

pub fn task_event(id: usize) -> Option<Broadcast<TaskEvent>> {
    ksync::critical(|| TASKS.lock().get(&id).map(|t| t.event.clone()))
}

pub fn process(id: usize) -> Option<Arc<Task>> {
    let task = self::task(id)?;
    Some(match task.main.upgrade() {
        Some(main) => main,
        None => task,
    })
}

#[async_handler]
pub async fn pid(ts: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    let task = &ts.task;
    cx.ret(task.main.upgrade().map_or(task.tid, |main| main.tid));
    Continue(None)
}

#[async_handler]
pub async fn ppid(ts: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    let task = &ts.task;
    cx.ret(task.parent.upgrade().map_or(1, |parent| parent.tid));
    Continue(None)
}

#[async_handler]
pub async fn times(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u64, Out>) -> Result<(), Error>>,
) -> ScRet {
    let mut out = cx.args();
    let data = [ts.user_times, ts.system_times, 0, 0];
    cx.ret(out.write_slice(ts.task.virt(), &data, false).await);
    Continue(None)
}

#[async_handler]
pub async fn exit(ts: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
    let _ = ts.task.files.flush_all().await;
    Break(cx.args())
}
