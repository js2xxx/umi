mod syscall;

use alloc::{sync::Arc, vec};
use core::{mem, ops::ControlFlow, sync::atomic::Ordering::SeqCst};

use arsc_rs::Arsc;
use sygnal::{ActionType, Sig, SigInfo};

pub use self::syscall::*;
use super::{TaskEvent, TaskState};

impl TaskState {
    pub(in crate::task) async fn handle_signals(&mut self) -> ControlFlow<(i32, Sig)> {
        let si = self.task.sig.pop(self.sig_mask);
        let si = si.or_else(|| self.task.shared_sig.load(SeqCst).pop(self.sig_mask));
        if let Some(si) = si {
            let action = self.sig_actions.get(si.sig);
            match action.ty {
                ActionType::Ignore => {}
                ActionType::Resume => {
                    let _ = self.task.event.send(&TaskEvent::Continued).await;
                }
                ActionType::Kill => {
                    self.sig_fatal(si, false);
                    return ControlFlow::Break((-1, si.sig));
                }
                ActionType::Suspend => {
                    let _ = self.task.event.send(&TaskEvent::Suspended(si.sig)).await;
                    self.task.sig.wait_one(Sig::SIGCONT).await;
                }
                ActionType::User { .. } => todo!(),
            }
        }
        ControlFlow::Continue(())
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
            .filter(|t| !Arc::ptr_eq(t, &self.task))
        {
            t.sig.push(si);
        }
    }
}
