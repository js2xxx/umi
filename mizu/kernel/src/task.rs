mod elf;
pub mod fd;
mod future;
mod init;
mod syscall;

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::pin::Pin;

use arsc_rs::Arsc;
use crossbeam_queue::SegQueue;
use futures_util::future::{select, select_all, Either};
use hashbrown::HashMap;
use kmem::Virt;
use ksc::Error::{self, ECHILD};
use ksync::{unbounded, Broadcast, Receiver};
use rand_riscv::RandomState;
use rv39_paging::{Attr, PAGE_SIZE};
use spin::{Lazy, Mutex};
use sygnal::{ActionSet, Sig, SigInfo, SigSet, Signals};

use self::fd::Files;
pub use self::{future::yield_now, init::InitTask, syscall::*};
use crate::mem::{Out, UserPtr};

const DEFAULT_STACK_SIZE: usize = PAGE_SIZE * 32;
const DEFAULT_STACK_ATTR: Attr = Attr::USER_ACCESS
    .union(Attr::READABLE)
    .union(Attr::WRITABLE);

#[derive(Clone, Copy, Debug)]
pub enum TaskEvent {
    Exited(i32, Option<Sig>),
    Suspended(Sig),
    Continued,
}

#[derive(Debug, Clone)]
struct Child {
    task: Arc<Task>,
    event: Receiver<SegQueue<TaskEvent>>,
}

#[derive(Debug)]
pub struct Task {
    parent: Weak<Task>,
    children: spin::Mutex<Vec<Child>>,
    tid: usize,

    sig: Signals,
    event: Broadcast<SegQueue<TaskEvent>>,
}

impl Task {
    fn event(&self) -> Receiver<SegQueue<TaskEvent>> {
        let (tx, rx) = unbounded();
        self.event.subscribe(tx);
        rx
    }

    pub async fn wait(&self) -> i32 {
        let event = self.event();
        let msg = "Task returned without sending a break code";
        loop {
            let e = match event.recv().await {
                Ok(e) => e,
                Err(err) => err.data().expect(msg),
            };
            if let TaskEvent::Exited(code, _) = e {
                break code;
            }
        }
    }
}

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

#[derive(Debug, Clone, Copy)]
pub enum WaitPid {
    Group(Option<usize>),
    Task(Option<usize>),
}

impl From<isize> for WaitPid {
    fn from(value: isize) -> Self {
        match value {
            -1 => WaitPid::Task(None),
            0 => WaitPid::Group(None),
            x if x > 0 => WaitPid::Task(Some(x as usize)),
            x => WaitPid::Group(Some(-x as usize)),
        }
    }
}

impl TaskState {
    async fn wait(&self, pid: WaitPid) -> Result<(TaskEvent, usize), Error> {
        let (res, tid) = match pid {
            WaitPid::Task(None) => {
                let children = ksync::critical(|| self.task.children.lock().clone());
                log::trace!("task::wait found {} child(ren)", children.len());

                match &children[..] {
                    [] => return Err(ECHILD),
                    [a] => (a.event.recv().await, a.task.tid),
                    [a, b] => match select(a.event.recv(), b.event.recv()).await {
                        Either::Left((te, _)) => (te, a.task.tid),
                        Either::Right((te, _)) => (te, b.task.tid),
                    },
                    _ => {
                        let events = children.iter().map(|c| &c.event);
                        let select_all = select_all(events.map(|event| event.recv())).await;
                        (select_all.0, children[select_all.1].task.tid)
                    }
                }
            }
            WaitPid::Task(Some(tid)) => {
                let child = ksync::critical(|| {
                    let children = self.task.children.lock();
                    children.iter().find(|c| c.task.tid == tid).cloned()
                });
                (child.ok_or(ECHILD)?.event.recv().await, tid)
            }
            x => todo!("{x:?}"),
        };
        log::trace!("task::wait tid = {tid}, event = {res:?}");
        let event = match res {
            Ok(w) => w,
            Err(e) => e.data().ok_or(ECHILD)?,
        };
        if matches!(event, TaskEvent::Exited(..)) {
            ksync::critical(|| self.task.children.lock().retain(|c| c.task.tid != tid));
        }
        Ok((event, tid))
    }

    async fn cleanup(mut self, code: i32) {
        if let Some(mut tid_clear) = self.tid_clear.take() {
            let _ = tid_clear.write(self.virt.as_ref(), 0).await;
        }

        let sig = self.exit_signal.take();
        if let (Some(sig), Some(parent)) = (sig, self.task.parent.upgrade()) {
            parent.sig.push(SigInfo {
                sig,
                code: sygnal::SigCode::USER,
                fields: sygnal::SigFields::SigChld {
                    pid: self.task.tid,
                    uid: 0,
                    status: code,
                },
            })
        }
        let _ = self.files.flush_all().await;

        self.task.event.send(&TaskEvent::Exited(code, sig)).await;
        log::trace!("Sent exited event {code} {sig:?}");

        ksync::critical(|| TASKS.lock().remove(&self.task.tid));
    }
}

static TASKS: Lazy<Mutex<HashMap<usize, Arc<Task>, RandomState>>> =
    Lazy::new(|| Mutex::new(HashMap::with_hasher(RandomState::new())));
