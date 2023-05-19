mod syscall;

use alloc::{sync::Arc, vec::Vec};
use core::{
    mem,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use futures_util::future::join_all;
use hashbrown::HashMap;
use ksc::Error::{self, EBADF, EMFILE};
use ksync::RwLock;
use rand_riscv::RandomState;
use spin::Mutex;
use umifs::{
    path::{Path, PathBuf},
    traits::Entry,
    types::{OpenOptions, Permissions},
};

pub use self::syscall::*;

pub const MAX_FDS: usize = 65536;
const CWD: i32 = -100;

#[derive(Clone)]
pub struct FdInfo {
    pub entry: Arc<dyn Entry>,
    pub close_on_exec: bool,
}

#[derive(Clone)]
struct IdAlloc {
    reuse: Vec<i32>,
    next: i32,
}

impl IdAlloc {
    fn alloc(&mut self) -> i32 {
        self.reuse.pop().unwrap_or_else(|| {
            let ret = self.next;
            self.next += 1;
            ret
        })
    }

    fn dealloc(&mut self, id: i32) {
        self.reuse.push(id)
    }
}

struct Fds {
    map: RwLock<HashMap<i32, FdInfo, RandomState>>,
    id_alloc: Mutex<IdAlloc>,
    limit: AtomicUsize,
}

const LIMIT_DEFAULT: usize = 64;

pub struct Files {
    fds: Arsc<Fds>,
    cwd: Arsc<spin::RwLock<PathBuf>>,
}

impl Files {
    pub fn new(stdio: [Arc<dyn Entry>; 3], cwd: PathBuf) -> Self {
        Files {
            fds: Arsc::new(Fds {
                map: RwLock::new(
                    stdio
                        .into_iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            let fd_info = FdInfo {
                                entry,
                                close_on_exec: true,
                            };
                            (i as i32, fd_info)
                        })
                        .collect(),
                ),
                id_alloc: Mutex::new(IdAlloc {
                    reuse: Vec::new(),
                    next: 3,
                }),
                limit: LIMIT_DEFAULT.into(),
            }),
            cwd: Arsc::new(spin::RwLock::new(cwd)),
        }
    }

    pub fn set_limit(&self, max: usize) -> usize {
        self.fds.limit.swap(max.min(MAX_FDS), SeqCst)
    }

    pub fn get_limit(&self) -> usize {
        self.fds.limit.load(SeqCst)
    }

    pub async fn reopen(&self, fd: i32, entry: Arc<dyn Entry>, close_on_exec: bool) {
        let fi = FdInfo {
            entry,
            close_on_exec,
        };
        if let Some(old) = self.fds.map.write().await.insert(fd, fi) {
            if let Some(io) = old.entry.to_io() {
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

    pub async fn open(&self, entry: Arc<dyn Entry>, close_on_exec: bool) -> Result<i32, Error> {
        let fi = FdInfo {
            entry,
            close_on_exec,
        };
        let mut map = self.fds.map.write().await;
        if map.len() >= self.fds.limit.load(SeqCst) {
            return Err(EMFILE);
        }
        let fd = ksync::critical(|| self.fds.id_alloc.lock().alloc());
        map.insert_unique_unchecked(fd, fi);
        Ok(fd)
    }

    pub async fn get_fi(&self, fd: i32) -> Result<FdInfo, Error> {
        match fd {
            CWD => {
                let entry = crate::fs::open_dir(
                    &self.cwd(),
                    OpenOptions::RDONLY | OpenOptions::DIRECTORY,
                    Permissions::SELF_R,
                )
                .await?;
                Ok(FdInfo {
                    entry,
                    close_on_exec: false,
                })
            }
            _ => (self.fds.map.read().await).get(&fd).cloned().ok_or(EBADF),
        }
    }

    pub async fn set_fi(&self, fd: i32, close_on_exec: bool) -> Result<(), Error> {
        if fd == CWD {
            return Ok(());
        }
        let mut map = self.fds.map.write().await;
        map.get_mut(&fd).ok_or(EBADF)?.close_on_exec = close_on_exec;
        Ok(())
    }

    pub async fn dup(&self, fd: i32, close_on_exec: Option<bool>) -> Result<i32, Error> {
        let fi = self.get_fi(fd).await?;
        self.open(fi.entry, close_on_exec.unwrap_or(fi.close_on_exec))
            .await
    }

    pub async fn get(&self, fd: i32) -> Result<Arc<dyn Entry>, Error> {
        self.get_fi(fd).await.map(|fi| fi.entry)
    }

    pub async fn close(&self, fd: i32) -> Result<(), Error> {
        match self.fds.map.write().await.remove(&fd) {
            Some(fi) => {
                ksync::critical(|| self.fds.id_alloc.lock().dealloc(fd));
                match fi.entry.to_io() {
                    Some(io) => io.flush().await,
                    None => Ok(()),
                }
            }
            None => Err(EBADF),
        }
    }

    pub async fn flush_all(&self) {
        let map = self.fds.map.write().await;
        let iter = map.values().filter_map(|fi| {
            fi.entry.clone().to_io().map(|io| async move {
                let _ = io.flush().await;
            })
        });
        join_all(iter).await;
    }

    pub async fn deep_fork(&self, share_cwd: bool, share_fd: bool) -> Self {
        Files {
            cwd: if share_cwd {
                self.cwd.clone()
            } else {
                Arsc::new(spin::RwLock::new(self.cwd()))
            },
            fds: if share_fd {
                self.fds.clone()
            } else {
                Arsc::new(Fds {
                    map: RwLock::new(self.fds.map.read().await.clone()),
                    id_alloc: Mutex::new(ksync::critical(|| self.fds.id_alloc.lock().clone())),
                    limit: LIMIT_DEFAULT.into(),
                })
            },
        }
    }

    pub async fn append_afterlife(&self, afterlife: &Self) {
        let afterlife = mem::take(&mut *afterlife.fds.map.write().await);
        let mut map = self.fds.map.write().await;
        map.retain(|_, fi| !fi.close_on_exec);
        map.extend(afterlife.into_iter());
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
    Ok([stdin, stdout, stderr])
}
