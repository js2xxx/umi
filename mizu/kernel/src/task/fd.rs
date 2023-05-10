mod syscall;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicI32, Ordering::SeqCst};

use futures_util::future::join_all;
use hashbrown::HashMap;
use ksc::Error::{self, EBADF, ENOSPC};
use ksync::RwLock;
use rand_riscv::RandomState;
use umifs::{
    path::{Path, PathBuf},
    traits::Entry,
    types::{OpenOptions, Permissions},
};

pub use self::syscall::*;

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
