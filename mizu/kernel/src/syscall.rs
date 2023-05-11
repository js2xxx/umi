use alloc::boxed::Box;
use core::{ops::ControlFlow, pin::Pin, time::Duration};

use co_trap::{TrapFrame, UserCx};
use kmem::Virt;
use ksc::{
    async_handler, AHandlers, Error,
    Scn::{self, *},
};
use ktime::{Instant, InstantExt};
use spin::Lazy;
use sygnal::SigInfo;

use crate::{
    mem::{In, Out, UserPtr},
    task::{self, fd, TaskState},
};

pub type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);
pub type ScRet = ControlFlow<i32, Option<SigInfo>>;

// TODO: Add handlers to the static.
pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, ScRet>> = Lazy::new(|| {
    AHandlers::new()
        // Memory management
        .map(BRK, crate::mem::brk)
        .map(MMAP, fd::mmap)
        .map(MUNMAP, fd::munmap)
        // Tasks
        .map(GETPID, task::pid)
        .map(GETPPID, task::ppid)
        .map(TIMES, task::times)
        .map(EXIT, task::exit)
        // FS operations
        .map(READ, fd::read)
        .map(WRITE, fd::write)
        .map(CHDIR, fd::chdir)
        .map(GETCWD, fd::getcwd)
        .map(DUP, fd::dup)
        .map(DUP3, fd::dup3)
        .map(OPENAT, fd::openat)
        .map(MKDIRAT, fd::mkdirat)
        .map(FSTAT, fd::fstat)
        .map(GETDENTS64, fd::getdents64)
        .map(UNLINKAT, fd::unlinkat)
        .map(CLOSE, fd::close)
        // Time
        .map(GETTIMEOFDAY, gettimeofday)
        .map(NANOSLEEP, sleep)
});

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
struct Tv {
    sec: u64,
    usec: u64,
}

#[async_handler]
async fn gettimeofday(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Tv, Out>, i32) -> Result<(), Error>>,
) -> ScRet {
    let (mut out, _) = cx.args();

    let now = Instant::now();
    let (sec, usec) = now.to_su();
    let ret = out.write(ts.task.virt(), Tv { sec, usec }).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
async fn sleep(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Tv, In>, UserPtr<Tv, Out>) -> Result<(), Error>>,
) -> ScRet {
    async fn sleep_inner(
        virt: Pin<&Virt>,
        input: UserPtr<Tv, In>,
        mut output: UserPtr<Tv, Out>,
    ) -> Result<(), Error> {
        let tv = input.read(virt).await?;

        let dur = Duration::from_secs(tv.sec) + Duration::from_micros(tv.usec);
        if dur.is_zero() {
            crate::task::yield_now().await
        } else {
            ktime::sleep(dur).await;
        }

        if !output.is_null() {
            output.write(virt, Default::default()).await?;
        }

        Ok(())
    }
    let (input, output) = cx.args();
    cx.ret(sleep_inner(ts.task.virt(), input, output).await);

    ScRet::Continue(None)
}
