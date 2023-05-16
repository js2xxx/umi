use alloc::boxed::Box;
use core::{ops::ControlFlow, pin::Pin, time::Duration};

use co_trap::{TrapFrame, UserCx};
use kmem::Virt;
use ksc::{
    async_handler, AHandlers, Error,
    Scn::{self, *},
};
use ktime::{Instant, InstantExt};
use riscv::register::time;
use spin::Lazy;
use sygnal::SigInfo;

use crate::{
    mem::{In, Out, UserPtr, USER_RANGE},
    task::{
        self,
        fd::{self, MAX_FDS},
        signal, TaskState,
    },
};

pub type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);
pub type ScRet = ControlFlow<i32, Option<SigInfo>>;

pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, ScRet>> = Lazy::new(|| {
    AHandlers::new()
        // Memory management
        .map(BRK, crate::mem::brk)
        .map(MMAP, fd::mmap)
        .map(MUNMAP, fd::munmap)
        // Tasks
        .map(SCHED_YIELD, task::uyield)
        .map(GETTID, task::tid)
        .map(GETPID, task::pid)
        .map(GETPPID, task::ppid)
        .map(TIMES, task::times)
        .map(SET_TID_ADDRESS, task::set_tid_addr)
        .map(CLONE, task::clone)
        .map(WAIT4, task::waitpid)
        .map(EXIT, task::exit)
        .map(EXIT_GROUP, task::exit_group)
        .map(EXECVE, task::execve)
        // Signals
        .map(SIGALTSTACK, signal::sigaltstack)
        .map(RT_SIGPROCMASK, signal::sigprocmask)
        .map(RT_SIGACTION, signal::sigaction)
        .map(RT_SIGTIMEDWAIT, signal::sigtimedwait)
        .map(KILL, signal::kill)
        .map(RT_SIGRETURN, task::TaskState::resume_from_signal)
        // FS operations
        .map(READ, fd::read)
        .map(WRITE, fd::write)
        .map(LSEEK, fd::lseek)
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
        .map(PIPE2, fd::pipe)
        .map(MOUNT, fd::mount)
        .map(UMOUNT2, fd::umount)
        // Time
        .map(GETTIMEOFDAY, gettimeofday)
        .map(CLOCK_GETTIME, clock_gettime)
        .map(NANOSLEEP, sleep)
        // Miscellaneous
        .map(UNAME, uname)
        .map(PRLIMIT64, prlimit)
});

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Tv {
    pub sec: u64,
    pub usec: u64,
}

#[async_handler]
async fn gettimeofday(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Tv, Out>, i32) -> Result<(), Error>>,
) -> ScRet {
    let (mut out, _) = cx.args();

    let now = Instant::now();
    let (sec, usec) = now.to_su();
    let ret = out.write(ts.virt.as_ref(), Tv { sec, usec }).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
async fn clock_gettime(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, UserPtr<Tv, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (_, mut out) = cx.args();

    let now = Instant::now();
    let (sec, usec) = now.to_su();
    let ret = out.write(ts.virt.as_ref(), Tv { sec, usec }).await;
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
    cx.ret(sleep_inner(ts.virt.as_ref(), input, output).await);

    ScRet::Continue(None)
}

#[async_handler]
async fn uname(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, Out>) -> Result<(), Error>>,
) -> ScRet {
    async fn inner(virt: Pin<&Virt>, mut out: UserPtr<u8, Out>) -> Result<(), Error> {
        let names: [&str; 6] = ["mizu", "umi", "alpha", "0.1.0", "riscv qemu", ""];
        for name in names {
            out.write_slice(virt, name.as_bytes(), true).await?;
            out.advance(65);
        }
        Ok(())
    }
    let ret = inner(ts.virt.as_ref(), cx.args());
    cx.ret(ret.await);
    ScRet::Continue(None)
}

const RLIMIT_CPU: u32 = 0; // CPU time in sec
const RLIMIT_DATA: u32 = 2; // max data size
const RLIMIT_STACK: u32 = 3; // max stack size
const RLIMIT_NPROC: u32 = 6; // max number of processes
const RLIMIT_NOFILE: u32 = 7; // max number of open files
const RLIMIT_AS: u32 = 9; // address space limit

#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Rlimit {
    cur: usize,
    max: usize,
}

#[async_handler]
async fn prlimit(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(u32, UserPtr<Rlimit, In>, UserPtr<Rlimit, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (ty, new, mut old) = cx.args();
    let fut = async move {
        let (cur, max) = match ty {
            RLIMIT_AS => (USER_RANGE.len(), USER_RANGE.len()),
            RLIMIT_NPROC => (65536, 65536),
            RLIMIT_CPU => {
                let s = time::read() / config::TIME_FREQ as usize;
                (s, usize::MAX)
            }
            RLIMIT_DATA | RLIMIT_STACK => (8 * 1024 * 1024, usize::MAX),
            RLIMIT_NOFILE => (
                if new.is_null() {
                    ts.files.get_limit()
                } else {
                    ts.files.set_limit(new.read(ts.virt.as_ref()).await?.cur)
                },
                MAX_FDS,
            ),
            _ => return Ok(()),
        };
        if !old.is_null() {
            old.write(ts.virt.as_ref(), Rlimit { cur, max }).await?;
        }
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
