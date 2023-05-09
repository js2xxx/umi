use alloc::{boxed::Box, sync::Arc};
use core::{
    alloc::Layout,
    mem::{self, MaybeUninit},
    sync::atomic::{AtomicI32, Ordering::SeqCst},
};

use co_trap::UserCx;
use futures_util::future::join_all;
use hashbrown::HashMap;
use ksc::{
    async_handler,
    Error::{self, EBADF, EEXIST, ENOSPC, ENOTDIR, EPERM, ERANGE},
};
use ksync::RwLock;
use rand_riscv::RandomState;
use umifs::{
    path::{Path, PathBuf},
    traits::Entry,
    types::{FileType, OpenOptions, Permissions},
};

use super::TaskState;
use crate::{
    mem::{In, Out, UserBuffer, UserPtr},
    syscall::ScRet,
};

const MAX_FDS: usize = 65536;

pub struct Files {
    map: RwLock<HashMap<i32, Arc<dyn Entry>, RandomState>>,
    cwd: spin::RwLock<PathBuf>,
    id_alloc: AtomicI32,
}

impl Files {
    pub fn new(stdio: [Arc<dyn Entry>; 3], cwd: PathBuf) -> Self {
        Files {
            map: RwLock::new(
                stdio
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| (i as i32, e))
                    .collect(),
            ),
            cwd: spin::RwLock::new(cwd),
            id_alloc: AtomicI32::new(3),
        }
    }

    pub async fn reopen(&self, fd: i32, entry: Arc<dyn Entry>) {
        if let Some(old) = self.map.write().await.insert(fd, entry) {
            if let Some(io) = old.to_io() {
                let _ = io.flush().await;
            }
        }
    }

    pub async fn chdir(&self, path: &Path) {
        ksync::critical(|| *self.cwd.write() = path.to_path_buf());
    }

    pub fn cwd(&self) -> PathBuf {
        ksync::critical(|| self.cwd.read().clone())
    }

    pub async fn open(&self, entry: Arc<dyn Entry>) -> Result<i32, Error> {
        let mut map = self.map.write().await;
        if map.len() >= MAX_FDS {
            return Err(ENOSPC);
        }
        let fd = self.id_alloc.fetch_add(1, SeqCst);
        map.insert_unique_unchecked(fd, entry);
        Ok(fd)
    }

    pub async fn get(&self, fd: i32) -> Result<Arc<dyn Entry>, Error> {
        const CWD: i32 = -100;
        match fd {
            CWD => {
                crate::fs::open_dir(
                    &self.cwd(),
                    OpenOptions::RDONLY | OpenOptions::DIRECTORY,
                    Permissions::SELF_R,
                )
                .await
            }
            _ => self.map.read().await.get(&fd).cloned().ok_or(EBADF),
        }
    }

    pub async fn close(&self, fd: i32) -> Result<(), Error> {
        match self.map.write().await.remove(&fd) {
            Some(entry) => match entry.to_io() {
                Some(io) => io.flush().await,
                None => Ok(()),
            },
            None => Err(EBADF),
        }
    }

    pub async fn flush_all(&self) {
        let map = self.map.write().await;
        let iter = map.values().filter_map(|e| {
            e.clone().to_io().map(|io| async move {
                let _ = io.flush().await;
            })
        });
        join_all(iter).await;
    }
}

pub async fn default_stdio() -> Result<[Arc<dyn Entry>; 3], Error> {
    let stderr = {
        let (fs, path) = crate::fs::get("dev/serial".as_ref()).unwrap();
        let root_dir = fs.root_dir().await?;
        root_dir
            .open(path, OpenOptions::WRONLY, Permissions::SELF_W)
            .await?
            .0
    };
    let stdout = stderr.clone();
    let stdin = stderr
        .clone()
        .open("".as_ref(), OpenOptions::RDONLY, Permissions::SELF_R)
        .await?
        .0;
    Ok([stderr, stdout, stdin])
}

#[async_handler]
pub async fn read(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> isize>,
) -> ScRet {
    async fn read_inner(
        ts: &mut TaskState,
        (fd, mut buffer, len): (i32, UserBuffer, usize),
    ) -> Result<usize, Error> {
        log::trace!("user read fd = {fd}, buffer addr = {buffer:?}, len = {len}");

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
        $(pub async fn $name:ident($files:ident: &Files, $($arg_name:ident : $arg_ty:ty),* $(,)?) -> $out:ty $body:block)*
    ) => {
        $(
            #[async_handler]
            pub async fn $name(
                ts: &mut TaskState,
                cx: UserCx<'_, fn($($arg_ty),*) -> $out>,
            ) -> ScRet {
                #[allow(unused_mut, unused_parens)]
                async fn inner(
                    $files: &Files,
                    ($(mut $arg_name),*): ($($arg_ty),*),
                ) -> $out $body

                let ret = inner(&ts.task.files, cx.args()).await;
                cx.ret(ret);

                ScRet::Continue(None)
            }
        )*
    };
}

pub const MAX_PATH_LEN: usize = 256;

fssc!(
    pub async fn chdir(files: &Files, path: UserPtr<u8, In>) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;

        crate::fs::open_dir(
            path,
            OpenOptions::RDONLY | OpenOptions::DIRECTORY,
            Permissions::SELF_R,
        )
        .await?;

        files.chdir(path).await;
        Ok(())
    }

    pub async fn getcwd(files: &Files, buf: UserPtr<u8, Out>, len: usize) -> Result<usize, Error> {
        let cwd = files.cwd();
        let path = cwd.as_str().as_bytes();
        if path.len() >= len {
            Err(ERANGE)
        } else {
            buf.write_slice(path, true)?;
            Ok(buf.addr())
        }
    }

    pub async fn dup(files: &Files, fd: i32) -> Result<i32, Error> {
        let entry = files.get(fd).await?;
        files.open(entry).await
    }

    pub async fn dup3(files: &Files, old: i32, new: i32, _flags: i32) -> Result<i32, Error> {
        let entry = files.get(old).await?;
        files.reopen(new, entry).await;
        Ok(new)
    }

    pub async fn openat(
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        options: i32,
        perm: u32,
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;
        let options = OpenOptions::from_bits_truncate(options);
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;
        let base = files.get(fd).await?;

        let (entry, _) = base.open(path, options, perm).await?;
        files.open(entry).await
    }

    pub async fn mkdirat(
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        perm: u32,
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;
        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;
        let base = files.get(fd).await?;

        let (entry, created) = base
            .open(path, OpenOptions::DIRECTORY | OpenOptions::CREAT, perm)
            .await?;
        if !created {
            return Err(EEXIST);
        }
        files.open(entry).await
    }

    pub async fn getdents64(
        files: &Files,
        fd: i32,
        ptr: UserPtr<u8, Out>,
        len: usize,
    ) -> Result<usize, Error> {
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
            ptr.write_slice(unsafe { mem::transmute(out.as_bytes()) }, false)?;
            ptr.advance(mem::size_of::<D>());
            ptr.write_slice(entry.name.as_bytes(), true)?;

            ptr.advance(layout.size() - mem::size_of::<D>());
            len -= layout.size();

            d = dir.next_dirent(Some(&entry)).await?;
            count += 1;
        }
    }

    pub async fn unlinkat(
        files: &Files,
        fd: i32,
        path: UserPtr<u8, In>,
        flags: i32,
    ) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;
        let base = files.get(fd).await?;

        let base = base.to_dir_mut().ok_or(ENOTDIR)?;
        base.unlink(path, (flags != 0).then_some(true)).await?;

        Ok(())
    }

    pub async fn close(files: &Files, fd: i32) -> Result<(), Error> {
        files.close(fd).await
    }
);
