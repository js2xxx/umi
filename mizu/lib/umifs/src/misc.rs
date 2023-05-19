use alloc::{boxed::Box, sync::Arc};

use async_trait::async_trait;
use ksc_core::Error::{self, EEXIST, ENOTDIR, EPERM};
use umio::{ioslice_len, Io};

use crate::{
    path::Path,
    traits::Entry,
    types::{FileType, IoSlice, IoSliceMut, Metadata, OpenOptions, Permissions, SeekFrom},
};

pub struct Null;

#[async_trait]
impl Io for Null {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Ok(0)
    }

    async fn stream_len(&self) -> Result<usize, Error> {
        Ok(isize::MAX as usize + 1)
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
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
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

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::CHR,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, true, false),
            block_size: 0,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}

pub struct Zero;

#[async_trait]
impl Io for Zero {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Ok(0)
    }

    async fn stream_len(&self) -> Result<usize, Error> {
        Ok(isize::MAX as usize + 1)
    }

    async fn read_at(&self, _: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        Ok(buffer.iter_mut().fold(0, |len, buf| {
            buf.fill(0);
            len + buf.len()
        }))
    }

    async fn write_at(&self, _: usize, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        Ok(ioslice_len(&buffer))
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
        options: OpenOptions,
        perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
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

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::CHR,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, true, false),
            block_size: 0,
            block_count: 0,
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}
