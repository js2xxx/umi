use alloc::{boxed::Box, sync::Arc};

use arsc_rs::Arsc;
use async_trait::async_trait;
use hashbrown::HashMap;
use kmem::Phys;
use ksc::Error::{self, EEXIST, ENOENT, ENOSYS, ENOTDIR, EPERM};
use ktime::Instant;
use rand_riscv::RandomState;
use rv39_paging::PAGE_SIZE;
use spin::Mutex;
use umifs::{
    path::{Path, PathBuf},
    traits::{Directory, DirectoryMut, Entry, FileSystem, Io, ToIo},
    types::{DirEntry, FileType, FsStat, Metadata, OpenOptions, Permissions},
};
use umio::IoPoll;

pub struct TmpFs(Arc<TmpRoot>);

impl TmpFs {
    pub fn new() -> Self {
        TmpFs(Arc::new(TmpRoot(Default::default())))
    }
}

#[async_trait]
impl FileSystem for TmpFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(self.0.clone())
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn stat(&self) -> FsStat {
        FsStat {
            ty: "tmpfs",
            block_size: PAGE_SIZE,
            block_count: 0,
            block_free: 0,
            file_count: ksync::critical(|| self.0 .0.lock().len()),
        }
    }
}

struct TmpRoot(Mutex<HashMap<PathBuf, Arc<TmpFile>, RandomState>>);

impl ToIo for TmpRoot {}

#[async_trait]
impl Entry for TmpRoot {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if path == "" {
            if options.contains(OpenOptions::CREAT) {
                return Err(EEXIST);
            }
            return Ok((self, false));
        }

        if options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            let file = Arc::new(TmpFile {
                phys: Arc::new(Phys::new(false)),
                perm,
                times: Mutex::new({
                    let now = Instant::now();
                    Times {
                        created: now,
                        modified: now,
                        accessed: now,
                    }
                }),
            });
            ksync::critical(|| {
                let mut list = self.0.lock();
                if list.try_insert(path.to_path_buf(), file.clone()).is_err() {
                    return Err(EEXIST);
                }
                Ok((file as _, true))
            })
        } else {
            let file = ksync::critical(|| self.0.lock().get(path).cloned());
            Ok((file.ok_or(ENOENT)?, false))
        }
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::DIR,
            len: 0,
            offset: rand_riscv::seed64(),
            perm: Permissions::all_same(true, true, true),
            block_size: PAGE_SIZE,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }

    fn to_dir(self: Arc<Self>) -> Option<Arc<dyn Directory>> {
        Some(self)
    }

    fn to_dir_mut(self: Arc<Self>) -> Option<Arc<dyn DirectoryMut>> {
        Some(self)
    }
}
impl IoPoll for TmpRoot {}

#[async_trait]
impl Directory for TmpRoot {
    async fn next_dirent(&self, _: Option<&DirEntry>) -> Result<Option<DirEntry>, Error> {
        todo!()
    }
}

#[async_trait]
impl DirectoryMut for TmpRoot {
    async fn rename(
        self: Arc<Self>,
        _: &Path,
        _: Arc<dyn DirectoryMut>,
        _: &Path,
    ) -> Result<(), Error> {
        Err(ENOSYS)
    }

    async fn link(
        self: Arc<Self>,
        _: &Path,
        _: Arc<dyn DirectoryMut>,
        _: &Path,
    ) -> Result<(), Error> {
        Err(ENOSYS)
    }

    async fn unlink(&self, _: &Path, _: Option<bool>) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct Times {
    created: Instant,
    modified: Instant,
    accessed: Instant,
}

struct TmpFile {
    phys: Arc<Phys>,
    perm: Permissions,
    times: Mutex<Times>,
}

impl ToIo for TmpFile {
    fn to_io(self: Arc<Self>) -> Option<Arc<dyn umifs::traits::Io>> {
        Some(self.phys.clone())
    }
}

#[async_trait]
impl Entry for TmpFile {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            return Err(EEXIST);
        }
        if !self.perm.contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    async fn metadata(&self) -> Metadata {
        let times = ksync::critical(|| *self.times.lock());
        Metadata {
            ty: FileType::FILE,
            len: self.phys.stream_len().await.unwrap(),
            offset: u64::MAX,
            perm: self.perm,
            block_size: PAGE_SIZE,
            block_count: 0,
            last_access: Some(times.accessed),
            last_modified: Some(times.modified),
            last_created: Some(times.created),
        }
    }

    async fn set_times(&self, c: Option<Instant>, m: Option<Instant>, a: Option<Instant>) {
        ksync::critical(|| {
            let mut times = self.times.lock();
            if let Some(c) = c {
                times.created = c;
            }
            if let Some(m) = m {
                times.modified = m;
            }
            if let Some(a) = a {
                times.accessed = a;
            }
        })
    }
}

impl IoPoll for TmpFile {}
