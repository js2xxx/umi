mod syscall;

use alloc::{boxed::Box, vec};
use core::{alloc::Layout, mem, sync::atomic::Ordering::SeqCst};

use arsc_rs::Arsc;
use co_trap::TrapFrame;
use ksc::async_handler;
use rv39_paging::LAddr;
use static_assertions::const_assert;
use sygnal::{ActionType, Sig, SigCode, SigFields, SigInfo, SigSet};

pub use self::syscall::*;
use super::{TaskEvent, TaskState};
use crate::{
    mem::{In, Out, UserPtr},
    syscall::ScRet,
};

impl TaskState {
    pub(in crate::task) async fn handle_signals(
        &mut self,
        tf: &mut TrapFrame,
    ) -> Result<(), (i32, Sig)> {
        let si = self.task.sig.pop(self.sig_mask);
        let si = si.or_else(|| self.task.shared_sig.load(SeqCst).pop(self.sig_mask));
        if let Some(si) = si {
            let action = self.sig_actions.get(si.sig);
            log::trace!("received signal {:?}, code = {}", si.sig, si.code);
            match action.ty {
                ActionType::Ignore => {}
                ActionType::Resume => {
                    let _ = self.task.event.send(&TaskEvent::Continued).await;
                }
                ActionType::Kill => {
                    self.sig_fatal(si, false);
                    return Err((0, si.sig));
                }
                ActionType::Suspend => {
                    let _ = self.task.event.send(&TaskEvent::Suspended(si.sig)).await;
                    self.task.sig.wait_one(Sig::SIGCONT).await;
                }
                ActionType::User { entry, .. } => {
                    let exit = SIGRETURN_GUARD.into();
                    if let Err(sig) = self.yield_to_signal(tf, si, entry, exit).await {
                        let sigsegv = SigInfo {
                            sig: Sig::SIGSEGV,
                            code: SigCode::KERNEL as _,
                            fields: SigFields::None,
                        };
                        if sig != Sig::SIGSEGV {
                            self.task.sig.push(sigsegv)
                        } else {
                            self.sig_fatal(sigsegv, false);
                            return Err((0, Sig::SIGSEGV));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(in crate::task) fn sig_fatal(&mut self, si: SigInfo, clear: bool) {
        let tgroup = if clear {
            mem::replace(
                &mut self.tgroup,
                Arsc::new((self.task.tid, spin::RwLock::new(vec![self.task.clone()]))),
            )
        } else {
            self.tgroup.clone()
        };
        for t in ksync::critical(|| tgroup.1.read().clone())
            .into_iter()
            .filter(|t| t.tid != self.task.tid)
        {
            log::debug!("Send fatal {:?} to task {}", si.sig, t.tid);
            t.sig.push(si);
        }
    }

    async fn yield_to_signal(
        &mut self,
        tf: &mut TrapFrame,
        si: SigInfo,
        entry: LAddr,
        exit: LAddr,
    ) -> Result<(), Sig> {
        let sig_stack = self.sig_stack.take();
        let cur = match sig_stack {
            Some(s) => s.base + s.len,
            None => tf.gpr.tx.sp.into(),
        };
        let pad_uc = Layout::new::<Ucontext>().pad_to_align().size();
        let mut uc_ptr = UserPtr::<Ucontext, Out>::new(cur - pad_uc);
        let mut usi_ptr = UserPtr::<UsigInfo, Out>::new(uc_ptr.addr() - MAX_SI_LEN);

        let virt = self.virt.as_ref();

        let usi = UsigInfo {
            sig: si.sig,
            errno: 0,
            code: si.code,
        };
        usi_ptr.write(virt, usi).await.map_err(|_| si.sig)?;

        let mut uc = Ucontext {
            flags: 0,
            link: 0usize.into(),
            stack: sig_stack.unwrap_or_default(),
            sig_mask: self.sig_mask.into(),
            _rsvd: 0,
            mc: Mcontext {
                pc: tf.sepc,
                ..Default::default()
            },
        };
        tf.gpr.copy_to_x(&mut uc.mc.x);
        uc_ptr.write(virt, uc).await.map_err(|_| si.sig)?;

        tf.gpr.tx.a[0..3].copy_from_slice(&[
            si.sig.raw() as usize,
            usi_ptr.addr().val(),
            uc_ptr.addr().val(),
        ]);

        tf.sepc = entry.val();
        tf.gpr.tx.ra = exit.val();
        tf.gpr.tx.sp = usi_ptr.addr().val();

        self.sig_mask |= si.sig;
        Ok(())
    }

    #[async_handler]
    pub async fn resume_from_signal(ts: &mut TaskState, tf: &mut TrapFrame) -> ScRet {
        let uc_ptr = UserPtr::<Ucontext, In>::new((tf.gpr.tx.sp + MAX_SI_LEN).into());
        let Ok(uc) = uc_ptr.read(ts.virt.as_ref()).await else {
            tf.sepc += 4;
            return ScRet::Continue(Some(SigInfo {
                sig: Sig::SIGSEGV,
                code: SigCode::KERNEL as _,
                fields: SigFields::None,
            }));
        };

        ts.sig_mask = uc.sig_mask.into();
        ts.sig_stack = (uc.stack.len != 0).then_some(uc.stack);
        tf.sepc = uc.mc.pc;
        tf.gpr.copy_from_x(&uc.mc.x);
        ScRet::Continue(None)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct UsigInfo {
    sig: Sig,
    errno: i32,
    code: i32,
}
const MAX_SI_LEN: usize = 128;
const_assert!(mem::size_of::<UsigInfo>() <= MAX_SI_LEN);

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Ucontext {
    flags: isize,
    link: LAddr,
    stack: SigStack,
    sig_mask: PaddedSigSet,
    _rsvd: usize,
    mc: Mcontext,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct PaddedSigSet([u64; 128 / mem::size_of::<u64>()]);

impl From<SigSet> for PaddedSigSet {
    fn from(value: SigSet) -> Self {
        PaddedSigSet([value.raw(), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    }
}

impl From<PaddedSigSet> for SigSet {
    fn from(val: PaddedSigSet) -> Self {
        val.0[0].into()
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
struct Mcontext {
    pc: usize,
    x: [usize; 31],
}
