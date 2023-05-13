use alloc::{boxed::Box, sync::Arc};
use core::{
    alloc::Layout,
    mem::{self, MaybeUninit},
    pin::Pin,
};

use afat32::NullTimeProvider;
use co_trap::UserCx;
use kmem::{Phys, Virt};
use ksc::{
    async_handler,
    Error::{self, EBADF, EEXIST, EINVAL, EISDIR, ENODEV, ENOSYS, ENOTBLK, ENOTDIR, EPERM, ERANGE},
};
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT};
use umifs::{
    traits::IntoAnyExt,
    types::{FileType, OpenOptions, Permissions},
};

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
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_mut_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
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
        if len == 0 {
            return Ok(0);
        }
        let mut bufs = buffer.as_slice(ts.virt.as_ref(), len).await?;

        let entry = ts.files.get(fd).await?;
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
            rdev: u64,
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

    pub async fn mmap(
        virt: Pin<&Virt>,
        files: &Files,
        addr: usize,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: usize,
    ) -> Result<usize, Error> {
        bitflags::bitflags! {
            #[derive(Default, Debug, Clone, Copy)]
            struct Prot: i32 {
                const READ     = 0x1;
                const WRITE    = 0x2;
                const EXEC     = 0x4;
            }

            struct Flags: i32 {
                const SHARED	= 0x01;		/* Share changes */
                const PRIVATE	= 0x02;		/* Changes are private */

                const FIXED     = 0x100;  /* Interpret addr exactly */
                const ANONYMOUS = 0x10;  /* don't use a file */

                const POPULATE  = 0x20000;  /* populate (prefault) pagetables */
            }
        }

        let prot = Prot::from_bits(prot).ok_or(ENOSYS)?;
        let flags = Flags::from_bits_truncate(flags);

        let cow = flags.contains(Flags::PRIVATE);
        let phys = if flags.contains(Flags::ANONYMOUS) {
            Phys::new_anon(cow)
        } else {
            let entry = files.get(fd).await?;
            match entry.clone().downcast::<Phys>() {
                Some(phys) => phys.clone_as(cow)?,
                None => Phys::new(entry.to_io().ok_or(EISDIR)?, 0, cow),
            }
        };

        let addr = flags.contains(Flags::FIXED).then(|| LAddr::from(addr));

        let offset = if offset & PAGE_MASK != 0 {
            return Err(EINVAL);
        } else {
            offset >> PAGE_SHIFT
        };

        let attr = Attr::builder()
            .user_access(true)
            .readable(prot.contains(Prot::READ))
            .writable(prot.contains(Prot::WRITE))
            .executable(prot.contains(Prot::EXEC))
            .build();

        let count = (len + PAGE_MASK) >> PAGE_SHIFT;
        let addr = virt.map(addr, Arc::new(phys), offset, count, attr).await?;

        if flags.contains(Flags::POPULATE) {
            virt.commit_range(addr..(addr + len)).await?;
        }

        Ok(addr.val())
    }

    pub async fn munmap(
        virt: Pin<&Virt>,
        _f: &Files,
        addr: usize,
        len: usize,
    ) -> Result<(), Error> {
        let len = (len + PAGE_MASK) & !PAGE_MASK;
        virt.unmap(addr.into()..(addr + len).into()).await
    }

    pub async fn pipe(virt: Pin<&Virt>, files: &Files, fd: UserPtr<i32, Out>) -> Result<(), Error> {
        let (tx, rx) = crate::fs::pipe();
        let tx = files.open(tx).await?;
        let rx = files.open(rx).await?;
        fd.write_slice(virt, &[rx, tx], false).await
    }

    pub async fn mount(
        virt: Pin<&Virt>,
        _f: &Files,
        src: UserPtr<u8, In>,
        dst: UserPtr<u8, In>,
        ty: UserPtr<u8, In>,
        _flags: usize,
        _data: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let mut src_buf = [0; MAX_PATH_LEN];
        let mut dst_buf = [0; MAX_PATH_LEN];
        let mut ty_buf = [0; 64];
        let src = src.read_path(virt, &mut src_buf).await?;
        let dst = dst.read_path(virt, &mut dst_buf).await?;
        let ty = ty.read_str(virt, &mut ty_buf).await?;

        let (src, _) = crate::fs::open(
            src,
            Default::default(),
            Permissions::all_same(true, true, true),
        )
        .await?;
        crate::fs::open_dir(dst, Default::default(), Default::default()).await?;

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
            crate::fs::mount(dst.to_path_buf(), fatfs);
        } else {
            return Err(ENODEV);
        }

        Ok(())
    }

    pub async fn umount(
        virt: Pin<&Virt>,
        _f: &Files,
        target: UserPtr<u8, In>,
    ) -> Result<(), Error> {
        let mut buf = [9; MAX_PATH_LEN];
        let target = target.read_path(virt, &mut buf).await?;
        crate::fs::unmount(target);
        Ok(())
    }
);
