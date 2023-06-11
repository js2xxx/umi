use alloc::{
    string::{String, ToString},
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::{
    ffi::CStr,
    mem,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use co_trap::TrapFrame;
use kmem::{Phys, Virt};
use ksc::Error::{self, EISDIR, ENOSYS};
use ksync::channel::Broadcast;
use rand_riscv::rand_core::RngCore;
use riscv::register::sstatus;
use rv39_paging::{Attr, LAddr, ID_OFFSET, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use sygnal::{ActionSet, Sig, SigSet};
use umifs::{
    path::Path,
    types::{OpenOptions, Permissions},
};

use crate::{
    executor,
    mem::Futexes,
    task::{
        elf, fd,
        fd::Files,
        future::{user_loop, TaskFut},
        Task, TaskState, DEFAULT_STACK_ATTR, DEFAULT_STACK_SIZE, TASKS,
    },
};

#[derive(Default)]
pub struct Command {
    image: Option<Arc<Phys>>,
    executable: String,
    virt: Option<Pin<Arsc<Virt>>>,
    parent: Weak<Task>,
    args: Vec<String>,
    envs: Vec<String>,
}

impl Command {
    pub fn new(executable: impl Into<String>) -> Self {
        Command {
            executable: executable.into(),
            ..Default::default()
        }
    }

    pub fn parent(&mut self, parent: Weak<Task>) -> &mut Self {
        self.parent = parent;
        self
    }

    pub async fn open(&mut self, path: impl AsRef<Path>) -> Result<&mut Self, Error> {
        let (entry, _) = crate::fs::open(
            path.as_ref(),
            OpenOptions::RDONLY,
            Permissions::SELF_R | Permissions::SELF_X,
        )
        .await?;
        let io = entry.to_io().ok_or(EISDIR)?;
        self.image = Some(Arc::new(crate::mem::new_phys(io, true)));
        Ok(self)
    }

    pub async fn open_executable(&mut self) -> Result<&mut Self, Error> {
        let (entry, _) = crate::fs::open(
            self.executable.as_ref(),
            OpenOptions::RDONLY,
            Permissions::SELF_R | Permissions::SELF_X,
        )
        .await?;
        let io = entry.to_io().ok_or(EISDIR)?;
        self.image = Some(Arc::new(crate::mem::new_phys(io, true)));
        Ok(self)
    }

    pub fn image(&mut self, image: Arc<Phys>) -> &mut Self {
        self.image = Some(image);
        self
    }

    pub fn virt(&mut self, virt: Pin<Arsc<Virt>>) -> &mut Self {
        self.virt = Some(virt);
        self
    }

    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<A: Into<String>>(&mut self, args: impl IntoIterator<Item = A>) -> &mut Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, env: impl Into<String>) -> &mut Self {
        self.envs.push(env.into());
        self
    }

    pub fn envs<A: Into<String>>(&mut self, envs: impl IntoIterator<Item = A>) -> &mut Self {
        self.envs.extend(envs.into_iter().map(Into::into));
        self
    }

    async fn build(&mut self) -> Result<InitTask, Error> {
        let Command {
            image,
            executable,
            virt,
            parent,
            args,
            envs,
        } = mem::take(self);
        InitTask::from_elf(
            executable,
            parent,
            &image.expect("Require an image"),
            virt.unwrap_or_else(crate::mem::new_virt),
            args,
            envs,
        )
        .await
    }

    pub async fn spawn(&mut self) -> Result<Arc<Task>, Error> {
        self.build().await?.spawn()
    }

    pub async fn exec(&mut self, ts: &mut TaskState, tf: &mut TrapFrame) -> Result<(), Error> {
        self.build().await?.reset(ts, tf).await;
        Ok(())
    }
}

struct InitTask {
    executable: String,
    parent: Weak<Task>,
    virt: Pin<Arsc<Virt>>,
    tf: TrapFrame,
    files: Files,
}

impl InitTask {
    async unsafe fn populate_args(
        stack: LAddr,
        virt: Pin<&Virt>,
        args: &[String],
        envs: &[String],
        auxv: &[(u8, usize)],
    ) -> Result<LAddr, Error> {
        let argc_len = mem::size_of::<usize>();
        let argv_len = mem::size_of::<usize>() * (args.len() + 1);
        let envp_len = mem::size_of::<usize>() * (envs.len() + 1);
        let auxv_len = mem::size_of::<usize>() * (auxv.len() * 2 + 1);
        let rand_len = mem::size_of::<u64>() * 2;
        let args_len = args.iter().map(|s| s.len() + 1).sum::<usize>();
        let envs_len = envs.iter().map(|s| s.len() + 1).sum::<usize>();

        let len = argc_len + argv_len + envp_len + auxv_len + rand_len + args_len + envs_len;
        let ret = LAddr::from((stack - len).val() & !7);

        let paddr = virt.commit(ret, Default::default()).await?;

        let argc_ptr = paddr.to_laddr(ID_OFFSET);
        let mut argv_ptr = argc_ptr + argc_len;
        let argv_addr = ret + argc_len;

        let mut envp_ptr = argv_ptr + argv_len;
        let envp_addr = argv_addr + argv_len;

        let mut auxv_ptr = envp_ptr + envp_len;
        let auxv_addr = envp_addr + envp_len;

        let rand_ptr = auxv_ptr + auxv_len;
        let rand_addr = auxv_addr + auxv_len;

        let mut args_ptr = rand_ptr + rand_len;
        let mut args_addr = rand_addr + rand_len;
        let mut envs_ptr = args_ptr + args_len;
        let mut envs_addr = args_addr + args_len;

        argc_ptr.cast::<usize>().write(args.len());

        for arg in args {
            argv_ptr.cast::<usize>().write(args_addr.val());
            let src = arg.as_bytes();
            args_ptr.copy_from_nonoverlapping(src.as_ptr(), src.len());
            argv_ptr += mem::size_of::<usize>();
            args_ptr += src.len() + 1;
            args_addr += src.len() + 1;
        }

        for env in envs {
            envp_ptr.cast::<usize>().write(envs_addr.val());
            let src = env.as_bytes();
            envs_ptr.copy_from_nonoverlapping(src.as_ptr(), src.len());
            envp_ptr += mem::size_of::<usize>();
            envs_ptr += src.len() + 1;
            envs_addr += src.len() + 1;
        }

        for (idx, val) in auxv.iter().copied() {
            let val = if val == 0xdeadbeef {
                rand_addr.val()
            } else {
                val
            };
            auxv_ptr.cast::<[usize; 2]>().write([idx as usize, val]);
            auxv_ptr += mem::size_of::<[usize; 2]>();
        }

        let mut rng = rand_riscv::rng();
        rand_ptr
            .cast::<[u64; 2]>()
            .write([rng.next_u64(), rng.next_u64()]);

        Ok(ret)
    }

    pub(super) async fn load_stack(
        virt: Pin<&Virt>,
        stack: Option<(usize, Attr)>,
        args: &[String],
        envs: &[String],
        auxv: &[(u8, usize)],
    ) -> Result<LAddr, Error> {
        log::trace!("InitTask::load_stack {stack:?}");

        let (stack_size, stack_attr) = stack
            .filter(|&(size, _)| size != 0)
            .unwrap_or((DEFAULT_STACK_SIZE, DEFAULT_STACK_ATTR));
        let stack_size = (stack_size + PAGE_MASK) & !PAGE_MASK;

        let addr = virt
            .map(
                None,
                Phys::new(true),
                0,
                (stack_size >> PAGE_SHIFT) + 1,
                stack_attr,
            )
            .await?;
        virt.reprotect(addr..(addr + PAGE_SIZE), stack_attr - Attr::WRITABLE)
            .await?;

        let end = addr + PAGE_SIZE + stack_size;
        let sp = unsafe { Self::populate_args(end, virt, args, envs, auxv) }.await?;

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
                (sstatus | (1 << 5) | (1 << 18) | (1 << 13)) & !(1 << 8)
            },
            ..Default::default()
        }
    }

    async fn from_elf(
        executable: String,
        parent: Weak<Task>,
        phys: &Arc<Phys>,
        virt: Pin<Arsc<Virt>>,
        args: Vec<String>,
        envs: Vec<String>,
    ) -> Result<Self, Error> {
        const AT_PHDR: u8 = 3; // Program header table base address
        const AT_PHENT: u8 = 4; // Size of program header entry
        const AT_PHNUM: u8 = 5; // Number of program headers
        const AT_PAGESZ: u8 = 6;
        const AT_BASE: u8 = 7; // Load base address
        const AT_RANDOM: u8 = 25;

        let (loaded, args) = match elf::get_interp(phys).await? {
            Some(interp) => {
                let mut interp = CStr::from_bytes_until_nul(&interp)?.to_str()?.to_string();
                interp = interp.replace("/lib/ld-musl-riscv64-sf.so.1", "libc.so");

                let (entry, _) = crate::fs::open(
                    interp.as_ref(),
                    OpenOptions::RDONLY,
                    Permissions::SELF_R | Permissions::SELF_X,
                )
                .await?;
                let phys = crate::mem::new_phys(entry.to_io().ok_or(EISDIR)?, true);
                let loaded = elf::load(&Arc::new(phys), None, virt.as_ref()).await?;

                let args = [interp, "--library-path=/".into()].into_iter().chain(args);
                (loaded, args.collect())
            }
            None => {
                let loaded = elf::load(phys, None, virt.as_ref()).await?;
                if loaded.is_dyn {
                    return Err(ENOSYS);
                }
                (loaded, args)
            }
        };
        virt.commit(loaded.entry, Attr::USER_RX).await?;

        let base = loaded.range.start;

        let stack = Self::load_stack(
            virt.as_ref(),
            loaded.stack,
            &args,
            &envs,
            &[
                (AT_PAGESZ, PAGE_SIZE),
                (AT_RANDOM, 0xdeadbeef),
                (AT_BASE, base.val()),
                (AT_PHDR, base.val() + loaded.header.e_phoff as usize),
                (AT_PHENT, loaded.header.e_phentsize as usize),
                (AT_PHNUM, loaded.header.e_phnum as usize),
            ],
        )
        .await?;

        let tf = Self::trap_frame(loaded.entry, stack, 0);

        Ok(InitTask {
            executable,
            parent,
            virt,
            tf,
            files: Files::new(fd::default_stdio().await?, "/".into()),
        })
    }

    fn spawn(self) -> Result<Arc<Task>, ksc::Error> {
        let tid = alloc_tid();
        let task = Arc::new(Task {
            executable: spin::Mutex::new(self.executable),
            parent: self.parent,
            children: spin::Mutex::new(Default::default()),
            tid,

            times: Default::default(),

            sig: Default::default(),
            shared_sig: Default::default(),
            event: Broadcast::new(),
        });

        let ts = TaskState {
            task: task.clone(),
            tgroup: Arsc::new((tid, spin::RwLock::new(vec![task.clone()]))),
            counters: super::time::counters(),
            sig_mask: SigSet::EMPTY,
            sig_stack: None,
            brk: 0,
            virt: self.virt,
            futex: Arsc::new(Futexes::new()),
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

    pub async fn reset(self, ts: &mut TaskState, tf: &mut TrapFrame) {
        ksync::critical(|| *ts.task.executable.lock() = self.executable);
        crate::trap::FP.with(|fp| fp.mark_reset());
        ts.task.shared_sig.swap(Default::default(), SeqCst);
        ts.brk = 0;
        ts.virt = self.virt;
        ts.files.close_on_exec().await;
        ts.futex = Arsc::new(Default::default());
        *tf = self.tf;
        super::yield_now().await
    }
}

pub(super) fn alloc_tid() -> usize {
    static TID: AtomicUsize = AtomicUsize::new(2);
    TID.fetch_add(1, SeqCst)
}
