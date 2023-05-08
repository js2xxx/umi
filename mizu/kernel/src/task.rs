mod elf;
pub mod fd;
mod future;

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    mem,
    ops::ControlFlow::Break,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use co_trap::{TrapFrame, UserCx};
use hashbrown::HashMap;
use kmem::{Phys, Virt};
use ksc::{
    async_handler,
    Error::{self, ENOSYS},
};
use ksync::Broadcast;
use rand_riscv::RandomState;
use riscv::register::sstatus;
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use spin::{Lazy, Mutex};
use sygnal::{ActionSet, Sig, SigSet, Signals};
use umifs::path::Path;

use self::fd::Files;
use crate::{
    executor,
    syscall::ScRet,
    task::future::{user_loop, TaskFut},
};

const DEFAULT_STACK_SIZE: usize = PAGE_SIZE * 32;
const DEFAULT_STACK_ATTR: Attr = Attr::USER_ACCESS
    .union(Attr::READABLE)
    .union(Attr::WRITABLE);

pub struct TaskState {
    task: Arc<Task>,
    sig_mask: SigSet,
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

    #[async_handler]
    pub async fn exit(_: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
        Break(cx.args())
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

pub struct InitTask {
    main: Weak<Task>,
    virt: Pin<Arsc<Virt>>,
    tf: TrapFrame,
    files: Arsc<Files>,
}

impl InitTask {
    async fn load_stack(virt: Pin<&Virt>, stack: Option<(usize, Attr)>) -> Result<LAddr, Error> {
        log::trace!("InitTask::load_stack {stack:x?}");

        let (stack_size, stack_attr) = stack
            .filter(|&(size, _)| size != 0)
            .unwrap_or((DEFAULT_STACK_SIZE, DEFAULT_STACK_ATTR));
        let stack_size = (stack_size + PAGE_MASK) & !PAGE_MASK;

        let addr = virt
            .map(
                None,
                Arc::new(Phys::new_anon()),
                0,
                (stack_size >> PAGE_SHIFT) + 1,
                stack_attr,
            )
            .await?;
        virt.reprotect(addr..(addr + PAGE_SIZE), stack_attr - Attr::WRITABLE)
            .await?;

        let sp = addr + PAGE_SIZE + stack_size - 8;
        virt.commit(LAddr::from(sp.val())).await?;

        Ok(sp)
    }

    fn trap_frame(entry: LAddr, stack: LAddr, arg: usize) -> TrapFrame {
        log::trace!("InitStack::trap_frame: entry = {entry:?}, stack = {stack:?}, arg = {arg}");
        TrapFrame {
            gpr: co_trap::Gpr {
                tx: co_trap::Tx {
                    sp: stack.val(),
                    gp: entry.val(),
                    a: [arg, 0, 0, 0, 0, 0, 0, 0],
                    ..Default::default()
                },
                ..Default::default()
            },
            sepc: entry.val(),
            sstatus: {
                let sstatus: usize = unsafe { mem::transmute(sstatus::read()) };
                (sstatus | (1 << 5)) & !(1 << 8)
            },
            ..Default::default()
        }
    }

    pub async fn from_elf(file: Phys, lib_path: Vec<&Path>) -> Result<Self, Error> {
        let phys = Arc::new(file);
        let virt = crate::mem::new_virt();

        let has_interp = if let Some(interp) = elf::get_interp(&phys).await? {
            let _ = (lib_path, interp);
            todo!("load deynamic linker");
        } else {
            false
        };

        let loaded = elf::load(&phys, None, virt.as_ref()).await?;
        if loaded.tls.is_some() && !has_interp {
            return Err(ENOSYS);
        }
        virt.commit(loaded.entry).await?;

        let stack = Self::load_stack(virt.as_ref(), loaded.stack).await?;

        let tf = Self::trap_frame(loaded.entry, stack, Default::default());

        Ok(InitTask {
            main: Weak::new(),
            virt,
            tf,
            files: Arsc::new(Files::new(fd::default_stdio().await?)),
        })
    }

    pub async fn thread(
        task: Arc<Task>,
        entry: LAddr,
        arg: usize,
        stack: Option<(usize, Attr)>,
    ) -> Result<Self, Error> {
        let virt = task.virt.clone();

        let stack = Self::load_stack(virt.as_ref(), stack).await?;

        let tf = Self::trap_frame(entry, stack, arg);

        Ok(InitTask {
            main: Weak::new(),
            virt,
            tf,
            files: task.files.clone(),
        })
    }

    pub fn spawn(self) -> Result<Arc<Task>, ksc::Error> {
        static TID: AtomicUsize = AtomicUsize::new(0);

        let tid = TID.fetch_add(1, SeqCst);
        let task = Arc::new(Task {
            main: self.main,
            tid,
            virt: self.virt,

            sig: Signals::new(),
            sig_actions: ActionSet::new(),
            event: Broadcast::new(),
            files: self.files,
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
