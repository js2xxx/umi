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
use hashbrown::HashMap;
use kmem::Virt;
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
        loop {
            match event.recv().await {
                Ok(TaskEvent::Exited(code, _)) => break code,
                Err(err) => {
                    let task_event = err
                        .data()
                        .expect("Task returned without sending a break code");
                    if let TaskEvent::Exited(code, _) = task_event {
                        break code;
                    }
                }
                _ => {}
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

impl TaskState {
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
