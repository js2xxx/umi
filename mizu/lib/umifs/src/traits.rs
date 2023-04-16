use alloc::{boxed::Box, sync::Arc};
use core::any::Any;

use async_trait::async_trait;
use ksc_core::Error;

use crate::{
    path::Path,
    types::{DirEntry, Metadata, OpenOptions, Permissions, SeekFrom},
};

pub trait IntoAny: Any {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

impl<T: Any + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self as _
    }
}

pub trait Entry: IntoAny {
    fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error>;

    fn metadata(&self) -> Metadata;
}

#[async_trait]
pub trait File: Entry {
    async fn read(&self, buffer: &mut [u8]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        self.read_at(offset, buffer).await
    }

    async fn write(&self, buffer: &[u8]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        self.write_at(offset, buffer).await
    }

    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error>;

    async fn read_at(&self, offset: usize, buffer: &mut [u8]) -> Result<usize, Error>;

    async fn write_at(&self, offset: usize, buffer: &[u8]) -> Result<usize, Error>;

    async fn flush(&self) -> Result<(), Error>;
}

#[async_trait]
pub trait Directory: Entry {
    async fn next_dirent(&self, last: Option<&str>) -> Result<DirEntry, Error>;
}

#[async_trait]
pub trait DirectoryMut: Directory {
    async fn rename(
        self: Arc<Self>,
        src: &str,
        dst_parent: Arc<dyn DirectoryMut>,
        dst: &str,
    ) -> Result<(), Error>;

    async fn link(
        self: Arc<Self>,
        src: &str,
        dst_parent: Arc<dyn DirectoryMut>,
        dst: &str,
    ) -> Result<(), Error>;

    async fn unlink(&self, name: &str, expect_dir: bool) -> Result<(), Error>;
}
