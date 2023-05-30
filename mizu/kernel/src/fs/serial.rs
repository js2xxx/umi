use alloc::{boxed::Box, sync::Arc};

use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use ksc::Error::{self, EBADF, ENOSYS, ENOTDIR};
use umifs::{
    path::Path,
    traits::{Entry, Io},
    types::{FileType, IoSlice, IoSliceMut, Metadata, OpenOptions, Permissions, SeekFrom},
};

pub struct Serial {
    read: bool,
    write: bool,
}

impl Serial {
    pub fn new() -> Self {
        Serial {
            read: true,
            write: true,
        }
    }
}

impl Default for Serial {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Io for Serial {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        if !self.read {
            return Err(EBADF);
        }
        Ok(stream::iter(buffer.iter_mut())
            .fold(0, |acc, buf| async move {
                stream::iter(buf.iter_mut())
                    .zip(crate::dev::Stdin::new())
                    .for_each(|(dst, b)| async move { *dst = b })
                    .await;
                acc + buf.len()
            })
            .await)
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        if !self.write {
            return Err(EBADF);
        }
        Ok(buffer.iter().fold(0, |acc, buf| {
            crate::dev::Stdout.write_bytes(buf);
            acc + buf.len()
        }))
    }

    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(ENOSYS)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(ENOSYS)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(ENOSYS)
    }

    async fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl Entry for Serial {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if !path.as_str().is_empty() || options.contains(OpenOptions::DIRECTORY) {
            return Err(ENOTDIR);
        }
        let (read, write) = match options.intersection(OpenOptions::ACCMODE) {
            OpenOptions::RDWR => (true, true),
            OpenOptions::WRONLY => (false, true),
            OpenOptions::RDONLY => (true, false),
            _ => unreachable!(),
        };
        Ok((Arc::new(Serial { read, write }), false))
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::FILE | FileType::REG,
            len: 0,
            offset: 0,
            block_size: 1,
            block_count: isize::MAX as usize,
            perm: Permissions::all_same(self.read, self.write, false),
            last_access: None,
            last_modified: None,
            last_created: None,
        }
    }
}
