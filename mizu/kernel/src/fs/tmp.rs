use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use hashbrown::{hash_map::Entry as H, HashMap};
use kmem::Phys;
use ksc::Error::{self, EEXIST, ENOENT, ENOSYS, ENOTDIR, EPERM};
use rand_riscv::RandomState;
use spin::Mutex;
use umifs::{
    path::{Path, PathBuf},
    traits::{Directory, DirectoryMut, Entry, FileSystem, Io, ToIo},
    types::{DirEntry, FileType, Metadata, OpenOptions, Permissions},
};

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
}

struct TmpRoot(Mutex<HashMap<PathBuf, Weak<TmpFile>, RandomState>>);

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
                phys: Arc::new(Phys::new_anon(true)),
                perm,
            });
            ksync::critical(|| {
                let mut list = self.0.lock();
                match list.entry(path.to_path_buf()) {
                    H::Occupied(mut ent) => {
                        if ent.get().upgrade().is_none() {
                            ent.insert(Arc::downgrade(&file));
                        } else {
                            return Err(EEXIST);
                        }
                    }
                    H::Vacant(ent) => {
                        ent.insert(Arc::downgrade(&file));
                    }
                }
                Ok((file as _, true))
            })
        } else {
            let weak = ksync::critical(|| self.0.lock().get(path).cloned());
            Ok((weak.and_then(|w| w.upgrade()).ok_or(ENOENT)?, false))
        }
    }

    async fn metadata(&self) -> Metadata {
        todo!()
    }
}

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

struct TmpFile {
    phys: Arc<Phys>,
    perm: Permissions,
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
        Metadata {
            ty: FileType::FILE,
            len: self.phys.stream_len().await.unwrap(),
            offset: u64::MAX,
            perm: self.perm,
            block_size: 0,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}
