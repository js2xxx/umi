use core::{
    future::Future,
    ops::ControlFlow::{Break, Continue},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use arsc_rs::Arsc;
use co_trap::{FastResult, TrapFrame};
use kmem::Virt;
use ksc::ENOSYS;
use ktime::Instant;
use pin_project::pin_project;
use riscv::register::scause::{Exception, Scause, Trap};
use sygnal::{ActionType, Sig, SigCode, SigInfo};

use super::{TaskEvent, TaskState};
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

const TASK_GRAN: Duration = Duration::from_millis(1);

pub async fn user_loop(mut ts: TaskState, mut tf: TrapFrame) {
    let mut time = Instant::now();
    'life: loop {
        while let Some(si) = ts.task.sig.pop(ts.sig_mask) {
            let _ = ts.task.event.send(&TaskEvent::Signaled(si.sig)).await;
            let action = ts.task.sig_actions.get(si.sig);
            match action.ty {
                ActionType::Ignore => {}
                ActionType::Resume => {
                    let _ = ts.task.event.send(&TaskEvent::Continued).await;
                }
                ActionType::Kill => break 'life,
                ActionType::Suspend => {
                    let _ = ts.task.event.send(&TaskEvent::Suspended(si.sig)).await;
                    ts.task.sig.wait_one(Sig::SIGCONT).await;
                }
                ActionType::User { .. } => todo!(),
            }
        }

        let (scause, fr) = co_trap::yield_to_user(&mut tf);
        match fr {
            FastResult::Continue => {}
            FastResult::Pending => continue,
            FastResult::Break => break 'life,
            FastResult::Yield => unreachable!(),
        }

        match handle_scause(scause, &mut ts, &mut tf).await {
            Continue(Some(sig)) => ts.task.sig.push(sig),
            Continue(None) => {}
            Break(code) => {
                let _ = ts.task.event.send(&TaskEvent::Exited(code)).await;
                break 'life;
            }
        }

        let new_time = Instant::now();
        if new_time - time >= TASK_GRAN {
            time = new_time;
            yield_now().await
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
                    log::info!("user syscall {scn:?}");
                    crate::syscall::SYSCALL
                        .handle(scn, (ts, tf))
                        .await
                        .ok_or(ENOSYS)
                }
                .await;
                match res {
                    Ok(res) => return res,
                    Err(err) => {
                        log::warn!("error in syscall: {err}");
                        tf.set_syscall_ret(err.into_raw())
                    }
                }
            }
            Exception::InstructionPageFault
            | Exception::LoadPageFault
            | Exception::StorePageFault => {
                log::info!(
                    "user {excep:?} at {:#x}, address = {:#x}",
                    tf.sepc,
                    tf.stval
                );
                let res = ts.task.virt.commit(tf.stval.into()).await;
                if let Err(err) = res {
                    log::error!("failing to commit pages at address {:#x}: {err}", tf.stval);
                    return Continue(Some(SigInfo {
                        sig: Sig::SIGSEGV,
                        code: SigCode::KERNEL,
                        fields: sygnal::SigFields::SigSys {
                            addr: tf.stval.into(),
                            num: 0,
                        },
                    }));
                }
            }
            _ => todo!(),
        },
    }
    Continue(None)
}

pub fn yield_now() -> YieldNow {
    YieldNow(false)
}

/// Future for the [`yield_now()`] function.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
