use alloc::{boxed::Box, sync::Arc};

use async_trait::async_trait;
use ksc_core::Error::{self, EEXIST, ENOTDIR, EPERM};

use crate::{
    path::Path,
    traits::{Entry, File},
    types::{FileType, IoSlice, IoSliceMut, Metadata, OpenOptions, Permissions, SeekFrom},
};

pub struct Null;

#[async_trait]
impl File for Null {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Ok(0)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Ok(0)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Ok(0)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Null {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        expect_ty: Option<FileType>,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() {
            return Err(ENOTDIR);
        }
        if !matches!(expect_ty, None | Some(FileType::FILE)) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            return Err(EEXIST);
        }
        if !Permissions::all_same(true, true, false).contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, true, false),
            last_access: None,
            last_modified: None,
        }
    }
}

pub struct Zero;

#[async_trait]
impl File for Zero {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Ok(0)
    }

    async fn read_at(&self, _: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        Ok(buffer.iter_mut().fold(0, |len, buf| {
            buf.fill(0);
            len + buf.len()
        }))
    }

    async fn write_at(&self, _: usize, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        Ok(buffer.iter_mut().fold(0, |len, buf| len + buf.len()))
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Zero {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        expect_ty: Option<FileType>,
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() {
            return Err(ENOTDIR);
        }
        if !matches!(expect_ty, None | Some(FileType::FILE)) {
            return Err(ENOTDIR);
        }
        if options.contains(OpenOptions::CREAT) {
            return Err(EEXIST);
        }
        if !Permissions::all_same(true, true, false).contains(perm) {
            return Err(EPERM);
        }
        Ok((self, false))
    }

    fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, true, false),
            last_access: None,
            last_modified: None,
        }
    }
}
