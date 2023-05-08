use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicI32, Ordering::SeqCst};

use co_trap::UserCx;
use futures_util::future::join_all;
use hashbrown::HashMap;
use ksc::{
    async_handler,
    Error::{self, EBADF},
};
use ksync::RwLock;
use rand_riscv::RandomState;
use umifs::{
    traits::Entry,
    types::{FileType, OpenOptions},
};

use super::TaskState;
use crate::{mem::UserBuffer, syscall::ScRet};

const MAX_FDS: usize = 65536;

pub struct Files {
    map: RwLock<HashMap<i32, Arc<dyn Entry>, RandomState>>,
    id_alloc: AtomicI32,
}

impl Files {
    pub fn new(stdio: [Arc<dyn Entry>; 3]) -> Self {
        Files {
            map: RwLock::new(
                stdio
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| (i as i32, e))
                    .collect(),
            ),
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
                Default::default(),
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
            Default::default(),
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
    cx: UserCx<'_, fn(i32, UserBuffer, usize) -> isize>,
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

    let ret = match write_inner(ts, cx.args()).await {
        Ok(len) => len as isize,
        Err(err) => err as isize,
    };
    cx.ret(ret);

    ScRet::Continue(None)
}
