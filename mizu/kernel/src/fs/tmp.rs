use alloc::{boxed::Box, sync::Arc};

use arsc_rs::Arsc;
use async_trait::async_trait;
use kmem::Phys;
use ksc::Error::{self, EEXIST, ENOSYS, ENOTDIR, EPERM};
use umifs::{
    path::Path,
    traits::{Directory, DirectoryMut, Entry, FileSystem, Io, ToIo},
    types::{DirEntry, FileType, Metadata, OpenOptions, Permissions},
};

pub struct TmpFs;

#[async_trait]
impl FileSystem for TmpFs {
    async fn root_dir(self: Arsc<Self>) -> Result<Arc<dyn Entry>, Error> {
        Ok(Arc::new(TmpRoot))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

struct TmpRoot;

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
        if !options.contains(OpenOptions::CREAT) {
            return Err(EPERM);
        }
        Ok((
            Arc::new(TmpFile {
                phys: Arc::new(Phys::new_anon(true)),
                perm,
            }),
            true,
        ))
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
