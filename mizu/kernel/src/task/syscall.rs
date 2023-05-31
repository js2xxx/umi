use alloc::{boxed::Box, string::ToString, sync::Arc, vec, vec::Vec};
use core::{
    mem,
    num::NonZeroUsize,
    ops::ControlFlow::{Break, Continue},
    sync::atomic::Ordering::SeqCst,
};

use arsc_rs::Arsc;
use cmd::Command;
use co_trap::{TrapFrame, UserCx};
use ksc::{
    async_handler,
    Error::{self, EINVAL, EPERM},
    RawReg,
};
use ksync::{channel::Broadcast, AtomicArsc};
use riscv::register::time;
use sygnal::{Sig, SigCode, SigFields, SigInfo, SigSet, Signals};

use crate::{
    executor,
    mem::{deep_fork, In, Out, UserPtr, USER_RANGE},
    syscall::{ScRet, Tv},
    task::{
        cmd,
        fd::MAX_PATH_LEN,
        future::{user_loop, TaskFut},
        time::Times,
        yield_now, Child, Task, TaskEvent, TaskState, TASKS,
    },
};

#[async_handler]
pub async fn uyield(_: &mut TaskState, cx: UserCx<'_, fn()>) -> ScRet {
    yield_now().await;
    cx.ret(());
    ScRet::Continue(None)
}

#[async_handler]
pub async fn tid(ts: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(ts.task.tid);
    Continue(None)
}

#[async_handler]
pub async fn pid(ts: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(ts.tgroup.0);
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
    let data = ts.task.times.get(true);
    cx.ret(out.write_slice(ts.virt.as_ref(), &data, false).await);
    Continue(None)
}
const RLIMIT_CPU: u32 = 0; // CPU time in sec
const RLIMIT_DATA: u32 = 2; // max data size
const RLIMIT_STACK: u32 = 3; // max stack size
const RLIMIT_NPROC: u32 = 6; // max number of processes
const RLIMIT_NOFILE: u32 = 7; // max number of open files
const RLIMIT_AS: u32 = 9; // address space limit

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Rlimit {
    cur: usize,
    max: usize,
}

#[async_handler]
pub async fn prlimit(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, u32, UserPtr<Rlimit, In>, UserPtr<Rlimit, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (pid, ty, new, mut old) = cx.args();
    let fut = async move {
        if pid != 0 {
            return Err(EPERM);
        }
        let (cur, max) = match ty {
            RLIMIT_AS => (USER_RANGE.len(), USER_RANGE.len()),
            RLIMIT_NPROC => (65536, 65536),
            RLIMIT_CPU => {
                let s = time::read() / config::TIME_FREQ as usize;
                (s, usize::MAX)
            }
            RLIMIT_DATA | RLIMIT_STACK => (8 * 1024 * 1024, usize::MAX),
            RLIMIT_NOFILE => {
                let limit = if new.is_null() {
                    ts.files.get_limit()
                } else {
                    ts.files.set_limit(new.read(ts.virt.as_ref()).await?.cur)
                };
                (limit, limit)
            }
            _ => (usize::MAX, usize::MAX),
        };
        if !old.is_null() {
            old.write(ts.virt.as_ref(), Rlimit { cur, max }).await?;
        }
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Rusage {
    pub utime: Tv,       // user CPU time used
    pub stime: Tv,       // system CPU time used
    pub maxrss: usize,   // maximum resident set size
    pub ixrss: usize,    // integral shared memory size
    pub idrss: usize,    // integral unshared data size
    pub isrss: usize,    // integral unshared stack size
    pub minflt: usize,   // page reclaims (soft page faults)
    pub majflt: usize,   // page faults (hard page faults)
    pub nswap: usize,    // swaps
    pub inblock: usize,  // block input operations
    pub oublock: usize,  // block output operations
    pub msgsnd: usize,   // IPC messages sent
    pub msgrcv: usize,   // IPC messages received
    pub nsignals: usize, // signals received
    pub nvcsw: usize,    // voluntary context switches
    pub nivcsw: usize,   // involuntary context switches
}

#[async_handler]
pub async fn getrusage(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<Rusage, Out>) -> Result<(), Error>>,
) -> ScRet {
    const RUSAGE_SELF: i32 = 0;
    const RUSAGE_CHILDREN: i32 = -1;
    const RUSAGE_THREAD: i32 = 1;

    let (who, mut out) = cx.args();
    let fut = async move {
        let [user, system] = match who {
            RUSAGE_SELF => ts.task.times.get_process(),
            RUSAGE_CHILDREN => ts.task.times.get_children(),
            RUSAGE_THREAD => ts.task.times.get_thread(),
            _ => return Err(EINVAL),
        };
        let rusage = Rusage {
            utime: Tv {
                sec: user.as_secs(),
                usec: user.subsec_micros() as _,
            },
            stime: Tv {
                sec: system.as_secs(),
                usec: system.subsec_micros() as _,
            },
            ..Default::default()
        };
        out.write(ts.virt.as_ref(), rusage).await?;
        Ok(())
    };
    cx.ret(fut.await);
    Continue(None)
}

#[async_handler]
pub async fn set_tid_addr(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<usize, Out>) -> usize>,
) -> ScRet {
    ts.tid_clear = Some(cx.args());
    cx.ret(ts.task.tid);
    Continue(None)
}

#[async_handler]
pub async fn exit(_: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
    Break(cx.args())
}

#[async_handler]
pub async fn exit_group(ts: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
    ts.sig_fatal(
        SigInfo {
            sig: Sig::SIGKILL,
            code: SigCode::USER as _,
            fields: SigFields::None,
        },
        false,
    );
    Break(cx.args())
}

async fn clone_task(
    ts: &mut TaskState,
    tf: &TrapFrame,
    flags: u64,
    stack: Option<NonZeroUsize>,
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
            /// Share thread group.
            const THREAD         = 0x00010000;
            /// Set TLS.
            const SETTLS         = 0x00080000;

            const PARENT_SETTID  = 0x00100000;
            const CHILD_CLEARTID = 0x00200000;
            const CHILD_SETTID   = 0x01000000;
        }
    }
    let flags = Flags::from_bits_truncate(flags);

    if flags.contains(Flags::SIGHAND) && !flags.contains(Flags::VM) {
        return Err(EINVAL);
    }

    if flags.contains(Flags::THREAD) && !flags.contains(Flags::SIGHAND) {
        return Err(EINVAL);
    }

    let bits = (flags & Flags::CSIGNAL).bits();
    let exit_signal = if flags.intersects(Flags::PARENT | Flags::THREAD) {
        ts.exit_signal
    } else {
        Some(if bits == 0 {
            Sig::SIGCHLD
        } else {
            Sig::new(bits as i32).ok_or(EINVAL)?
        })
    };

    log::trace!("clone_task: flags = {flags:?}");

    let new_tid = cmd::alloc_tid();
    log::trace!("new tid = {new_tid}");
    let task = Arc::new(Task {
        executable: spin::Mutex::new(ksync::critical(|| ts.task.executable.lock().clone())),
        parent: if flags.intersects(Flags::PARENT | Flags::THREAD) {
            ts.task.parent.clone()
        } else {
            Arc::downgrade(&ts.task)
        },
        children: spin::Mutex::new(Vec::new()),
        tid: new_tid,
        times: if flags.intersects(Flags::THREAD) {
            Times::new_thread(&ts.task.times)
        } else {
            Default::default()
        },
        sig: Signals::new(),
        shared_sig: AtomicArsc::new(if flags.contains(Flags::THREAD) {
            ts.task.shared_sig.load(SeqCst)
        } else {
            Default::default()
        }),
        event: Broadcast::new(),
    });
    if flags.contains(Flags::PARENT_SETTID) {
        ptid.write(ts.virt.as_ref(), new_tid).await?;
    }
    if flags.contains(Flags::CHILD_SETTID) {
        ctid.write(ts.virt.as_ref(), new_tid).await?;
    }

    log::trace!("clone_task: cloning virt");

    let virt = if flags.contains(Flags::VM) {
        ts.virt.clone()
    } else {
        deep_fork(&ts.virt).await?
    };

    let mut new_tf = *tf;

    log::trace!("clone_task: setting up TrapFrame");

    new_tf.set_syscall_ret(0);
    if let Some(stack) = stack {
        new_tf.gpr.tx.sp = stack.get();
    }
    if flags.contains(Flags::SETTLS) {
        new_tf.gpr.tx.tp = tls;
    }
    log::trace!("clone_task: setting up TaskState");

    let new_ts = TaskState {
        task: task.clone(),
        tgroup: if flags.contains(Flags::THREAD) {
            let tgroup = ts.tgroup.clone();
            ksync::critical(|| tgroup.1.write().push(task.clone()));
            tgroup
        } else {
            Arsc::new((new_tid, spin::RwLock::new(vec![task.clone()])))
        },
        sig_mask: SigSet::EMPTY,
        sig_stack: None,
        brk: ts.brk,
        virt,
        futex: if flags.contains(Flags::THREAD) {
            ts.futex.clone()
        } else {
            Arsc::new(ts.futex.deep_fork())
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

    if !flags.contains(Flags::THREAD) {
        log::trace!(
            "clone_task: push into parent: {:?}",
            new_ts.task.parent.upgrade().map(|s| s.tid)
        );

        if let Some(parent) = new_ts.task.parent.upgrade() {
            ksync::critical(|| {
                parent.children.lock().push(Child {
                    task: new_ts.task.clone(),
                    event: new_ts.task.event(),
                })
            });

            ksync::critical(|| log::debug!("now have {} child(ren)", parent.children.lock().len()));
        }
    }

    ksync::critical(|| TASKS.lock().insert(new_tid, task.clone()));
    let fut = TaskFut::new(new_ts.virt.clone(), user_loop(new_ts, new_tf));
    executor().spawn(fut).detach();

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
    let ret = clone_task(
        ts,
        &cx,
        flags,
        NonZeroUsize::new(stack),
        parent_tidptr,
        tls,
        child_tidptr,
    )
    .await;
    cx.ret(ret);
    Continue(None)
}

#[async_handler]
pub async fn waitpid(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(isize, UserPtr<i32, Out>, i32) -> Result<usize, Error>>,
) -> ScRet {
    let (pid, mut wstatus, _options) = cx.args();
    let inner = async move {
        let (event, tid) = ts.wait(pid.into()).await?;
        if !wstatus.is_null() {
            let ws = match event {
                TaskEvent::Exited(code, sig) => ((code & 0xff) << 8) | sig.map_or(0, Sig::raw),
                TaskEvent::Suspended(sig) => (sig.raw() << 8) | 0x7f,
                TaskEvent::Continued => 0xffff,
            };
            log::trace!("Generated ws = {ws:#x}");
            wstatus.write(ts.virt.as_ref(), ws).await?;
        }
        Ok(tid)
    };
    cx.ret(inner.await);
    Continue(None)
}

#[async_handler]
pub async fn execve(
    ts: &mut TaskState,
    mut cx: UserCx<
        '_,
        fn(UserPtr<u8, In>, UserPtr<usize, In>, UserPtr<usize, In>) -> Result<(), Error>,
    >,
) -> ScRet {
    async fn inner(
        ts: &mut TaskState,
        tf: &mut TrapFrame,
        name: UserPtr<u8, In>,
        args: UserPtr<usize, In>,
        envs: UserPtr<usize, In>,
    ) -> Result<(), Error> {
        let mut ptrs = [0; 64];
        let mut data = [0; MAX_PATH_LEN];

        let (name, root) = name.read_path(ts.virt.as_ref(), &mut data).await?;
        let mut name = if root {
            name.to_path_buf()
        } else {
            ts.files.cwd().join(name)
        };

        let argc = args
            .read_slice_with_zero(ts.virt.as_ref(), &mut ptrs)
            .await?;
        let mut args = Vec::new();
        for &ptr in argc {
            let arg = UserPtr::<_, In>::from_raw(ptr);
            let arg = arg.read_str(ts.virt.as_ref(), &mut data).await?;
            args.push(arg.to_string());
        }

        let envc = envs
            .read_slice_with_zero(ts.virt.as_ref(), &mut ptrs)
            .await?;
        let mut envs = Vec::new();
        for &ptr in envc {
            let env = UserPtr::<_, In>::from_raw(ptr);
            let env = env.read_str(ts.virt.as_ref(), &mut data).await?;
            envs.push(env.to_string());
        }

        if name.as_str().ends_with(".sh") {
            name = "busybox".into();
            let mut old = mem::replace(&mut args, vec!["busybox".into(), "sh".into()]);
            args.append(&mut old);
        }

        log::trace!("task::execve: name = {name:?}, args = {args:?}, envs = {envs:?}");

        let mut cmd = Command::new(name);
        cmd.open_executable().await?;

        ts.sig_fatal(
            SigInfo {
                sig: Sig::SIGKILL,
                code: SigCode::DETHREAD as _,
                fields: SigFields::None,
            },
            true,
        );
        ts.virt.clear().await;

        log::trace!("task::execve: start loading ELF. No way back.");

        cmd.parent(ts.task.parent.clone())
            .virt(ts.virt.clone())
            .args(args)
            .envs(envs)
            .exec(ts, tf)
            .await?;

        Ok(())
    }
    let (name, args, env) = cx.args();
    let ret = inner(ts, &mut cx, name, args, env).await;
    if ret.is_err() {
        cx.ret(ret)
    }
    Continue(None)
}
