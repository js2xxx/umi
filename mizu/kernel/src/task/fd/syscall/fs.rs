use alloc::boxed::Box;
use core::{
    alloc::Layout,
    mem::{self, MaybeUninit},
    pin::Pin,
};

use afat32::NullTimeProvider;
use arsc_rs::Arsc;
use co_trap::UserCx;
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, *},
};
use ktime::Instant;
use rand_riscv::RandomState;
use umifs::types::{FileType, Metadata, OpenOptions, Permissions, SetMetadata, Times};

use crate::{
    mem::{In, Out, UserPtr},
    syscall::{ffi::Ts, ScRet},
    task::{
        fd::{FdInfo, Files, SavedNextDirent, MAX_PATH_LEN},
        TaskState,
    },
};

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
            atime: time(metadata.times.last_access),
            mtime: time(metadata.times.last_modified),
            ctime: time(metadata.times.last_created),
            ..Default::default()
        }
    }
}

fssc! {
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
) -> Result<(), Error> {
    let mut buf = [0; MAX_PATH_LEN];
    let (path, root) = path.read_path(virt, &mut buf).await?;
    let perm = Permissions::from_bits(perm).ok_or(EPERM)?;

    log::trace!("user mkdir fd = {fd}, path = {path:?}, perm = {perm:?}");

    let (_, created) = if root {
        crate::fs::open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm).await?
    } else {
        let base = files.get(fd).await?;
        base.open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm)
            .await?
    };
    if !created {
        return Err(EEXIST);
    }
    Ok(())
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
    let metadata = SetMetadata {
        times: Times {
            last_created: None,
            last_modified: m,
            last_access: a,
        },
        ..Default::default()
    };
    file.set_metadata(metadata).await
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
}

#[async_handler]
pub async fn ftruncate(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, usize) -> Result<(), Error>>,
) -> ScRet {
    let (fd, len) = cx.args();
    let fut = async {
        let file = ts.files.get(fd).await?;
        let metadata = SetMetadata {
            len: Some(len),
            ..Default::default()
        };
        file.set_metadata(metadata).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn truncate(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, In>, usize) -> Result<(), Error>>,
) -> ScRet {
    let (path, len) = cx.args();
    let mut buf = [0; MAX_PATH_LEN];
    let fut = async {
        let (path, _) = path.read_path(ts.virt.as_ref(), &mut buf).await?;
        let (file, _) = crate::fs::open(
            path,
            OpenOptions::RDONLY,
            Permissions::all_same(true, false, false),
        )
        .await?;
        let metadata = SetMetadata {
            len: Some(len),
            ..Default::default()
        };
        file.set_metadata(metadata).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
