use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec,
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
use rv39_paging::{Attr, LAddr, ID_OFFSET, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use sygnal::{ActionSet, Sig, SigSet, Signals};
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
    parent: Weak<Task>,
    virt: Pin<Arsc<Virt>>,
    tf: TrapFrame,
    files: Files,
}

impl InitTask {
    pub(super) async fn load_stack(
        virt: Pin<&Virt>,
        stack: Option<(usize, Attr)>,
        args: Vec<String>,
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

        let end = addr + PAGE_SIZE + stack_size;
        let count = args.len();
        let len =
            mem::size_of::<usize>() * (count + 1) + args.iter().map(|s| s.len() + 1).sum::<usize>();
        assert!(len <= PAGE_SIZE);

        let sp = LAddr::from((end - len).val() & !7);
        let paddr = virt.commit(sp).await?;

        // Populate args.
        unsafe {
            let lsp = paddr.to_laddr(ID_OFFSET);
            log::trace!("argc: *{sp:?} = {count}");
            lsp.cast::<usize>().write(count);
            let argv = lsp.add(mem::size_of::<usize>());
            let argp = argv.add(mem::size_of::<usize>() * count);
            let (_, off) = args.iter().fold((argp, Vec::new()), |(ptr, mut off), arg| {
                let aptr = (ptr as usize)
                    .wrapping_add(sp.val())
                    .wrapping_sub(lsp.val());
                off.push(aptr);
                let arg = arg.as_bytes();
                ptr.copy_from_nonoverlapping(arg.as_ptr(), arg.len());
                ptr.add(arg.len()).write(0);
                (ptr.add(arg.len() + 1), off)
            });
            log::trace!("argv: *{argv:p} = {off:x?}");
            argv.cast::<usize>()
                .copy_from_nonoverlapping(off.as_ptr(), off.len());
        }

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
        virt: Pin<Arsc<Virt>>,
        lib_path: Vec<&Path>,
        args: Vec<String>,
    ) -> Result<Self, Error> {
        let phys = Arc::new(file);

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

        let stack = Self::load_stack(virt.as_ref(), loaded.stack, args).await?;

        let tf = Self::trap_frame(loaded.entry, stack, 0);

        Ok(InitTask {
            parent,
            virt,
            tf,
            files: Files::new(fd::default_stdio().await?, "/".into()),
        })
    }

    pub fn spawn(self) -> Result<Arc<Task>, ksc::Error> {
        let tid = alloc_tid();
        let task = Arc::new(Task {
            parent: self.parent,
            children: spin::Mutex::new(Default::default()),
            tid,

            sig: Signals::new(),
            event: Broadcast::new(),
        });

        let ts = TaskState {
            task: task.clone(),
            tgroup: Arsc::new((tid, spin::RwLock::new(vec![task.clone()]))),
            sig_mask: SigSet::EMPTY,
            brk: 0,
            system_times: 0,
            user_times: 0,
            virt: self.virt,
            files: self.files,
            sig_actions: Arsc::new(ActionSet::new()),
            tid_clear: None,
            exit_signal: Some(Sig::SIGCHLD),
        };

        ksync::critical(|| TASKS.lock().insert(tid, task.clone()));
        let fut = TaskFut::new(ts.virt.clone(), user_loop(ts, self.tf));
        executor().spawn(fut).detach();

        Ok(task)
    }

    pub fn reset(self, ts: &mut TaskState, tf: &mut TrapFrame) {
        ts.virt = self.virt;
        // TODO: ts.files.append_afterlife(self.files);
        *tf = self.tf;
    }
}

pub(super) fn alloc_tid() -> usize {
    static TID: AtomicUsize = AtomicUsize::new(2);
    TID.fetch_add(1, SeqCst)
}
