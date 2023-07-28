mod cmd;
mod elf;
pub mod fd;
mod future;
pub mod signal;
mod syscall;
mod time;

use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};

use arsc_rs::Arsc;
use crossbeam_queue::SegQueue;
use futures_util::future::{select, select_all, Either};
use kmem::Virt;
use ksc::Error::{self, ECHILD, EPERM};
use ksync::{
    channel::{mpmc::Receiver, unbounded, Broadcast},
    AtomicArsc,
};
use rv39_paging::{Attr, PAGE_SIZE};
use sygnal::{ActionSet, Sig, SigInfo, SigSet, Signals};

pub use self::{cmd::Command, future::yield_now, syscall::*};
use self::{
    fd::Files,
    signal::SigStack,
    time::{Counter, Times},
};
use crate::mem::{Futexes, Out, Shm, UserPtr};

const DEFAULT_STACK_SIZE: usize = PAGE_SIZE * 80;
const DEFAULT_STACK_ATTR: Attr = Attr::builder()
    .user_access(true)
    .readable(true)
    .writable(true)
    .build();

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
    executable: spin::Mutex<String>,

    times: Arc<Times>,

    sig: Signals,
    shared_sig: AtomicArsc<Signals>,
    event: Broadcast<SegQueue<TaskEvent>>,
}

impl Task {
    fn event(&self) -> Receiver<SegQueue<TaskEvent>> {
        let (tx, rx) = unbounded();
        self.event.subscribe(tx);
        rx
    }

    pub async fn wait(&self) -> (i32, Option<Sig>) {
        let event = self.event();
        let msg = "Task returned without sending a break code";
        loop {
            let e = match event.recv().await {
                Ok(e) => e,
                Err(err) => err.data().expect(msg),
            };
            if let TaskEvent::Exited(code, sig) = e {
                break (code, sig);
            }
        }
    }
}

pub struct TaskState {
    pub(crate) task: Arc<Task>,
    tgroup: Arsc<(usize, spin::RwLock<Vec<Arc<Task>>>)>,
    counters: [Counter; 3],

    sig_mask: SigSet,
    sig_stack: Option<SigStack>,
    pub(crate) brk: usize,

    pub(crate) virt: Arsc<Virt>,
    pub(crate) futex: Arsc<Futexes>,
    pub(crate) shm: Arsc<Shm>,
    sig_actions: Arsc<ActionSet>,
    pub(crate) files: Files,
    tid_clear: Option<UserPtr<usize, Out>>,
    exit_signal: Option<Sig>,
}

#[derive(Debug, Clone, Copy)]
pub enum PidSelection {
    Group(Option<usize>),
    Task(Option<usize>),
}

impl From<isize> for PidSelection {
    fn from(value: isize) -> Self {
        match value {
            -1 => PidSelection::Task(None),
            0 => PidSelection::Group(None),
            x if x > 0 => PidSelection::Task(Some(x as usize)),
            x => PidSelection::Group(Some(-x as usize)),
        }
    }
}

impl TaskState {
    async fn wait(&self, pid: PidSelection) -> Result<(TaskEvent, usize), Error> {
        let (res, tid) = match pid {
            PidSelection::Task(None) => {
                let children = ksync::critical(|| self.task.children.lock().clone());
                log::trace!("task::wait found {} child(ren)", children.len());

                for c in children.iter() {
                    self.task.times.append_child(&c.task.times)
                }
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
            PidSelection::Task(Some(tid)) if tid == self.task.tid => return Err(EPERM),
            PidSelection::Task(Some(tid)) => {
                let child = ksync::critical(|| {
                    let children = self.task.children.lock();
                    for c in children.iter() {
                        self.task.times.append_child(&c.task.times)
                    }
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

    async fn cleanup(mut self, code: i32, sig: Option<Sig>) {
        if let Some(mut tid_clear) = self.tid_clear.take() {
            let _ = tid_clear.write(&self.virt, 0).await;
            self.futex.notify(tid_clear.to_futex_key(), 1);
        }

        let last_thread = ksync::critical(|| {
            let mut tgroup = self.tgroup.1.write();
            let index = tgroup.iter().position(|t| Arc::ptr_eq(t, &self.task));
            tgroup.swap_remove(index.unwrap());
            tgroup.is_empty()
        });
        if last_thread {
            let exit_signal = self.exit_signal.take();
            if let (Some(sig), Some(parent)) = (exit_signal, self.task.parent.upgrade()) {
                parent.sig.push(SigInfo {
                    sig,
                    code: sygnal::SigCode::USER as _,
                    fields: sygnal::SigFields::SigChld {
                        pid: self.task.tid,
                        uid: 0,
                        status: code,
                    },
                })
            }

            self.virt.clear().await;
            // let default_pt = LAddr::from(&crate::rxx::BOOT_PAGES as *const
            // _).to_paddr(ID_OFFSET);
            // unsafe { kmem::unset_virt(default_pt) };
        }

        let _ = self.files.flush_all().await;

        self.task.event.send(&TaskEvent::Exited(code, sig)).await;
        log::trace!("Sent exited event {code} {sig:?}");
    }
}
