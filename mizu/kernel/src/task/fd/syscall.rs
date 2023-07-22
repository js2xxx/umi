mod fs;
mod io;
mod net;

use alloc::boxed::Box;

use co_trap::UserCx;
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, *},
};
use umifs::types::{OpenOptions, Permissions};

pub use self::{fs::*, io::*, net::*};
use super::Files;
use crate::{
    mem::{In, Out, UserPtr},
    syscall::ScRet,
    task::{fd::FdInfo, TaskState},
};

#[async_handler]
pub async fn readlinkat(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u8, In>, UserPtr<u8, Out>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, path, mut out, len) = cx.args();
    let fut = async move {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(&ts.virt, &mut buf).await?;

        let options = OpenOptions::RDONLY;
        let perm = Default::default();

        log::trace!("user readlinkat fd = {fd}, path = {path:?}");

        if root && path == "/proc/self/exe" {
            let executable = ksync::critical(|| ts.task.executable.lock().clone());
            if executable.len() + 1 >= len {
                return Err(ENAMETOOLONG);
            }
            out.write_slice(&ts.virt, executable.as_bytes(), true)
                .await?;
            return Ok(executable.len());
        }

        let _entry = if root {
            crate::fs::open(path, options, perm).await?.0
        } else {
            let base = ts.files.get(fd).await?;
            match base.open(path, options, perm).await {
                Ok((entry, _)) => entry,
                Err(ENOENT) if ts.files.cwd() == "" => {
                    crate::fs::open(path, options, perm).await?.0
                }
                Err(err) => return Err(err),
            }
        };
        Err(EINVAL)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

pub const MAX_PATH_LEN: usize = 256;

#[async_handler]
pub async fn chdir(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, In>) -> Result<(), Error>>,
) -> ScRet {
    let path = cx.args();
    let fut = async {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(&ts.virt, &mut buf).await?;

        log::trace!("user chdir path = {path:?}");
        if root {
            crate::fs::open_dir(path, OpenOptions::RDONLY, Permissions::SELF_R).await?;

            ts.files.chdir(path).await;
        } else {
            let path = ts.files.cwd().join(path);
            crate::fs::open_dir(&path, OpenOptions::RDONLY, Permissions::SELF_R).await?;
            ts.files.chdir(&path).await;
        }
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn getcwd(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, Out>, usize) -> Result<UserPtr<u8, Out>, Error>>,
) -> ScRet {
    let (mut buf, len) = cx.args();
    let fut = async {
        log::trace!("user getcwd buf = {buf:?}, len = {len}");

        let cwd = ts.files.cwd();
        let path = cwd.as_str().as_bytes();
        if path.len() >= len {
            Err(ERANGE)
        } else {
            buf.write_slice(&ts.virt, path, true).await?;
            Ok(buf)
        }
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn dup(ts: &mut TaskState, cx: UserCx<'_, fn(i32) -> Result<i32, Error>>) -> ScRet {
    let fd = cx.args();
    log::trace!("user dup fd = {fd}");

    cx.ret(ts.files.dup(fd, None).await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn dup3(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, i32, i32) -> Result<i32, Error>>,
) -> ScRet {
    let (old, new, flags) = cx.args();
    let fut = async {
        log::trace!("user dup old = {old}, new = {new}, flags = {flags}");

        let mut fi = ts.files.get_fi(old).await?;
        fi.close_on_exec = flags != 0;
        ts.files.reopen(new, fi).await;
        Ok(new)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn fcntl(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, usize, usize) -> Result<i32, Error>>,
) -> ScRet {
    let (fd, cmd, arg) = cx.args();
    let fut = async {
        const DUPFD: usize = 0;
        const GETFD: usize = 1;
        const SETFD: usize = 2;
        const GETFL: usize = 3;
        const SETFL: usize = 4;
        const DUPFD_CLOEXEC: usize = 1030;

        match cmd {
            DUPFD => ts.files.dup(fd, None).await,
            DUPFD_CLOEXEC => ts.files.dup(fd, Some(arg != 0)).await,

            GETFD => ts.files.get_fi(fd).await.map(|fi| fi.close_on_exec as i32),
            SETFD => {
                let set = |fi: &mut FdInfo| fi.close_on_exec = arg != 0;
                ts.files.set_fi(fd, set).await.map(|_| 0)
            }

            GETFL => {
                let res = ts.files.get_fi(fd).await;
                res.map(|fi| (fi.nonblock as i32) << OpenOptions::NONBLOCK.bits().ilog2())
            }
            SETFL => {
                let set = |fi: &mut FdInfo| fi.nonblock = arg != 0;
                ts.files.set_fi(fd, set).await.map(|_| 0)
            }
            _ => Err(EINVAL),
        }
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn close(ts: &mut TaskState, cx: UserCx<'_, fn(i32) -> Result<(), Error>>) -> ScRet {
    let fd = cx.args();
    log::trace!("user close fd = {fd}");

    cx.ret(ts.files.close(fd).await);
    ScRet::Continue(None)
}

async fn pipe_inner(files: &Files, virt: &Virt, mut fd: UserPtr<i32, Out>) -> Result<(), Error> {
    let (tx, rx) = crate::fs::pipe();

    let tx = crate::task::fd::FdInfo {
        entry: tx,
        close_on_exec: false,
        nonblock: false,
        perm: Permissions::SELF_W,
        saved_next_dirent: Default::default(),
    };
    let tx = files.open(tx).await?;

    let rx = crate::task::fd::FdInfo {
        entry: rx,
        close_on_exec: false,
        nonblock: false,
        perm: Permissions::SELF_R,
        saved_next_dirent: Default::default(),
    };
    let rx = files.open(rx).await?;

    fd.write_slice(virt, &[rx, tx], false).await
}

#[async_handler]
pub async fn pipe(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<i32, Out>) -> Result<(), Error>>,
) -> ScRet {
    let fut = pipe_inner(&ts.files, &ts.virt, cx.args());
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn socket_pair(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(usize, usize, usize, UserPtr<i32, Out>) -> Result<(), Error>>,
) -> ScRet {
    let fut = pipe_inner(&ts.files, &ts.virt, cx.args().3);
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn ioctl(ts: &mut TaskState, cx: UserCx<'_, fn(i32) -> Result<(), Error>>) -> ScRet {
    let fd = cx.args();
    let fut = async {
        ts.files.get(fd).await?;
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
