use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
use core::{
    num::NonZeroUsize,
    ops::ControlFlow::{Break, Continue},
};

use arsc_rs::Arsc;
use co_trap::{TrapFrame, UserCx};
use futures_util::future::{select, select_all, Either};
use kmem::Phys;
use ksc::{
    async_handler,
    Error::{self, ECHILD, EINVAL, ENOTDIR},
    RawReg,
};
use ksync::Broadcast;
use sygnal::{Sig, SigSet, Signals};
use umifs::types::Permissions;

use crate::{
    executor,
    mem::{In, Out, UserPtr},
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
pub async fn pid(ts: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    let task = &ts.task;
    cx.ret(task.tid);
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
pub async fn exit(_: &mut TaskState, cx: UserCx<'_, fn(i32)>) -> ScRet {
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
            /// Set TLS.
            const SETTLS         = 0x00080000;

            const PARENT_SETTID  = 0x00100000;
            const CHILD_CLEARTID = 0x00200000;
            const CHILD_SETTID   = 0x01000000;
        }
    }
    let flags = Flags::from_bits_truncate(flags);
    let bits = (flags & Flags::CSIGNAL).bits();
    let exit_signal = Some(if bits == 0 {
        Sig::SIGCHLD
    } else {
        Sig::new(bits as i32).ok_or(EINVAL)?
    });

    log::trace!("clone_task: flags = {flags:?}");

    let new_tid = init::alloc_tid();
    log::trace!("new tid = {new_tid}");
    let task = Arc::new(Task {
        parent: if flags.contains(Flags::PARENT) {
            ts.task.parent.clone()
        } else {
            Arc::downgrade(&ts.task)
        },
        children: spin::Mutex::new(Vec::new()),
        tid: new_tid,
        sig: Signals::new(),
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
        ts.virt.as_ref().deep_fork().await?
    };

    let mut new_tf = *tf;

    log::trace!("clone_task: setting up TrapFrame");

    new_tf.set_syscall_ret(0);
    new_tf.gpr.tx.sp = match stack {
        Some(stack) => stack.get(),
        None => InitTask::load_stack(virt.as_ref(), None, Default::default())
            .await?
            .val(),
    };
    if flags.contains(Flags::SETTLS) {
        new_tf.gpr.tx.tp = tls;
    }
    log::trace!("clone_task: setting up TaskState");

    let new_ts = TaskState {
        task: task.clone(),
        sig_mask: SigSet::EMPTY,
        brk: ts.brk,
        system_times: 0,
        user_times: 0,
        virt,
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

    log::trace!(
        "clone_task: push into parent: {:?}",
        new_ts.task.parent.upgrade()
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
        } else {
            let child = ksync::critical(|| {
                let children = ts.task.children.lock();
                children
                    .iter()
                    .find(|c| c.task.tid == pid as usize)
                    .cloned()
            });
            (child.ok_or(ECHILD)?.event.recv().await, pid as usize)
        };
        log::trace!("task::wait tid = {tid}, event = {res:?}");
        let event = match res {
            Ok(w) => w,
            Err(e) => e.data().ok_or(ECHILD)?,
        };
        if matches!(event, TaskEvent::Exited(..)) {
            ksync::critical(|| ts.task.children.lock().retain(|c| c.task.tid != tid));
        }

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
    }
    let (pid, wstatus, options) = cx.args();

    cx.ret(inner(ts, pid, wstatus, options).await);
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

        let name = name
            .read_path(ts.virt.as_ref(), &mut data)
            .await?
            .to_path_buf();

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

        ts.virt.clear().await;
        let init = InitTask::from_elf(
            ts.task.parent.clone(),
            Phys::new(file.to_io().ok_or(ENOTDIR)?, 0, true),
            ts.virt.clone(),
            Default::default(),
            args,
        )
        .await?;
        init.reset(ts, tf);

        Ok(())
    }
    let (name, args, env) = cx.args();
    let ret = inner(ts, &mut cx, name, args, env).await;
    if ret.is_err() {
        cx.ret(ret)
    }
    Continue(None)
}
