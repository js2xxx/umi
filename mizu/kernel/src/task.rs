mod elf;
pub mod fd;
mod future;
mod init;

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    ops::ControlFlow::{Break, Continue},
    pin::Pin,
};

use arsc_rs::Arsc;
use co_trap::{TrapFrame, UserCx};
use futures_util::future::select_all;
use hashbrown::HashMap;
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, ECHILD, EINVAL},
};
use ksync::Broadcast;
use rand_riscv::RandomState;
use rv39_paging::{Attr, PAGE_SIZE};
use spin::{Lazy, Mutex};
use sygnal::{ActionSet, Sig, SigInfo, SigSet, Signals};

use self::fd::Files;
pub use self::{future::yield_now, init::InitTask};
use crate::{
    executor,
    mem::{Out, UserPtr},
    syscall::ScRet,
    task::future::{user_loop, TaskFut},
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

    pub(crate) virt: Pin<Arsc<Virt>>,
    sig_actions: Arsc<ActionSet>,
    files: Files,
    tid_clear: Option<UserPtr<usize, Out>>,
    exit_signal: Option<Sig>,
}

#[derive(Clone, Copy, Debug)]
pub enum TaskEvent {
    Exited(i32, Option<Sig>),
    Suspended(Sig),
    Continued,
}

pub struct Task {
    main: Weak<Task>,
    parent: Weak<Task>,
    children: spin::Mutex<Vec<Arc<Task>>>,
    tid: usize,

    sig: Signals,
    event: Broadcast<TaskEvent>,
}

impl Task {
    pub fn event(&self) -> Broadcast<TaskEvent> {
        self.event.clone()
    }

    pub async fn wait(&self) -> i32 {
        let event = self.event();
        loop {
            if let Ok(TaskEvent::Exited(code, _)) = event.recv().await {
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
    cx.ret(out.write_slice(ts.virt.as_ref(), &data, false).await);
    Continue(None)
}

#[async_handler]
pub async fn exit(ts: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
    let code = cx.args();

    if let Some(mut tid_clear) = ts.tid_clear.take() {
        let _ = tid_clear.write(ts.virt.as_ref(), 0).await;
    }

    let take = ts.exit_signal.take();
    if let (Some(sig), Some(parent)) = (take, ts.task.parent.upgrade()) {
        parent.sig.push(SigInfo {
            sig,
            code: sygnal::SigCode::USER,
            fields: sygnal::SigFields::SigChld {
                pid: ts.task.tid,
                uid: 0,
                status: code,
            },
        })
    }

    if ts.task.main.upgrade().is_none() {
        ts.virt.clear().await;
    }

    ksync::critical(|| TASKS.lock().remove(&ts.task.tid));
    let _ = ts.files.flush_all().await;

    Break((code, take))
}

async fn clone_task(
    ts: &mut TaskState,
    tf: &TrapFrame,
    flags: u64,
    stack: usize,
    mut ptid: UserPtr<usize, Out>,
    tls: usize,
    mut ctid: UserPtr<usize, Out>,
) -> Result<usize, Error> {
    bitflags::bitflags! {
        #[derive(Debug, Copy, Clone)]
        struct Flags: u64 {
            const CSIGNAL        = 0x000000ff;
            /// Share virt.
            const VM             = 0x00000100;
            /// Share cwd.
            const FS             = 0x00000200;
            /// Share fd.
            const FILES          = 0x00000400;
            /// Share sigaction.
            const SIGHAND        = 0x00000800;
            /// Share parent.
            const PARENT         = 0x00008000;
            /// Set TLS.
            const SETTLS         = 0x00080000;

            const PARENT_SETTID  = 0x00100000;
            const CHILD_CLEARTID = 0x00200000;
            const CHILD_SETTID   = 0x01000000;
        }
    }
    let flags = Flags::from_bits_truncate(flags);
    let bits = (flags & Flags::CSIGNAL).bits();
    let exit_signal = if bits == 0 {
        None
    } else {
        Some(Sig::new(bits as i32).ok_or(EINVAL)?)
    };

    log::trace!("clone_task: flags = {flags:?}");

    let new_tid = init::alloc_tid();
    log::trace!("new tid = {new_tid}");
    let task = Task {
        main: if flags.contains(Flags::VM) {
            Arc::downgrade(&ts.task)
        } else {
            Weak::new()
        },
        parent: if flags.contains(Flags::PARENT) {
            ts.task.parent.clone()
        } else {
            Arc::downgrade(&ts.task)
        },
        children: spin::Mutex::new(Vec::new()),
        tid: new_tid,
        sig: Signals::new(),
        event: Broadcast::new(),
    };
    if flags.contains(Flags::PARENT_SETTID) {
        ptid.write(ts.virt.as_ref(), new_tid).await?;
    }
    if flags.contains(Flags::CHILD_SETTID) {
        ctid.write(ts.virt.as_ref(), new_tid).await?;
    }

    let mut new_tf = *tf;

    new_tf.set_syscall_ret(0);
    new_tf.gpr.tx.sp = stack;
    if flags.contains(Flags::SETTLS) {
        new_tf.gpr.tx.tp = tls;
    }

    let task = Arc::new(task);
    let new_ts = TaskState {
        task: task.clone(),
        sig_mask: SigSet::EMPTY,
        brk: ts.brk,
        system_times: 0,
        user_times: 0,
        virt: if flags.contains(Flags::VM) {
            ts.virt.clone()
        } else {
            ts.virt.as_ref().deep_fork().await?
        },
        files: ts
            .files
            .deep_fork(flags.contains(Flags::FS), flags.contains(Flags::FILES))
            .await,
        sig_actions: if flags.contains(Flags::SIGHAND) {
            ts.sig_actions.clone()
        } else {
            Arsc::new(ts.sig_actions.deep_fork())
        },
        tid_clear: flags.contains(Flags::CHILD_CLEARTID).then_some(ctid),
        exit_signal,
    };

    if let Some(parent) = new_ts.task.parent.upgrade() {
        ksync::critical(|| parent.children.lock().push(new_ts.task.clone()))
    }

    let fut = TaskFut::new(new_ts.virt.clone(), user_loop(new_ts, new_tf));
    executor().spawn(fut).detach();
    ksync::critical(|| TASKS.lock().insert(new_tid, task.clone()));

    Ok(new_tid)
}

#[async_handler]
pub async fn clone(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(u64, usize, UserPtr<usize, Out>, usize, UserPtr<usize, Out>) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (flags, stack, parent_tidptr, tls, child_tidptr) = cx.args();
    let ret = clone_task(ts, &cx, flags, stack, parent_tidptr, tls, child_tidptr).await;
    cx.ret(ret);
    Continue(None)
}

#[async_handler]
pub async fn waitpid(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(isize, UserPtr<i32, Out>, i32) -> Result<usize, Error>>,
) -> ScRet {
    async fn inner(
        ts: &mut TaskState,
        pid: isize,
        mut wstatus: UserPtr<i32, Out>,
        _options: i32,
    ) -> Result<usize, Error> {
        log::trace!("task::wait pid = {pid}");
        let (res, tid) = if pid <= 0 {
            let children = ksync::critical(|| ts.task.children.lock().clone());
            log::trace!("task::wait found {} child(ren)", children.len());
            let events = children.iter().map(|c| c.event());
            let select_all = select_all(events.map(|event| event.recv_once())).await;
            (select_all.0, children[select_all.1].tid)
        } else {
            let child = ksync::critical(|| ts.task.children.lock().get(pid as usize).cloned());
            (child.ok_or(ECHILD)?.event().recv().await, pid as usize)
        };
        log::trace!("task::wait event = {res:?}");
        let event = match res {
            Ok(w) => w,
            Err(e) => e.data().ok_or(ECHILD)?,
        };

        let ws = match event {
            TaskEvent::Exited(code, sig) => ((code & 0xff) << 8) | sig.map_or(0, Sig::raw),
            TaskEvent::Suspended(sig) => (sig.raw() << 8) | 0x7f,
            TaskEvent::Continued => 0xffff,
        };
        wstatus.write(ts.virt.as_ref(), ws).await?;
        Ok(tid)
    }
    let (pid, wstatus, options) = cx.args();

    cx.ret(inner(ts, pid, wstatus, options).await);
    Continue(None)
}
