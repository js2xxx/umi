pub mod ffi;

use alloc::boxed::Box;
use core::{mem, ops::ControlFlow, time::Duration};

use co_trap::{TrapFrame, UserCx};
use kmem::Virt;
use ksc::{
    async_handler, AHandlers,
    Error::{self, EINVAL},
    Scn::{self, *},
};
use ktime::Instant;
use rand_riscv::rand_core::RngCore;
use spin::Lazy;
use sygnal::SigInfo;

use self::ffi::{Ts, Tv};
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
        .map(MSYNC, crate::mem::msync)
        .map(MADVISE, crate::mem::madvise)
        .map(MPROTECT, crate::mem::mprotect)
        .map(MUNMAP, crate::mem::munmap)
        .map(MEMBARRIER, crate::mem::membarrier)
        .map(SHMGET, crate::mem::shmget)
        .map(SHMCTL, dummy_zero)
        .map(SHMAT, crate::mem::shmat)
        .map(SHMDT, crate::mem::shmdt)
        // Tasks
        .map(SCHED_YIELD, task::uyield)
        .map(SCHED_SETSCHEDULER, dummy_zero)
        .map(SCHED_GETSCHEDULER, dummy_zero)
        .map(SCHED_GETPARAM, dummy_zero)
        .map(SCHED_SETAFFINITY, dummy_zero)
        .map(SCHED_GETAFFINITY, task::affinity)
        .map(GETTID, task::tid)
        .map(GETPID, task::pid)
        .map(GETPPID, task::ppid)
        .map(TIMES, task::times)
        .map(SETITIMER, task::setitimer)
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
        .map(RT_SIGSUSPEND, signal::sigsuspend)
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
        .map(PPOLL, fd::ppoll)
        .map(PSELECT6, fd::pselect)
        .map(SENDFILE, fd::sendfile)
        .map(SYNC, fd::sync)
        .map(FSYNC, fd::fsync)
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
        .map(FCHMOD, fd::fchmod)
        .map(FCHMODAT, fd::fchmodat)
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
        .map(FTRUNCATE, fd::ftruncate)
        .map(TRUNCATE, fd::truncate)
        // Network
        .map(SOCKET, fd::socket)
        .map(SOCKETPAIR, fd::socket_pair)
        .map(GETSOCKNAME, fd::getsockname)
        .map(GETSOCKOPT, fd::getsockopt)
        .map(SETSOCKOPT, fd::setsockopt)
        .map(GETPEERNAME, fd::getpeername)
        .map(SENDTO, fd::sendto)
        .map(RECVFROM, fd::recvfrom)
        .map(CONNECT, fd::connect)
        .map(BIND, fd::bind)
        .map(LISTEN, fd::listen)
        .map(ACCEPT, fd::accept)
        .map(SHUTDOWN, fd::shutdown)
        // Time
        .map(GETTIMEOFDAY, gettimeofday)
        .map(CLOCK_GETTIME, clock_gettime)
        .map(CLOCK_GETRES, clock_getres)
        .map(CLOCK_NANOSLEEP, clock_nanosleep)
        .map(NANOSLEEP, sleep)
        // Miscellaneous
        .map(UNAME, uname)
        .map(GETRANDOM, getrandom)
        .map(SETSID, dummy_zero)
        .map(GETEUID, dummy_zero)
        .map(GETEGID, dummy_zero)
        .map(GETPGID, dummy_zero)
        .map(GETUID, dummy_zero)
        .map(GETGID, dummy_zero)
        .map(SYSLOG, dummy_zero)
        .map(UMASK, dummy_umask)
});

#[async_handler]
async fn dummy_zero(_: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(0);
    ScRet::Continue(None)
}

#[async_handler]
async fn dummy_umask(_: &mut TaskState, cx: UserCx<'_, fn() -> usize>) -> ScRet {
    cx.ret(0o777);
    ScRet::Continue(None)
}

#[async_handler]
async fn gettimeofday(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Tv, Out>, i32) -> Result<(), Error>>,
) -> ScRet {
    let (mut out, _) = cx.args();

    let t = Instant::now().into();
    let ret = out.write(&ts.virt, t).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
async fn clock_gettime(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (_, mut out) = cx.args();

    let t = Instant::now().into();
    let ret = out.write(&ts.virt, t).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
async fn clock_getres(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (_, mut out) = cx.args();
    cx.ret(out.write(&ts.virt, Duration::from_nanos(1).into()).await);
    ScRet::Continue(None)
}

#[async_handler]
async fn clock_nanosleep(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize, UserPtr<Ts, In>, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    let (_, flags, input, mut output) = cx.args();
    let fut = async {
        let t = input.read(&ts.virt).await?;
        if t.sec >= isize::MAX as _ || t.nsec >= 1_000_000_000 {
            return Err(EINVAL);
        }

        if flags == 0 {
            let dur: Duration = t.into();
            if dur.is_zero() {
                crate::task::yield_now().await
            } else {
                ktime::sleep(dur).await;
            }
        } else {
            let inst: Instant = t.into();
            if Instant::now() < inst {
                ktime::sleep_until(inst).await;
            }
        }

        if !output.is_null() {
            output.write(&ts.virt, Default::default()).await?;
        }
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
async fn sleep(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Ts, In>, UserPtr<Ts, Out>) -> Result<(), Error>>,
) -> ScRet {
    async fn sleep_inner(
        virt: &Virt,
        input: UserPtr<Ts, In>,
        mut output: UserPtr<Ts, Out>,
    ) -> Result<(), Error> {
        let ts = input.read(virt).await?;
        if ts.sec >= isize::MAX as _ || ts.nsec >= 1_000_000_000 {
            return Err(EINVAL);
        }

        let dur: Duration = ts.into();
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
    cx.ret(sleep_inner(&ts.virt, input, output).await);

    ScRet::Continue(None)
}

#[async_handler]
async fn uname(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, Out>) -> Result<(), Error>>,
) -> ScRet {
    async fn inner(virt: &Virt, mut out: UserPtr<u8, Out>) -> Result<(), Error> {
        let names: [&str; 6] = ["mizu", "umi", "5.0.0", "23.05", "riscv", ""];
        for name in names {
            out.write_slice(virt, name.as_bytes(), true).await?;
            out.advance(65);
        }
        Ok(())
    }
    let ret = inner(&ts.virt, cx.args());
    cx.ret(ret.await);
    ScRet::Continue(None)
}

#[async_handler]
async fn getrandom(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, Out>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (mut buf, len) = cx.args();
    let fut = async {
        let mut rng = rand_riscv::rng();
        let mut rest = len;
        while rest > 0 {
            let count = rest.min(mem::size_of::<u64>());
            let data = rng.next_u64().to_le_bytes();
            buf.write_slice(&ts.virt, &data[..count], false).await?;
            buf.advance(count);
            rest -= count;
        }
        Ok(len)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
