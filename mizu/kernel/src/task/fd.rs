use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicI32, Ordering::SeqCst};

use co_trap::UserCx;
use futures_util::future::join_all;
use hashbrown::HashMap;
use ksc::{
    async_handler,
    Error::{self, EBADF, EEXIST, ENOSPC, EPERM, ERANGE},
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
            id_alloc: AtomicI32::new(0),
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

    pub async fn open(&self, entry: Arc<dyn Entry>) -> Option<i32> {
        let mut map = self.map.write().await;
        if map.len() >= MAX_FDS {
            return None;
        }
        let fd = self.id_alloc.fetch_add(1, SeqCst);
        map.insert_unique_unchecked(fd, entry);
        Some(fd)
    }

    pub async fn get(&self, fd: i32) -> Option<Arc<dyn Entry>> {
        self.map.read().await.get(&fd).cloned()
    }

    pub async fn close(&self, fd: i32) -> bool {
        if let Some(entry) = self.map.write().await.remove(&fd) {
            if let Some(io) = entry.to_io() {
                let _ = io.flush().await;
            }
            true
        } else {
            false
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
            .open(
                path,
                Some(FileType::FILE),
                OpenOptions::WRONLY,
                Permissions::SELF_W,
            )
            .await?
            .0
    };
    let stdout = stderr.clone();
    let stdin = stderr
        .clone()
        .open(
            "".as_ref(),
            Some(FileType::FILE),
            OpenOptions::RDONLY,
            Permissions::SELF_R,
        )
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

        let entry = ts.task.files.get(fd).await.ok_or(EBADF)?;
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

        let entry = ts.task.files.get(fd).await.ok_or(EBADF)?;
        let io = entry.to_io().ok_or(EBADF)?;

        io.write(&mut bufs).await
    }

    let ret = write_inner(ts, cx.args()).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

const MAX_PATH_LEN: usize = 256;

#[async_handler]
pub async fn chdir(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, In>) -> Result<(), Error>>,
) -> ScRet {
    async fn chdir_inner(files: &Files, path: UserPtr<u8, In>) -> Result<(), Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;

        crate::fs::open_dir(path, OpenOptions::RDONLY, Permissions::SELF_R).await?;

        files.chdir(path).await;
        Ok(())
    }

    let ret = chdir_inner(&ts.task.files, cx.args()).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn getcwd(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<u8, Out>, usize) -> Result<(), Error>>,
) -> ScRet {
    async fn getcwd_inner(
        files: &Files,
        (mut buf, len): (UserPtr<u8, Out>, usize),
    ) -> Result<(), Error> {
        let cwd = files.cwd();
        let path = cwd.as_str().as_bytes();
        if path.len() >= len {
            Err(ERANGE)
        } else {
            buf.write_slice(path, true)?;
            Ok(())
        }
    }
    let ret = getcwd_inner(&ts.task.files, cx.args()).await;
    cx.ret(ret);

    ScRet::Continue(None)
}

#[async_handler]
pub async fn mkdirat(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u8, In>, u32) -> Result<i32, Error>>,
) -> ScRet {
    async fn mkdir_inner(
        files: &Files,
        (fd, path, perm): (i32, UserPtr<u8, In>, u32),
    ) -> Result<i32, Error> {
        let mut buf = [0; MAX_PATH_LEN];
        let path = path.read_path(&mut buf)?;

        let perm = Permissions::from_bits(perm).ok_or(EPERM)?;
        let base = files.get(fd).await.ok_or(EBADF)?;
        let (entry, created) = base
            .open(
                path,
                Some(FileType::DIR),
                OpenOptions::DIRECTORY | OpenOptions::CREAT,
                perm,
            )
            .await?;
        if !created {
            return Err(EEXIST);
        }
        files.open(entry).await.ok_or(ENOSPC)
    }

    let ret = mkdir_inner(&ts.task.files, cx.args()).await;
    cx.ret(ret);

    ScRet::Continue(None)
}
