use alloc::{boxed::Box, sync::Arc};
use core::any::Any;

use arsc_rs::Arsc;
use async_trait::async_trait;
use ksc_core::Error::{self, EINTR, EIO};

use crate::{
    path::Path,
    types::{
        advance_slices, ioslice_is_empty, DirEntry, IoSlice, IoSliceMut, Metadata, OpenOptions,
        Permissions, SeekFrom,
    },
};

pub trait IntoAny: Any {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync>;
}

impl<T: Any + Send + Sync> IntoAny for T {
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self as _
    }

    fn into_any_arsc(self: Arsc<Self>) -> Arsc<dyn Any + Send + Sync> {
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
pub trait File: IntoAny + Send + Sync {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        let read_len = self.read_at(offset, buffer).await?;
        self.seek(SeekFrom::Current(read_len as isize)).await?;
        Ok(read_len)
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        let written_len = self.write_at(offset, buffer).await?;
        self.seek(SeekFrom::Current(written_len as isize)).await?;
        Ok(written_len)
    }

    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error>;

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error>;

    async fn write_at(&self, offset: usize, buffer: &mut [IoSlice]) -> Result<usize, Error>;

    async fn flush(&self) -> Result<(), Error>;
}

#[async_trait]
pub trait FileExt: File {
    async fn read_exact_at(
        &self,
        mut offset: usize,
        mut buffer: &mut [IoSliceMut],
    ) -> Result<(), Error> {
        while !ioslice_is_empty(&buffer) {
            match self.read_at(offset, &mut *buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    offset += n;
                    advance_slices(&mut buffer, n)
                }
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if ioslice_is_empty(&buffer) {
            Ok(())
        } else {
            Err(EIO)
        }
    }

    async fn read_exact(&self, mut buffer: &mut [IoSliceMut]) -> Result<(), Error> {
        while !ioslice_is_empty(&buffer) {
            match self.read(&mut *buffer).await {
                Ok(0) => break,
                Ok(n) => advance_slices(&mut buffer, n),
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if ioslice_is_empty(&buffer) {
            Ok(())
        } else {
            Err(EIO)
        }
    }

    async fn write_all_at(
        &self,
        mut offset: usize,
        mut buffer: &mut [IoSlice],
    ) -> Result<(), Error> {
        while !ioslice_is_empty(&buffer) {
            match self.write_at(offset, &mut *buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    offset += n;
                    advance_slices(&mut buffer, n)
                }
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if ioslice_is_empty(&buffer) {
            Ok(())
        } else {
            Err(EIO)
        }
    }

    async fn write_all(&self, mut buffer: &mut [IoSlice]) -> Result<(), Error> {
        while !ioslice_is_empty(&buffer) {
            match self.write(&mut *buffer).await {
                Ok(0) => break,
                Ok(n) => advance_slices(&mut buffer, n),
                Err(EINTR) => {}
                Err(e) => return Err(e),
            }
        }
        if ioslice_is_empty(&buffer) {
            Ok(())
        } else {
            Err(EIO)
        }
    }
}
impl<T: File + ?Sized> FileExt for T {}

/// Used in implementations of `read_at` by files where random access is not
/// supported.
pub async fn read_at_by_seek<F: File>(
    file: &F,
    offset: usize,
    buffer: &mut [IoSliceMut<'_>],
) -> Result<usize, Error> {
    let old = file.seek(SeekFrom::Current(0)).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    let res = file.read(buffer).await;
    let _ = file.seek(SeekFrom::Start(old)).await;
    res
}

/// Used in implementations of `write_at` by files where random access is not
/// supported.
pub async fn write_at_by_seek<F: File>(
    file: &F,
    offset: usize,
    buffer: &mut [IoSlice<'_>],
) -> Result<usize, Error> {
    let old = file.seek(SeekFrom::Current(0)).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    let res = file.write(buffer).await;
    let _ = file.seek(SeekFrom::Start(old)).await;
    res
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
