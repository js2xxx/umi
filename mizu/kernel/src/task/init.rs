use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    mem,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use co_trap::TrapFrame;
use kmem::{Phys, Virt};
use ksc::Error::{self, ENOSYS};
use ksync::Broadcast;
use riscv::register::sstatus;
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use sygnal::{ActionSet, SigSet, Signals};
use umifs::path::Path;

use crate::{
    executor,
    task::{
        elf, fd,
        fd::Files,
        future::{user_loop, TaskFut},
        Task, TaskState, DEFAULT_STACK_ATTR, DEFAULT_STACK_SIZE, TASKS,
    },
};

pub struct InitTask {
    main: Weak<Task>,
    parent: Weak<Task>,
    virt: Pin<Arsc<Virt>>,
    tf: TrapFrame,
    files: Files,
}

impl InitTask {
    pub(super) async fn load_stack(
        virt: Pin<&Virt>,
        stack: Option<(usize, Attr)>,
    ) -> Result<LAddr, Error> {
        log::trace!("InitTask::load_stack {stack:?}");

        let (stack_size, stack_attr) = stack
            .filter(|&(size, _)| size != 0)
            .unwrap_or((DEFAULT_STACK_SIZE, DEFAULT_STACK_ATTR));
        let stack_size = (stack_size + PAGE_MASK) & !PAGE_MASK;

        let addr = virt
            .map(
                None,
                Arc::new(Phys::new_anon(true)),
                0,
                (stack_size >> PAGE_SHIFT) + 1,
                stack_attr,
            )
            .await?;
        virt.reprotect(addr..(addr + PAGE_SIZE), stack_attr - Attr::WRITABLE)
            .await?;

        let sp = addr + PAGE_SIZE + stack_size - 8;
        virt.commit(LAddr::from(sp.val())).await?;

        log::trace!("InitTask::load_stack finish {sp:?}");
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
                (sstatus | (1 << 5) | (1 << 18)) & !(1 << 8)
            },
            ..Default::default()
        }
    }

    pub async fn from_elf(
        parent: Weak<Task>,
        file: Phys,
        lib_path: Vec<&Path>,
    ) -> Result<Self, Error> {
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
            parent,
            virt,
            tf,
            files: Files::new(fd::default_stdio().await?, "/".into()),
        })
    }

    pub fn spawn(self) -> Result<Arc<Task>, ksc::Error> {
        let tid = alloc_tid();
        let task = Arc::new(Task {
            main: self.main,
            parent: self.parent,
            children: spin::Mutex::new(Default::default()),
            tid,

            sig: Signals::new(),
            event: Broadcast::new(),
        });

        let ts = TaskState {
            task: task.clone(),
            sig_mask: SigSet::EMPTY,
            brk: 0,
            system_times: 0,
            user_times: 0,
            virt: self.virt,
            files: self.files,
            sig_actions: Arsc::new(ActionSet::new()),
            tid_clear: None,
            exit_signal: None,
        };

        ksync::critical(|| TASKS.lock().insert(tid, task.clone()));
        let fut = TaskFut::new(ts.virt.clone(), user_loop(ts, self.tf));
        executor().spawn(fut).detach();

        Ok(task)
    }
}

pub(super) fn alloc_tid() -> usize {
    static TID: AtomicUsize = AtomicUsize::new(2);
    TID.fetch_add(1, SeqCst)
}
