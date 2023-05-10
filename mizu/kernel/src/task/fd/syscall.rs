use alloc::boxed::Box;
use core::{
    alloc::Layout,
    mem::{self, MaybeUninit},
    pin::Pin,
};

use co_trap::UserCx;
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, EBADF, EEXIST, ENOTDIR, EPERM, ERANGE},
};
use umifs::types::{FileType, OpenOptions, Permissions};

use super::Files;
use crate::{
    mem::{In, Out, UserBuffer, UserPtr},
    syscall::ScRet,
    task::TaskState,
};

#[async_handler]
pub async fn read(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> isize>,
) -> ScRet {
    async fn read_inner(
        ts: &mut TaskState,
        (fd, mut buffer, len): (i32, UserBuffer, usize),
    ) -> Result<usize, Error> {
        log::trace!("user read fd = {fd}, buffer = {buffer:?}, len = {len}");

        let mut bufs = buffer.as_mut_slice(ts.task.virt.as_ref(), len).await?;

        let entry = ts.task.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.read(&mut bufs).await
    }

    let ret = match read_inner(ts, cx.args()).await {
        Ok(len) => len as isize,
        Err(err) => err as isize,
    };
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn write(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> Result<usize, Error>>,
) -> ScRet {
    async fn write_inner(
        ts: &mut TaskState,
        (fd, buffer, len): (i32, UserBuffer, usize),
    ) -> Result<usize, Error> {
        log::trace!("user write fd = {fd}, buffer = {buffer:?}, len = {len}");

        let mut bufs = buffer.as_slice(ts.task.virt.as_ref(), len).await?;

        let entry = ts.task.files.get(fd).await?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write(&mut bufs).await
    }

    let ret = write_inner(ts, cx.args()).await;
    cx.ret(ret);

    ScRet::Continue(None)
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

                let ret = inner(ts.task.virt.as_ref(), &ts.task.files, cx.args()).await;
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
        let path = path.read_path(virt, &mut buf).await?;

        log::trace!("user chdir path = {path:?}");

        crate::fs::open_dir(
            path,
            OpenOptions::RDONLY | OpenOptions::DIRECTORY,
            Permissions::SELF_R,
        )
        .await?;

        files.chdir(path).await;
        Ok(())
    }

    pub async fn getcwd(
        virt: Pin<&Virt>,
        files: &Files,
        buf: UserPtr<u8, Out>,
        len: usize,
    ) -> Result<usize, Error> {
        log::trace!("user getcwd buf = {buf:?}, len = {len}");

        let cwd = files.cwd();
        let path = cwd.as_str().as_bytes();
        if path.len() >= len {
            Err(ERANGE)
        } else {
            buf.write_slice(virt, path, true).await?;
            Ok(buf.addr())
        }
    }

    pub async fn dup(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<i32, Error> {
        log::trace!("user dup fd = {fd}");

        let entry = files.get(fd).await?;
        files.open(entry).await
    }

    pub async fn dup3(
        _v: Pin<&Virt>,
        files: &Files,
        old: i32,
        new: i32,
        _flags: i32,
    ) -> Result<i32, Error> {
        log::trace!("user dup old = {old}, new = {new}, flags = {_flags}");

        let entry = files.get(old).await?;
        files.reopen(new, entry).await;
        Ok(new)
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
        let path = path.read_path(virt, &mut buf).await?;

        let options = OpenOptions::from_bits_truncate(options);
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;

        log::trace!(
            "user openat fd = {fd}, path = {path:?}, options = {options:?}, perm = {perm:?}"
        );

        let base = files.get(fd).await?;
        let (entry, _) = base.open(path, options, perm).await?;
        files.open(entry).await
    }

    pub async fn mkdirat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        perm: u32,
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(virt, &mut buf).await?;
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;
        let base = files.get(fd).await?;

        log::trace!("user mkdir fd = {fd}, path = {path:?}, perm = {perm:?}");

        let (entry, created) = base
            .open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm)
            .await?;
        if !created {
            return Err(EEXIST);
        }
        files.open(entry).await
    }

    pub async fn fstat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        out: UserPtr<u8, Out>,
    ) -> Result<(), Error> {
        #[derive(Debug, Clone, Copy, Default)]
        #[repr(C, packed)]
        struct Kstat {
            dev: u64,
            inode: u64,
            perm: Permissions,
            link_count: u32,
            uid: u32,
            gid: u32,
            rdev: u32,
            __pad: u64,
            size: usize,
            blksize: u32,
            __pad2: u32,
            blocks: u64,
            atime_sec: u64,
            atime_nsec: u64,
            mtime_sec: u64,
            mtime_nsec: u64,
            ctime_sec: u64,
            ctime_nsec: u64,
            __pad3: [u32; 2],
        }
        let mut out = out.cast::<Kstat>();

        let file = files.get(fd).await?;
        let metadata = file.metadata().await;

        out.write(
            virt,
            Kstat {
                dev: 1,
                inode: metadata.offset,
                perm: metadata.perm,
                link_count: 1,
                size: metadata.len,
                blksize: metadata.block_size as u32,
                blocks: metadata.block_count as u64,
                ..Default::default()
            },
        )
        .await
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
        let entry = files.get(fd).await?;
        let dir = entry.to_dir().ok_or(ENOTDIR)?;

        let mut d = dir.next_dirent(None).await?;
        let mut count = 0;
        loop {
            let Some(entry) = d else { break Ok(count) };

            let layout = Layout::new::<D>()
                .extend_packed(Layout::for_value(&*entry.name))?
                .extend_packed(Layout::new::<u8>())?
                .pad_to_align();
            if layout.size() > len {
                break Ok(count);
            }
            let Ok(reclen) = layout.size().try_into() else {
                break Ok(count);
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

            d = dir.next_dirent(Some(&entry)).await?;
            count += 1;
        }
    }

    pub async fn unlinkat(
        virt: Pin<&Virt>,
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        flags: i32,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(virt, &mut buf).await?;

        log::trace!("user mkdir fd = {fd}, path = {path:?}, flags = {flags}");

        let base = files.get(fd).await?;
        let base = base.to_dir_mut().ok_or(ENOTDIR)?;
        base.unlink(path, (flags != 0).then_some(true)).await?;

        Ok(())
    }

    pub async fn close(_v: Pin<&Virt>, files: &Files, fd: i32) -> Result<(), Error> {
        log::trace!("user close fd = {fd}");

        files.close(fd).await
    }
);
