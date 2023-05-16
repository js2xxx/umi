use alloc::boxed::Box;
use core::{mem, num::NonZeroI32};

use co_trap::UserCx;
use ksc::{
    async_handler,
    Error::{self, EINVAL},
};
use rv39_paging::{LAddr, PAGE_SIZE};
use sygnal::{Action, ActionType, Sig, SigSet};

use crate::{
    mem::{In, Out, UserPtr},
    syscall::ScRet,
    task::TaskState,
};

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SigAction {
    handler: usize,
    mask: SigSet,
    flags: SigFlags,
    restorer: LAddr,
}
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

impl Default for SigAction {
    fn default() -> Self {
        SigAction {
            handler: 0,
            mask: SigSet::EMPTY,
            flags: Default::default(),
            restorer: 0usize.into(),
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
                exit,
                use_extra_cx,
                use_alt_stack,
            } => {
                let default_exit = exit.val() != SIGRETURN_GUARD;
                SigAction {
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
                        if !default_exit {
                            flags |= SigFlags::RESTORER
                        }
                        flags
                    },
                    restorer: if default_exit { exit } else { 0usize.into() },
                }
            }
        }
    }
}

bitflags::bitflags! {
    #[derive(Default, Clone, Copy, Debug)]
    struct SigFlags: isize {
        const SIGINFO = 4;
        const ONSTACK = 0x08000000;
        const RESTORER = 0x04000000;
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
        let action = action.read(ts.virt.as_ref()).await?;
        let action = Action {
            ty: match action.handler {
                SIG_DFL => ActionType::default(sig),
                SIG_IGN => ActionType::Ignore,
                entry => ActionType::User {
                    entry: entry.into(),
                    exit: if action.flags.contains(SigFlags::RESTORER) {
                        action.restorer
                    } else {
                        SIGRETURN_GUARD.into()
                    },
                    use_extra_cx: action.flags.contains(SigFlags::SIGINFO),
                    use_alt_stack: action.flags.contains(SigFlags::ONSTACK),
                },
            },
            mask: action.mask,
        };
        let action = ts.sig_actions.replace(sig, action);
        if !old.is_null() {
            old.write(ts.virt.as_ref(), action.into()).await?;
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
    let (stack, mut old) = cx.args();
    let fut = async move {
        if !old.is_null() {
            old.write(ts.virt.as_ref(), ts.sig_stack.unwrap_or_default())
                .await?;
        }
        ts.sig_stack = if !stack.is_null() {
            let stack = stack.read(ts.virt.as_ref()).await?;
            if stack.len < PAGE_SIZE * 2 || stack.flags != 0 {
                return Err(EINVAL);
            }
            Some(stack)
        } else {
            None
        };
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
        let set = set.read(ts.virt.as_ref()).await?;
        let current = ts.sig_mask;

        if !old.is_null() {
            old.write(ts.virt.as_ref(), current).await?;
        }
        ts.sig_mask = match how {
            SIG_BLOCK => current | set,
            SIG_UNBLOCK => current & !set,
            SIG_SETMASK => set,
            _ => return Err(EINVAL),
        };
        Ok(())
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}
