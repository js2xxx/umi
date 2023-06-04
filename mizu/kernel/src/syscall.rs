use alloc::boxed::Box;
use core::{ops::ControlFlow, pin::Pin, time::Duration};

use co_trap::{TrapFrame, UserCx};
use kmem::Virt;
use ksc::{
    async_handler, AHandlers,
    Error::{self, EINVAL},
    Scn::{self, *},
};
use ktime::{Instant, InstantExt};
use spin::Lazy;
use sygnal::SigInfo;

use crate::{
    mem::{In, Out, UserPtr},
    task::{self, fd, signal, TaskState},
};

pub type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);
pub type ScRet = ControlFlow<i32, Option<SigInfo>>;

pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, ScRet>> = Lazy::new(|| {
    AHandlers::new()
        // Memory management
        .map(BRK, crate::mem::brk)
        .map(FUTEX, crate::mem::futex)
        .map(GET_ROBUST_LIST, crate::mem::get_robust_list)
        .map(SET_ROBUST_LIST, crate::mem::set_robust_list)
        .map(MMAP, crate::mem::mmap)
        .map(MPROTECT, crate::mem::mprotect)
        .map(MUNMAP, crate::mem::munmap)
        .map(MEMBARRIER, crate::mem::membarrier)
        // Tasks
        .map(SCHED_YIELD, task::uyield)
        .map(GETTID, task::tid)
        .map(GETPID, task::pid)
        .map(GETPPID, task::ppid)
        .map(TIMES, task::times)
        .map(PRLIMIT64, task::prlimit)
        .map(GETRUSAGE, task::getrusage)
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
        .map(TKILL, signal::tkill)
        .map(TGKILL, signal::tgkill)
        .map(RT_SIGRETURN, task::TaskState::resume_from_signal)
        // FS operations
        .map(READ, fd::read)
        .map(WRITE, fd::write)
        .map(READV, fd::readv)
        .map(WRITEV, fd::writev)
        .map(PREAD64, fd::pread)
        .map(PWRITE64, fd::pwrite)
        .map(PREADV64, fd::preadv)
        .map(PWRITEV64, fd::pwritev)
        .map(LSEEK, fd::lseek)
        .map(PPOLL, dummy_ppoll)
        .map(SENDFILE, fd::sendfile)
        .map(CHDIR, fd::chdir)
        .map(GETCWD, fd::getcwd)
        .map(DUP, fd::dup)
        .map(DUP3, fd::dup3)
        .map(FCNTL, fd::fcntl)
        .map(OPENAT, fd::openat)
        .map(READLINKAT, fd::readlinkat)
        .map(FACCESSAT, fd::faccessat)
        .map(MKDIRAT, fd::mkdirat)
        .map(FSTAT, fd::fstat)
        .map(NEWFSTATAT, fd::fstatat)
        .map(UTIMENSAT, fd::utimensat)
        .map(GETDENTS64, fd::getdents64)
        .map(RENAMEAT2, fd::renameat)
        .map(UNLINKAT, fd::unlinkat)
        .map(CLOSE, fd::close)
        .map(PIPE2, fd::pipe)
        .map(MOUNT, fd::mount)
        .map(UMOUNT2, fd::umount)
        .map(STATFS, fd::statfs)
        .map(IOCTL, fd::ioctl)
        // Time
        .map(GETTIMEOFDAY, gettimeofday)
        .map(CLOCK_GETTIME, clock_gettime)
        .map(NANOSLEEP, sleep)
        // Miscellaneous
        .map(UNAME, uname)
        .map(GETEUID, dummy_zero)
        .map(GETEGID, dummy_zero)
        .map(GETPGID, dummy_zero)
        .map(GETUID, dummy_zero)
        .map(GETGID, dummy_zero)
});

#[async_handler]
async fn dummy_zero(_: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(0);
    ScRet::Continue(None)
}

#[async_handler]
async fn dummy_ppoll(_: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(1);
    ScRet::Continue(None)
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Tv {
    pub sec: u64,
    pub usec: u64,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Ts {
    pub sec: u64,
    pub nsec: u64,
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
    cx: UserCx<'_, fn(usize, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (_, mut out) = cx.args();

    let now = Instant::now();
    let (sec, usec) = now.to_su();
    let t = Ts {
        sec,
        nsec: usec * 1000,
    };
    let ret = out.write(ts.virt.as_ref(), t).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
async fn sleep(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Ts, In>, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    async fn sleep_inner(
        virt: Pin<&Virt>,
        input: UserPtr<Ts, In>,
        mut output: UserPtr<Ts, Out>,
    ) -> Result<(), Error> {
        let Ts { sec, nsec } = input.read(virt).await?;
        if sec >= isize::MAX as _ || nsec >= 1_000_000_000 {
            return Err(EINVAL);
        }

        let dur = Duration::from_secs(sec) + Duration::from_nanos(nsec);
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
        let names: [&str; 6] = ["mizu", "umi", "5.0.0", "23.05", "riscv", ""];
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
