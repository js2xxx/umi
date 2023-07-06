macro_rules! fssc {
    (
        $(pub async fn $name:ident(
            $virt:ident: Pin<&Virt>,
            $files:ident: &Files,
            $($arg_name:ident : $arg_ty:ty),* $(,)?
        ) -> $out:ty $body:block)*
    ) => {
        $(
            #[async_handler]
            pub async fn $name(
                ts: &mut TaskState,
                cx: UserCx<'_, fn($($arg_ty),*) -> $out>,
            ) -> ScRet {
                #[allow(unused_mut, unused_parens)]
                async fn inner(
                    $virt: Pin<&Virt>,
                    $files: &Files,
                    ($(mut $arg_name),*): ($($arg_ty),*),
                ) -> $out $body

                let ret = inner(ts.virt.as_ref(), &ts.files, cx.args()).await;
                cx.ret(ret);

                ScRet::Continue(None)
            }
        )*
    };
}

mod fs;
mod io;
mod net;

use alloc::boxed::Box;
use core::pin::Pin;

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
    task::TaskState,
};

#[async_handler]
pub async fn readlinkat(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u8, In>, UserPtr<u8, Out>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, path, mut out, len) = cx.args();
    let fut = async move {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(ts.virt.as_ref(), &mut buf).await?;

        let options = OpenOptions::RDONLY;
        let perm = Default::default();

        log::trace!("user readlinkat fd = {fd}, path = {path:?}");

        if root && path == "/proc/self/exe" {
            let executable = ksync::critical(|| ts.task.executable.lock().clone());
            if executable.len() + 1 >= len {
                return Err(ENAMETOOLONG);
            }
            out.write_slice(ts.virt.as_ref(), executable.as_bytes(), true)
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

fssc! {
    pub async fn chdir(
        virt: Pin<&Virt>,
        files: &Files,
        path: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        log::trace!("user chdir path = {path:?}");
        if root {
            crate::fs::open_dir(path, OpenOptions::RDONLY, Permissions::SELF_R).await?;

            files.chdir(path).await;
        } else {
            let path = files.cwd().join(path);
            crate::fs::open_dir(&path, OpenOptions::RDONLY, Permissions::SELF_R).await?;
            files.chdir(&path).await;
        }
        Ok(())
    }

    pub async fn getcwd(
        virt: Pin<&Virt>,
        files: &Files,
        buf: UserPtr<u8, Out>,
        len: usize,
    ) -> Result<UserPtr<u8, Out>, Error> {
        log::trace!("user getcwd buf = {buf:?}, len = {len}");

        let cwd = files.cwd();
        let path = cwd.as_str().as_bytes();
        if path.len() >= len {
            Err(ERANGE)
        } else {
            buf.write_slice(virt, path, true).await?;
            Ok(buf)
        }
    }

    pub async fn dup(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<i32, Error> {
        log::trace!("user dup fd = {fd}");

        files.dup(fd, None).await
    }

    pub async fn dup3(
        _v: Pin<&Virt>,
        files: &Files,
        old: i32,
        new: i32,
        flags: i32,
    ) -> Result<i32, Error> {
        log::trace!("user dup old = {old}, new = {new}, flags = {flags}");

        let fi = files.get_fi(old).await?;
        files.reopen(new, fi.entry, fi.perm, flags != 0).await;
        Ok(new)
    }

    pub async fn fcntl(
        _v: Pin<&Virt>,
        files: &Files,
        fd: i32,
        cmd: usize,
        arg: usize,
    ) -> Result<i32, Error> {
        const DUPFD: usize = 0;
        const GETFD: usize = 1;
        const SETFD: usize = 2;
        const GETFL: usize = 3;
        const SETFL: usize = 4;
        const DUPFD_CLOEXEC: usize = 1030;

        match cmd {
            DUPFD => files.dup(fd, None).await,
            DUPFD_CLOEXEC => files.dup(fd, Some(arg != 0)).await,
            GETFD => files.get_fi(fd).await.map(|fi| fi.close_on_exec as i32),
            SETFD => {
                let c = arg != 0;
                files.set_fi(fd, Some(c), None, None).await.map(|_| 0)
            }
            GETFL => files.get_fi(fd).await.map(|fi| fi.perm.bits() as _),
            SETFL => {
                let perm = Permissions::from_bits_truncate(arg as u32);
                files.set_fi(fd, None, Some(perm), None).await.map(|_| 0)
            }
            _ => Err(EINVAL),
        }
    }

    pub async fn close(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<(), Error> {
        log::trace!("user close fd = {fd}");

        files.close(fd).await
    }

    pub async fn pipe(virt: Pin<&Virt>, files: &Files, fd: UserPtr<i32, Out>) -> Result<(), Error> {
        let (tx, rx) = crate::fs::pipe();
        let tx = files.open(tx, Permissions::SELF_W, false).await?;
        let rx = files.open(rx, Permissions::SELF_R, false).await?;
        fd.write_slice(virt, &[rx, tx], false).await
    }

    pub async fn ioctl(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<(), Error> {
        files.get(fd).await?;
        Ok(())
    }
}
