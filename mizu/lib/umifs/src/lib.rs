#![no_std]

pub mod path;
pub mod types;

use alloc::{boxed::Box, sync::Arc};
use core::any::Any;

use async_trait::async_trait;
use ksc_core::Error;

use self::{
    path::Path,
    types::{DirEntry, Metadata, OpenOptions, Permissions},
};

extern crate alloc;

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
pub trait File: Entry {}

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
