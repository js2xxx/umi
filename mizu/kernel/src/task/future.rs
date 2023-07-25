use core::{
    future::Future,
    mem,
    ops::ControlFlow::{Break, Continue},
    pin::{pin, Pin},
    sync::atomic::Ordering::SeqCst,
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use co_trap::{FastResult, TrapFrame};
use futures_util::{
    future::{select, Either},
    FutureExt,
};
use kmem::Virt;
use ksc::{Error::EINTR, Scn, ENOSYS};
use ktime::TimeOutExt;
use pin_project::pin_project;
use riscv::register::{
    scause::{Exception, Scause, Trap},
    time,
};
use rv39_paging::Attr;
use sygnal::{Sig, SigCode, SigInfo, SigSet};

use super::TaskState;
use crate::{
    syscall::ScRet,
    task::signal::SIGRETURN_GUARD,
    trap::{Fp, FP},
};

#[pin_project]
pub struct TaskFut<F> {
    virt: Arsc<Virt>,
    fp: Fp,
    #[pin]
    fut: F,
}

impl<F> TaskFut<F> {
    pub fn new(virt: Arsc<Virt>, fut: F) -> Self {
        TaskFut {
            virt,
            fp: FP.try_with(Fp::copy).unwrap_or_default(),
            fut,
        }
    }
}

impl<F: Future> Future for TaskFut<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { self.virt.clone().load() };
        let this = self.project();
        let ret = FP.set(this.fp, || this.fut.poll(cx));
        if ret.is_pending() {
            this.fp.yield_now();
        }
        ret
    }
}

const TASK_GRAN: u64 = 20000;

pub async fn user_loop(mut ts: TaskState, mut tf: TrapFrame) {
    log::debug!("task {} startup, a0 = {}", ts.task.tid, tf.gpr.tx.a[0]);

    let mut stat_time = time::read64();
    let mut sched_time = stat_time;
    let (code, sig) = 'life: loop {
        if let Err((code, sig)) = ts.handle_signals(&mut tf).await {
            break 'life (code, Some(sig));
        }

        let sys = time::read64();
        ts.task.times.update_system(sys - stat_time);
        stat_time = sys;

        log::trace!(
            "task {} entering user cx, sepc = {:#x}",
            ts.task.tid,
            tf.sepc
        );
        if !crate::mem::USER_RANGE.contains(&tf.sepc) {
            log::warn!("ILLEGAL USER DEST: {:#x}", tf.sepc);
            break 'life (i32::MIN, Some(Sig::SIGSEGV));
        }

        let (scause, fr) = crate::trap::yield_to_user(&mut tf);

        let usr = time::read64();
        ts.task.times.update_user(usr - stat_time);
        stat_time = usr;

        match fr {
            FastResult::Continue => {}
            FastResult::Pending => continue,
            FastResult::Break => break 'life (0, None),
            FastResult::Yield => unreachable!(),
        }

        match handle_scause(scause, &mut ts, &mut tf).await {
            Continue(Some(sig)) => ts.task.sig.push(sig),
            Continue(None) => {}
            Break(code) => break 'life (code, None),
        }

        let now = time::read64();
        if now - sched_time >= TASK_GRAN {
            sched_time = now;
            log::trace!("task {} yield", ts.task.tid);
            yield_now().await;
            log::trace!("task {} yielded", ts.task.tid);
        }

        for c in ts.counters.iter_mut() {
            if let Some(si) = c.update(&ts.task.times) {
                ts.task.sig.push(si)
            }
        }
    };
    ts.cleanup(code, sig).await
}

async fn handle_scause(scause: Scause, ts: &mut TaskState, tf: &mut TrapFrame) -> ScRet {
    match scause.cause() {
        Trap::Interrupt(intr) => crate::trap::handle_intr(intr, "user task"),
        Trap::Exception(excep) => match excep {
            Exception::UserEnvCall => return ts.handle_syscall(tf).await,
            Exception::InstructionPageFault
            | Exception::LoadPageFault
            | Exception::StorePageFault => {
                log::info!(
                    "task {} {excep:?} at {:#x}, address = {:#x}",
                    ts.task.tid,
                    tf.sepc,
                    tf.stval
                );
                if tf.stval == SIGRETURN_GUARD {
                    return TaskState::resume_from_signal(ts, tf).await;
                }

                let attr = Attr::builder()
                    .readable(excep == Exception::LoadPageFault)
                    .writable(excep == Exception::StorePageFault)
                    .executable(excep == Exception::InstructionPageFault)
                    .build();

                let res = ts.virt.commit(tf.stval.into(), attr).await;
                if let Err(err) = res {
                    log::error!(
                        "task {} committing pages failed at address {:#x}: {err}",
                        ts.task.tid,
                        tf.stval
                    );
                    return Continue(Some(SigInfo {
                        sig: Sig::SIGSEGV,
                        code: SigCode::KERNEL as _,
                        fields: sygnal::SigFields::SigSys {
                            addr: tf.stval.into(),
                            num: 0,
                        },
                    }));
                }
            }
            _ => panic!(
                "task {} unhandled excep {excep:?} at {:#x}, stval = {:#x}",
                ts.task.tid, tf.sepc, tf.stval
            ),
        },
    }
    Continue(None)
}

impl TaskState {
    async fn handle_syscall(&mut self, tf: &mut TrapFrame) -> ScRet {
        let scn = match tf.scn() {
            Ok(scn) => scn,
            Err(num) => {
                log::warn!("SYSCALL not implemented: {num}");
                tf.set_syscall_ret(ENOSYS.into_raw());
                return Continue(None);
            }
        };

        if scn != Scn::WRITE || tf.syscall_arg::<0>() >= 3 {
            // Get rid of tracing writes to STDIO.
            log::info!(
                "task {} syscall {scn:?}, sepc = {:#x}",
                self.task.tid,
                tf.sepc
            );
        }

        let sig_mask = matches!(
            scn,
            Scn::KILL | Scn::TKILL | Scn::TGKILL | Scn::RT_SIGQUEUEINFO | Scn::RT_SIGSUSPEND
        )
        .then(|| mem::replace(&mut self.sig_mask, !SigSet::EMPTY));

        let next_deadline = self.counters.iter().filter_map(|c| c.next_deadline()).min();

        let syscall = async {
            let task = self.task.clone();

            let local = pin!(task.sig.wait_event(!self.sig_mask));
            let shared = task.shared_sig.load(SeqCst);
            let shared = pin!(shared.wait_event(!self.sig_mask));

            let handle = pin!(crate::syscall::SYSCALL.handle(scn, (self, &mut *tf)));

            let task = select(handle, select(local, shared)).map(|either| match either {
                Either::Left((Some(res), _)) => Ok(res),
                Either::Left((None, _)) => Err(ENOSYS),
                Either::Right(_) => Err(EINTR),
            });
            match next_deadline {
                None => task.await,
                Some(ddl) => task.on_timeout(ddl, || Err(EINTR)).await,
            }
        }
        .await;

        if let Some(sig_mask) = sig_mask {
            self.sig_mask = sig_mask;
        }

        tf.set_syscall_ret(match syscall {
            Ok(res) => return res,
            Err(ENOSYS) => {
                log::warn!("SYSCALL not implemented: {scn:?}");
                ENOSYS.into_raw()
            }
            Err(e) => e.into_raw(),
        });

        Continue(None)
    }
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
