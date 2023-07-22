use alloc::boxed::Box;
use core::{mem, num::NonZeroI32, pin::pin, sync::atomic::Ordering::SeqCst};

use co_trap::UserCx;
use futures_util::future::{select, Either};
use ksc::{
    async_handler,
    Error::{self, EINVAL, EPERM, ESRCH, ETIMEDOUT},
};
use ktime::TimeOutExt;
use rv39_paging::{LAddr, PAGE_SIZE};
use sygnal::{Action, ActionType, Sig, SigCode, SigFields, SigInfo, SigSet};

use super::UsigInfo;
use crate::{
    mem::{In, Out, UserPtr},
    syscall::{ffi::Tv, ScRet},
    task::{PidSelection, TaskState},
};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SigAction {
    handler: usize,
    mask: SigSet,
    flags: SigFlags,
}
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

impl Default for SigAction {
    fn default() -> Self {
        SigAction {
            handler: 0,
            mask: SigSet::EMPTY,
            flags: Default::default(),
        }
    }
}

impl From<Action> for SigAction {
    fn from(action: Action) -> Self {
        match action.ty {
            ActionType::Ignore => SigAction {
                handler: SIG_IGN,
                ..Default::default()
            },
            ActionType::Kill | ActionType::Suspend | ActionType::Resume => SigAction {
                handler: SIG_DFL,
                ..Default::default()
            },
            ActionType::User {
                entry,
                use_extra_cx,
                use_alt_stack,
            } => SigAction {
                handler: entry.val(),
                mask: action.mask,
                flags: {
                    let mut flags = Default::default();
                    if use_extra_cx {
                        flags |= SigFlags::SIGINFO
                    }
                    if use_alt_stack {
                        flags |= SigFlags::ONSTACK
                    }
                    flags
                },
            },
        }
    }
}

bitflags::bitflags! {
    #[derive(Default, Clone, Copy, Debug)]
    struct SigFlags: isize {
        const SIGINFO = 4;
        const ONSTACK = 0x08000000;
    }
}

pub const SIGRETURN_GUARD: usize = 0xAEF0_AEF0_AEF0_AEF0;

#[async_handler]
pub async fn sigaction(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(i32, UserPtr<SigAction, In>, UserPtr<SigAction, Out>, usize) -> Result<(), Error>,
    >,
) -> ScRet {
    let (sig, action, mut old, size) = cx.args();
    let fut = async move {
        if size != mem::size_of::<SigSet>() {
            return Err(EINVAL);
        }
        let sig = NonZeroI32::new(sig)
            .and_then(|s| Sig::new(s.get()))
            .ok_or(EINVAL)?;
        let action = action.read(&ts.virt).await?;
        let action = Action {
            ty: match action.handler {
                SIG_DFL => ActionType::default(sig),
                SIG_IGN => ActionType::Ignore,
                entry => ActionType::User {
                    entry: entry.into(),
                    use_extra_cx: action.flags.contains(SigFlags::SIGINFO),
                    use_alt_stack: action.flags.contains(SigFlags::ONSTACK),
                },
            },
            mask: action.mask,
        };
        let old_action = ts.sig_actions.replace(sig, action);
        if old_action.ty == ActionType::Ignore {
            ts.sig_mask &= !sig;
        }
        if action.ty == ActionType::Ignore {
            ts.sig_mask |= sig;
        }
        if !old.is_null() {
            old.write(&ts.virt, old_action.into()).await?;
        }
        Ok(())
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SigStack {
    pub base: LAddr,
    pub flags: i32,
    pub len: usize,
}

impl Default for SigStack {
    fn default() -> Self {
        SigStack {
            base: 0usize.into(),
            flags: 0,
            len: 0,
        }
    }
}

#[async_handler]
pub async fn sigaltstack(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<SigStack, In>, UserPtr<SigStack, Out>) -> Result<(), Error>>,
) -> ScRet {
    const DISABLE: i32 = 2;

    let (stack, mut old) = cx.args();
    let fut = async move {
        if !old.is_null() {
            old.write(&ts.virt, ts.sig_stack.unwrap_or_default())
                .await?;
        }
        if !stack.is_null() {
            let stack = stack.read(&ts.virt).await?;
            if stack.len < PAGE_SIZE * 2 {
                return Err(EINVAL);
            }
            ts.sig_stack = (stack.flags & DISABLE == 0).then_some(stack);
        }
        Ok(())
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn sigprocmask(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<SigSet, In>, UserPtr<SigSet, Out>, usize) -> Result<(), Error>>,
) -> ScRet {
    const SIG_BLOCK: i32 = 0;
    const SIG_UNBLOCK: i32 = 1;
    const SIG_SETMASK: i32 = 2;

    let (how, set, mut old, size) = cx.args();
    let fut = async move {
        if size != mem::size_of::<SigSet>() {
            return Err(EINVAL);
        }
        let set = if !set.is_null() {
            Some(set.read(&ts.virt).await?)
        } else {
            None
        };
        let current = ts.sig_mask;

        if !old.is_null() {
            old.write(&ts.virt, current).await?;
        }
        if let Some(set) = set {
            ts.sig_mask = match how {
                SIG_BLOCK => current | set,
                SIG_UNBLOCK => current & !set,
                SIG_SETMASK => set,
                _ => return Err(EINVAL),
            };
        }
        Ok(())
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn sigtimedwait(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(
            UserPtr<SigSet, In>,
            UserPtr<UsigInfo, Out>,
            UserPtr<Tv, In>,
            usize,
        ) -> Result<i32, Error>,
    >,
) -> ScRet {
    let (set, mut usi_ptr, tv, size) = cx.args();
    let fut = async move {
        if size != mem::size_of::<SigSet>() {
            return Err(EINVAL);
        }
        let set = set.read(&ts.virt).await?;
        let dur = tv.read(&ts.virt).await?.into();
        if set.is_empty() {
            ktime::sleep(dur).await;
            return Ok(0);
        }

        let shared_sig = ts.task.shared_sig.load(SeqCst);

        let local = pin!(ts.task.sig.wait(set));
        let shared = pin!(shared_sig.wait(set));
        let res = select(local, shared)
            .ok_or_timeout(dur, || ETIMEDOUT)
            .await?;
        let si = match res {
            Either::Left((si, _)) => si,
            Either::Right((si, _)) => si,
        };
        if !usi_ptr.is_null() {
            let usi = UsigInfo {
                sig: si.sig,
                errno: 0,
                code: si.code,
            };
            usi_ptr.write(&ts.virt, usi).await?;
        }

        Ok(si.sig.raw())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn kill(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(isize, i32) -> Result<(), Error>>,
) -> ScRet {
    let (pid, sig) = cx.args();
    let fut = async move {
        let pid = PidSelection::from(pid);
        let sig = NonZeroI32::new(sig)
            .and_then(|s| Sig::new(s.get()))
            .ok_or(EINVAL)?;

        let si = SigInfo {
            sig,
            code: SigCode::USER as _,
            fields: SigFields::SigKill {
                pid: ts.task.tid,
                uid: 0,
            },
        };
        match pid {
            PidSelection::Task(Some(tid)) if tid == ts.task.tid => ts.task.sig.push(si),
            PidSelection::Task(Some(tid)) => {
                let child = ksync::critical(|| {
                    let children = ts.task.children.lock();
                    let mut iter = children.iter();
                    iter.find(|c| c.task.tid == tid).map(|c| c.task.clone())
                });
                child.ok_or(ESRCH)?.sig.push(si);
            }
            x => todo!("kill {x:?}"),
        }
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn tkill(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, i32) -> Result<(), Error>>,
) -> ScRet {
    let (tid, sig) = cx.args();
    let fut = async move {
        let sig = NonZeroI32::new(sig)
            .and_then(|s| Sig::new(s.get()))
            .ok_or(EINVAL)?;

        let si = SigInfo {
            sig,
            code: SigCode::USER as _,
            fields: SigFields::SigKill {
                pid: ts.task.tid,
                uid: 0,
            },
        };

        let task = ksync::critical(|| ts.tgroup.1.read().iter().find(|t| t.tid == tid).cloned());
        task.ok_or(ESRCH)?.sig.push(si);
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn tgkill(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize, i32) -> Result<(), Error>>,
) -> ScRet {
    let (tgid, tid, sig) = cx.args();
    let fut = async move {
        let sig = NonZeroI32::new(sig)
            .and_then(|s| Sig::new(s.get()))
            .ok_or(EINVAL)?;

        if ts.tgroup.0 != tgid {
            return Err(EPERM);
        }

        let si = SigInfo {
            sig,
            code: SigCode::USER as _,
            fields: SigFields::SigKill {
                pid: ts.task.tid,
                uid: 0,
            },
        };

        let task = ksync::critical(|| ts.tgroup.1.read().iter().find(|t| t.tid == tid).cloned());
        task.ok_or(ESRCH)?.sig.push(si);
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
