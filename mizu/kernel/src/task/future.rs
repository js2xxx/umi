use core::{
    future::Future,
    ops::ControlFlow::{Break, Continue},
    pin::Pin,
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use co_trap::{FastResult, TrapFrame};
use kmem::Virt;
use ksc::ENOSYS;
use pin_project::pin_project;
use riscv::register::scause::{Exception, Scause, Trap};
use sygnal::{ActionType, Sig, SigCode, SigInfo};

use super::TaskState;
use crate::syscall::ScRet;

#[pin_project]
pub struct TaskFut<F> {
    virt: Pin<Arsc<Virt>>,
    #[pin]
    fut: F,
}

impl<F> TaskFut<F> {
    pub fn new(virt: Pin<Arsc<Virt>>, fut: F) -> Self {
        TaskFut { virt, fut }
    }
}

impl<F: Future> Future for TaskFut<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { self.virt.clone().load() };
        self.project().fut.poll(cx)
    }
}

pub async fn user_loop(mut ts: TaskState, mut tf: TrapFrame) {
    loop {
        while let Some(si) = ts.task.sig.pop(ts.sig_mask) {
            let action = ts.task.sig_actions.get(si.sig);
            match action.ty {
                ActionType::Ignore | ActionType::Resume => {}
                ActionType::Kill => break,
                ActionType::Suspend => {
                    ts.task.sig.wait_one(Sig::SIGCONT).await;
                }
                ActionType::User { .. } => todo!(),
            }
        }

        let (scause, fr) = co_trap::yield_to_user(&mut tf);
        match fr {
            FastResult::Continue => {}
            FastResult::Pending => continue,
            FastResult::Break => break,
            FastResult::Yield => unreachable!(),
        }

        match handle_scause(scause, &mut ts, &mut tf).await {
            Continue(Some(sig)) => ts.task.sig.push(sig),
            Continue(None) => {}
            Break(_code) => break,
        }
    }
}

async fn handle_scause(scause: Scause, ts: &mut TaskState, tf: &mut TrapFrame) -> ScRet {
    match scause.cause() {
        Trap::Interrupt(intr) => crate::trap::handle_intr(intr, "user task"),
        Trap::Exception(excep) => match excep {
            Exception::UserEnvCall => {
                let res = async {
                    let scn = tf.scn().ok_or(ENOSYS)?;
                    crate::syscall::SYSCALL
                        .handle(scn, (ts, tf))
                        .await
                        .ok_or(ENOSYS)
                }
                .await;
                match res {
                    Ok(res) => return res,
                    Err(err) => tf.set_syscall_ret(err.into_raw()),
                }
            }
            Exception::LoadPageFault | Exception::StorePageFault => {
                return Continue(Some(SigInfo {
                    sig: Sig::SIGSEGV,
                    code: SigCode::KERNEL,
                    fields: sygnal::SigFields::SigSys {
                        addr: tf.stval.into(),
                        num: 0,
                    },
                }))
            }
            _ => todo!(),
        },
    }
    Continue(None)
}
