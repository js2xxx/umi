use alloc::{boxed::Box, string::ToString, sync::Arc, vec, vec::Vec};
use core::{
    num::NonZeroUsize,
    ops::ControlFlow::{Break, Continue},
    sync::atomic::Ordering::SeqCst,
};

use arsc_rs::Arsc;
use co_trap::{TrapFrame, UserCx};
use ksc::{
    async_handler,
    Error::{self, EINVAL, ENOTDIR},
    RawReg,
};
use ksync::{AtomicArsc, Broadcast};
use sygnal::{Sig, SigCode, SigFields, SigInfo, SigSet, Signals};
use umifs::types::Permissions;

use crate::{
    executor,
    mem::{deep_fork, In, Out, UserPtr},
    syscall::ScRet,
    task::{
        fd::MAX_PATH_LEN,
        future::{user_loop, TaskFut},
        init, yield_now, Child, InitTask, Task, TaskEvent, TaskState, TASKS,
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
    let data = [ts.user_times, ts.system_times, 0, 0];
    cx.ret(out.write_slice(ts.virt.as_ref(), &data, false).await);
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

    let new_tid = init::alloc_tid();
    log::trace!("new tid = {new_tid}");
    let task = Arc::new(Task {
        parent: if flags.intersects(Flags::PARENT | Flags::THREAD) {
            ts.task.parent.clone()
        } else {
            Arc::downgrade(&ts.task)
        },
        children: spin::Mutex::new(Vec::new()),
        tid: new_tid,
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
        system_times: 0,
        user_times: 0,
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
        let name = if root {
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

        log::trace!("task::execve: name = {name:?}, args = {args:?}, envs = {envs:?}");

        let (file, _) = crate::fs::open(&name, Default::default(), Permissions::all()).await?;

        ts.sig_fatal(
            SigInfo {
                sig: Sig::SIGKILL,
                code: SigCode::DETHREAD as _,
                fields: SigFields::None,
            },
            true,
        );
        ts.virt.clear().await;
        ts.futex = Arsc::new(Default::default());
        ts.task.shared_sig.swap(Default::default(), SeqCst);

        let phys = crate::mem::new_phys(file.to_io().ok_or(ENOTDIR)?, true);

        log::trace!("task::execve: start loading ELF. No way back.");

        let init = InitTask::from_elf(
            ts.task.parent.clone(),
            &Arc::new(phys),
            ts.virt.clone(),
            args,
            envs,
        )
        .await?;
        init.reset(ts, tf).await;

        Ok(())
    }
    let (name, args, env) = cx.args();
    let ret = inner(ts, &mut cx, name, args, env).await;
    if ret.is_err() {
        cx.ret(ret)
    }
    Continue(None)
}
