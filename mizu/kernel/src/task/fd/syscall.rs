use alloc::{boxed::Box, vec, vec::Vec};
use core::{
    alloc::Layout,
    mem::{self, MaybeUninit},
    pin::Pin,
    time::Duration,
};

use afat32::NullTimeProvider;
use arsc_rs::Arsc;
use co_trap::UserCx;
use futures_util::{
    stream::{self, FuturesUnordered},
    FutureExt, StreamExt, TryStreamExt,
};
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, *},
};
use ktime::{Instant, TimeOutExt};
use rand_riscv::RandomState;
use sygnal::SigSet;
use umifs::types::{FileType, Metadata, OpenOptions, Permissions};
use umio::SeekFrom;

use super::Files;
use crate::{
    mem::{In, InOut, Out, UserBuffer, UserPtr},
    syscall::{ffi::Ts, ScRet},
    task::{
        fd::{FdInfo, SavedNextDirent},
        yield_now, TaskState,
    },
};

#[async_handler]
pub async fn read(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, mut buffer, len) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_mut_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.read(&mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn write(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write(&mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pread(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, mut buffer, len, offset) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_mut_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.read_at(offset, &mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pwrite(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, buffer, len, offset) = cx.args();
    let fut = async move {
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write_at(offset, &mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IoVec {
    buffer: UserBuffer,
    len: usize,
}
const MAX_IOV_LEN: usize = 8;

#[async_handler]
pub async fn readv(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;
        let virt = ts.virt.as_ref();
        let mut bufs = stream::iter(iov_buf[..vlen].iter_mut())
            .then(|iov| iov.buffer.as_mut_slice(virt, iov.len))
            .try_fold(Vec::new(), |mut acc, mut iov| async move {
                acc.append(&mut iov);
                Ok(acc)
            })
            .await?;

        io.read(&mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn writev(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;
        let virt = ts.virt.as_ref();
        let mut bufs = stream::iter(iov_buf[..vlen].iter())
            .then(|iov| iov.buffer.as_slice(virt, iov.len))
            .try_fold(Vec::new(), |mut acc, mut iov| async move {
                acc.append(&mut iov);
                Ok(acc)
            })
            .await?;

        io.write(&mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn preadv(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen, offset) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;
        let virt = ts.virt.as_ref();
        let mut bufs = stream::iter(iov_buf[..vlen].iter_mut())
            .then(|iov| iov.buffer.as_mut_slice(virt, iov.len))
            .try_fold(Vec::new(), |mut acc, mut iov| async move {
                acc.append(&mut iov);
                Ok(acc)
            })
            .await?;

        io.read_at(offset, &mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn pwritev(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<IoVec, In>, usize, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (fd, iov, vlen, offset) = cx.args();
    let fut = async move {
        if vlen == 0 {
            return Ok(0);
        }
        let vlen = vlen.min(MAX_IOV_LEN);
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        let mut iov_buf = [Default::default(); MAX_IOV_LEN];
        iov.read_slice(ts.virt.as_ref(), &mut iov_buf[..vlen])
            .await?;
        let virt = ts.virt.as_ref();
        let mut bufs = stream::iter(iov_buf[..vlen].iter())
            .then(|iov| iov.buffer.as_slice(virt, iov.len))
            .try_fold(Vec::new(), |mut acc, mut iov| async move {
                acc.append(&mut iov);
                Ok(acc)
            })
            .await?;

        io.write_at(offset, &mut bufs).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn lseek(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, isize, isize) -> Result<usize, Error>>,
) -> ScRet {
    const SEEK_SET: isize = 0;
    const SEEK_CUR: isize = 1;
    const SEEK_END: isize = 2;

    let (fd, offset, whence) = cx.args();
    let fut = async move {
        let whence = match whence {
            SEEK_SET => SeekFrom::Start(offset as usize),
            SEEK_CUR => SeekFrom::Current(offset),
            SEEK_END => SeekFrom::End(offset),
            _ => return Err(EINVAL),
        };
        let entry = ts.files.get(fd).await?;
        let io = entry.to_io().ok_or(EISDIR)?;
        io.seek(whence).await
    };
    cx.ret(fut.await);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn sendfile(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, i32, UserPtr<usize, InOut>, usize) -> Result<usize, Error>>,
) -> ScRet {
    let (output, input, mut offset_ptr, mut count) = cx.args();
    let fut = async move {
        let output = ts.files.get(output).await?.to_io().ok_or(EISDIR)?;
        let input = ts.files.get(input).await?.to_io().ok_or(EISDIR)?;

        let mut buf = vec![0; count.min(MAX_PATH_LEN)];
        if offset_ptr.is_null() {
            let mut ret = 0;
            while count > 0 {
                let len = count.min(MAX_PATH_LEN);
                let read_len = input.read(&mut [&mut buf[..len]]).await?;
                let written_len = output.write(&mut [&buf[..read_len]]).await?;
                ret += written_len;
                count -= written_len;

                if written_len == 0 || written_len < read_len {
                    break;
                }
            }
            Ok(ret)
        } else {
            let mut offset = offset_ptr.read(ts.virt.as_ref()).await?;
            let mut ret = 0;
            while count > 0 {
                let len = count.min(MAX_PATH_LEN);
                let read_len = input.read_at(offset, &mut [&mut buf[..len]]).await?;
                let written_len = output.write(&mut [&buf[..read_len]]).await?;

                offset += written_len;
                ret += written_len;
                count -= written_len;
                if written_len == 0 {
                    break;
                }
            }
            offset_ptr.write(ts.virt.as_ref(), offset).await?;
            Ok(ret)
        }
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

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

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct Kstat {
    dev: u64,
    inode: u64,
    mode: i32,
    link_count: u32,
    uid: u32,
    gid: u32,
    rdev: u64,
    __pad: u64,
    size: usize,
    blksize: u32,
    __pad2: u32,
    blocks: u64,
    atime: Ts,
    mtime: Ts,
    ctime: Ts,
    __pad3: [u32; 2],
}

impl From<Metadata> for Kstat {
    fn from(metadata: Metadata) -> Self {
        fn mode(ty: FileType, perm: Permissions) -> i32 {
            perm.bits() as i32 | ((ty.bits() as i32) << 12)
        }

        fn time(i: Option<Instant>) -> Ts {
            i.map_or(Default::default(), Into::into)
        }

        Kstat {
            dev: 1,
            inode: metadata.offset,
            mode: mode(metadata.ty, metadata.perm),
            link_count: 1,
            size: metadata.len,
            blksize: metadata.block_size as u32,
            blocks: metadata.block_count as u64,
            atime: time(metadata.last_access),
            mtime: time(metadata.last_modified),
            ctime: time(metadata.last_created),
            ..Default::default()
        }
    }
}

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

pub const MAX_PATH_LEN: usize = 256;

fssc!(
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

    pub async fn openat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        options: i32,
        perm: u32,
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        let options = OpenOptions::from_bits_truncate(options);
        let perm = Permissions::from_bits_truncate(perm);

        log::trace!(
            "user openat fd = {fd}, path = {path:?}, options = {options:?}, perm = {perm:?}"
        );

        let entry = if root {
            crate::fs::open(path, options, perm).await?.0
        } else {
            let base = files.get(fd).await?;
            match base.open(path, options, perm).await {
                Ok((entry, _)) => entry,
                Err(ENOENT) if files.cwd() == "" => crate::fs::open(path, options, perm).await?.0,
                Err(err) => return Err(err),
            }
        };
        let close_on_exec = options.contains(OpenOptions::CLOEXEC);
        files.open(entry, perm, close_on_exec).await
    }

    pub async fn faccessat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        options: i32,
        perm: u32,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        let options = OpenOptions::from_bits_truncate(options);
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;

        log::trace!(
            "user accessat fd = {fd}, path = {path:?}, options = {options:?}, perm = {perm:?}"
        );

        if root {
            crate::fs::open(path, options, perm).await?;
        } else {
            let base = files.get(fd).await?;
            match base.open(path, options, perm).await {
                Err(ENOENT) if files.cwd() == "" => crate::fs::open(path, options, perm).await?,
                res => res?,
            };
        };
        Ok(())
    }

    pub async fn mkdirat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        perm: u32,
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;

        log::trace!("user mkdir fd = {fd}, path = {path:?}, perm = {perm:?}");

        let (entry, created) = if root {
            crate::fs::open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm).await?
        } else {
            let base = files.get(fd).await?;
            base.open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm)
                .await?
        };
        if !created {
            return Err(EEXIST);
        }
        files.open(entry, perm, false).await
    }

    pub async fn fstat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        out: UserPtr<Kstat, Out>,
    ) -> Result<(), Error> {
        let file = files.get(fd).await?;
        let metadata = file.metadata().await;
        out.write(virt, metadata.into()).await
    }

    pub async fn fstatat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        out: UserPtr<Kstat, Out>,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        log::trace!("user fstatat fd = {fd}, path = {path:?}");

        let file = if root {
            crate::fs::open(
                path,
                OpenOptions::RDONLY,
                Permissions::all_same(true, false, false),
            )
            .await?
            .0
        } else {
            let base = files.get(fd).await?;
            if path == "" {
                base
            } else {
                base.open(
                    path,
                    OpenOptions::RDONLY,
                    Permissions::all_same(true, false, false),
                )
                .await?
                .0
            }
        };
        let metadata = file.metadata().await;
        out.write(virt, metadata.into()).await
    }

    pub async fn utimensat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        times: UserPtr<Ts, In>,
    ) -> Result<(), Error> {
        const UTIME_NOW: u64 = 0x3fffffff;
        const UTIME_OMIT: u64 = 0x3ffffffe;

        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        let file = if root {
            crate::fs::open(
                path,
                OpenOptions::WRONLY,
                Permissions::all_same(true, false, false),
            )
            .await?
            .0
        } else {
            let base = files.get(fd).await?;
            if path == "" {
                base
            } else {
                base.open(
                    path,
                    OpenOptions::WRONLY,
                    Permissions::all_same(true, false, false),
                )
                .await?
                .0
            }
        };

        let now = Instant::now();
        let (a, m) = if times.is_null() {
            (Some(now), Some(now))
        } else {
            let mut buf = [Ts::default(); 2];
            times.read_slice(virt, &mut buf).await?;
            let [a, m] = buf;
            let a = match a.nsec {
                UTIME_NOW => Some(now),
                UTIME_OMIT => None,
                _ => Some(a.into()),
            };
            let m = match m.nsec {
                UTIME_NOW => Some(now),
                UTIME_OMIT => None,
                _ => Some(m.into()),
            };
            (a, m)
        };
        file.set_times(None, m, a).await;
        Ok(())
    }

    pub async fn getdents64(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        ptr: UserPtr<u8, Out>,
        len: usize,
    ) -> Result<usize, Error> {
        log::trace!("user getdents64 fd = {fd}, ptr = {ptr:?}, len = {len}");

        #[repr(C, packed)]
        struct D {
            inode: u64,
            offset: u64,
            reclen: u16,
            ty: FileType,
        }
        let FdInfo {
            entry,
            saved_next_dirent,
            ..
        } = files.get_fi(fd).await?;

        let dir = entry.to_dir().ok_or(ENOTDIR)?;

        let first = match saved_next_dirent {
            SavedNextDirent::Start => None,
            SavedNextDirent::Next(dirent) => Some(dirent),
            SavedNextDirent::End => return Ok(0),
        };
        let mut d = dir.next_dirent(first.as_ref()).await?;
        let mut read_len = 0;
        loop {
            let Some(entry) = &d else { break };

            let layout = Layout::new::<D>()
                .extend_packed(Layout::for_value(&*entry.name))?
                .extend_packed(Layout::new::<u8>())?
                .pad_to_align();
            if layout.size() > len {
                break;
            }
            let Ok(reclen) = layout.size().try_into() else {
                break;
            };

            let mut out = MaybeUninit::<D>::uninit();
            out.write(D {
                inode: rand_riscv::seed64(),
                offset: entry.metadata.offset,
                reclen,
                ty: entry.metadata.ty,
            });
            ptr.write_slice(virt, unsafe { mem::transmute(out.as_bytes()) }, false)
                .await?;
            ptr.advance(mem::size_of::<D>());
            ptr.write_slice(virt, entry.name.as_bytes(), true).await?;

            ptr.advance(layout.size() - mem::size_of::<D>());
            len -= layout.size();
            read_len += layout.size();

            d = dir.next_dirent(Some(entry)).await?;
        }

        let s = match d {
            None => SavedNextDirent::End,
            Some(dirent) => SavedNextDirent::Next(dirent),
        };
        files.set_fi(fd, None, None, Some(s)).await?;

        Ok(read_len)
    }

    pub async fn renameat(
        virt: Pin<&Virt>,
        files: &Files,
        src: i32,
        src_path: UserPtr<u8, In>,
        dst: i32,
        dst_path: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let [mut src_buf, mut dst_buf] = [[0; MAX_PATH_LEN]; 2];
        let (src_path, _) = src_path.read_path(virt, &mut src_buf).await?;
        let (dst_path, _) = dst_path.read_path(virt, &mut dst_buf).await?;

        log::trace!("user renameat src = {src}/{src_path:?}, dst = {dst}/{dst_path:?}");

        let src = files.get(src).await?.to_dir_mut().ok_or(ENOTDIR)?;
        let dst = files.get(dst).await?.to_dir_mut().ok_or(ENOTDIR)?;

        src.rename(src_path, dst, dst_path).await?;
        Ok(())
    }

    pub async fn unlinkat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        flags: i32,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;

        log::trace!("user unlinkat fd = {fd}, path = {path:?}, flags = {flags}");

        if root {
            crate::fs::unlink(path).await
        } else {
            let base = files.get(fd).await?;
            let base = base.to_dir_mut().ok_or(ENOTDIR)?;
            match base.unlink(path, (flags != 0).then_some(true)).await {
                Ok(()) => Ok(()),
                Err(ENOENT) if files.cwd() == "" => crate::fs::unlink(path).await,
                Err(err) => Err(err),
            }
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

    pub async fn mount(
        virt: Pin<&Virt>,
        files: &Files,
        src: UserPtr<u8, In>,
        dst: UserPtr<u8, In>,
        ty: UserPtr<u8, In>,
        _flags: usize,
        _data: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let mut src_buf = [0; MAX_PATH_LEN];
        let mut dst_buf = [0; MAX_PATH_LEN];
        let mut ty_buf = [0; 64];
        let (src, root_src) = src.read_path(virt, &mut src_buf).await?;
        let (dst, root_dst) = dst.read_path(virt, &mut dst_buf).await?;
        let ty = ty.read_str(virt, &mut ty_buf).await?;

        let (src, _) = if root_src {
            crate::fs::open(
                src,
                Default::default(),
                Permissions::all_same(true, true, true),
            )
            .await?
        } else {
            crate::fs::open(
                &files.cwd().join(src),
                Default::default(),
                Permissions::all_same(true, true, true),
            )
            .await?
        };
        if root_dst {
            crate::fs::open_dir(dst, Default::default(), Default::default()).await?;
        } else {
            crate::fs::open_dir(
                &files.cwd().join(dst),
                Default::default(),
                Default::default(),
            )
            .await?;
        }

        let metadata = src.metadata().await;
        if metadata.ty != FileType::BLK {
            return Err(ENOTBLK);
        }
        let Some(io) = src.to_io() else {
            return Err(ENOTBLK)
        };

        if ty == "vfat" {
            let fatfs =
                afat32::FatFileSystem::new(io, metadata.block_size.ilog2(), NullTimeProvider)
                    .await?;
            crate::fs::mount(dst.to_path_buf(), "<UNKNOWN>".into(), fatfs);
        } else {
            return Err(ENODEV);
        }

        Ok(())
    }

    pub async fn umount(
        virt: Pin<&Virt>,
        files: &Files,
        target: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let mut buf = [9; MAX_PATH_LEN];
        let (target, root) = target.read_path(virt, &mut buf).await?;
        if root {
            crate::fs::unmount(target);
        } else {
            crate::fs::unmount(&files.cwd().join(target));
        }
        Ok(())
    }

    pub async fn statfs(
        virt: Pin<&Virt>,
        files: &Files,
        path: UserPtr<u8, In>,
        out: UserPtr<u64, Out>,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let (path, root) = path.read_path(virt, &mut buf).await?;
        let fs = if root {
            crate::fs::get(path).ok_or(EINVAL)?.0
        } else {
            crate::fs::get(&files.cwd().join(path)).ok_or(EINVAL)?.0
        };
        let hasher = RandomState::new();
        let stat = fs.stat().await;
        let fsid = Arsc::as_ptr(&fs) as *const () as _;
        out.write_slice(
            virt,
            &[
                hasher.hash_one(stat.ty),
                stat.block_size as u64,
                stat.block_count as u64,
                stat.block_free as u64,
                stat.block_free as u64,
                stat.file_count as u64,
                0xdeadbeef,
                fsid,
                i16::MAX as _,
                0xbeef,
                0xbeef,
                0,
                0,
                0,
                0,
            ],
            false,
        )
        .await
    }

    pub async fn ioctl(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<(), Error> {
        files.get(fd).await?;
        Ok(())
    }
);

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct PollFd {
    fd: i32,
    events: umio::Event,
    revents: umio::Event,
}

async fn poll_fds(
    pfd: &mut [PollFd],
    files: &Files,
    timeout: Option<Duration>,
) -> Result<usize, Error> {
    let files = stream::iter(&*pfd)
        .then(|pfd| files.get(pfd.fd))
        .try_collect::<Vec<_>>()
        .await?;

    pfd.iter_mut()
        .for_each(|pfd| pfd.revents = umio::Event::empty());

    let iter = files.iter().zip(&*pfd).enumerate();
    let events = iter.map(|(index, (e, p))| e.event(p.events).map(move |e| (index, e)));
    let mut events = events.collect::<FuturesUnordered<_>>();

    let mut count = 0;
    match timeout {
        Some(Duration::ZERO) => loop {
            let next = ksync::poll_once(events.next()).flatten();
            let Some((index, event)) = next else { break };
            if let Some(event) = event {
                log::trace!("PFD fd = {}, event = {event:?}", pfd[index].fd);
                pfd[index].revents |= event;
                count += 1;
            }
        },
        Some(timeout) => {
            let ddl = Instant::now() + timeout;
            loop {
                let next = events.next().on_timeout(ddl, || None).await;
                let Some((index, event)) = next else { break };
                if let Some(event) = event {
                    log::trace!("PFD fd = {}, event = {event:?}", pfd[index].fd);
                    pfd[index].revents |= event;
                    count += 1;
                }
            }
        }
        None => loop {
            let Some((index, event)) = events.next().await else { break };
            if let Some(event) = event {
                log::trace!("PFD fd = {}, event = {event:?}", pfd[index].fd);
                pfd[index].revents |= event;
                count += 1;
            }
        },
    }
    Ok(count)
}

#[async_handler]
pub async fn ppoll(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(
            UserPtr<PollFd, InOut>,
            usize,
            UserPtr<Ts, In>,
            UserPtr<SigSet, In>,
            usize,
        ) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (mut poll_fd, len, timeout, _sigmask, sigmask_size) = cx.args();
    let fut = async {
        if sigmask_size != mem::size_of::<SigSet>() {
            return Err(EINVAL);
        }
        if len > ts.files.get_limit() {
            return Err(EINVAL);
        }
        let timeout = if timeout.is_null() {
            None
        } else {
            Some(timeout.read(ts.virt.as_ref()).await?.into())
        };

        let mut pfd = vec![PollFd::default(); len];
        poll_fd.read_slice(ts.virt.as_ref(), &mut pfd).await?;

        let count = poll_fds(&mut pfd, &ts.files, timeout).await?;

        poll_fd.write_slice(ts.virt.as_ref(), &pfd, false).await?;
        Ok(count)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

const FD_SET_BITS: usize = usize::BITS as usize;

fn push_pfd(pfd: &mut Vec<PollFd>, fd_set: &[usize], events: umio::Event) {
    for (index, mut fds) in fd_set.iter().copied().enumerate() {
        let base = (index * FD_SET_BITS) as i32;
        while fds > 0 {
            let mask = fds & (!fds + 1);
            let fd = mask.trailing_zeros() as i32 + base;
            pfd.push(PollFd {
                fd,
                events,
                revents: Default::default(),
            });
            fds -= mask;
        }
    }
}

fn write_fd_set(pfd: &[PollFd], fd_set: &mut [usize], events: umio::Event) {
    fd_set.fill(0);
    for pfd in pfd {
        if pfd.revents.contains(events) {
            let index = pfd.fd as usize / FD_SET_BITS;
            let mask = 1 << (pfd.fd as usize % FD_SET_BITS);
            fd_set[index] |= mask;
        }
    }
}

#[async_handler]
pub async fn pselect(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(
            usize,
            UserPtr<usize, InOut>,
            UserPtr<usize, InOut>,
            UserPtr<usize, InOut>,
            UserPtr<Ts, In>,
            UserPtr<SigSet, In>,
        ) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (count, mut rd, mut wr, mut ex, timeout, _sigmask) = cx.args();
    let fut = async {
        if count > ts.files.get_limit() {
            return Err(EINVAL);
        }

        let timeout = if timeout.is_null() {
            None
        } else {
            Some(timeout.read(ts.virt.as_ref()).await?.into())
        };
        if count == 0 {
            match timeout {
                Some(Duration::ZERO) => yield_now().await,
                Some(timeout) => ktime::sleep(timeout).await,
                _ => {}
            }
            return Ok(0);
        }

        let len = (count + mem::size_of::<usize>() - 1) / mem::size_of::<usize>();
        let mut buf = vec![0; len];

        let mut pfd = Vec::new();
        if !rd.is_null() {
            rd.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::READABLE);
        }
        if !wr.is_null() {
            wr.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::WRITABLE);
        }
        if !ex.is_null() {
            ex.read_slice(ts.virt.as_ref(), &mut buf).await?;
            push_pfd(&mut pfd, &buf, umio::Event::EXCEPTION);
        }

        let count = poll_fds(&mut pfd, &ts.files, timeout).await?;

        if !rd.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::READABLE);
            rd.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }
        if !wr.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::WRITABLE);
            wr.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }
        if !ex.is_null() {
            write_fd_set(&pfd, &mut buf, umio::Event::EXCEPTION);
            ex.write_slice(ts.virt.as_ref(), &buf, false).await?;
        }

        Ok(count)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
