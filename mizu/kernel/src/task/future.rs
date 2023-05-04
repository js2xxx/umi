use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use co_trap::{FastResult, TrapFrame};
use kmem::Virt;
use ksc::ENOSYS;
use pin_project::pin_project;
use riscv::register::scause::{Exception, Interrupt, Scause, Trap};
use sygnal::{ActionType, Sig, SigInfo};

use super::TaskState;

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

        if let Err(sig) = handle_scause(scause, &mut ts, &mut tf).await {
            ts.task.sig.push(sig);
        }
    }
}

async fn handle_scause(
    scause: Scause,
    ts: &mut TaskState,
    tf: &mut TrapFrame,
) -> Result<(), SigInfo> {
    match scause.cause() {
        Trap::Interrupt(intr) => match intr {
            Interrupt::SupervisorTimer => ktime::timer_tick(),
            Interrupt::SupervisorExternal => crate::dev::INTR.notify(hart_id::hart_id()),
            _ => todo!(),
        },
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
                    Ok(res) => res?,
                    Err(err) => tf.set_syscall_ret(err.into_raw()),
                }
            }
            _ => todo!(),
        },
    }
    Ok(())
}
