use alloc::{boxed::Box, sync::Arc};
use core::{
    any::Any,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use async_trait::async_trait;
use kmem::{Backend, Frame};
use ksc_core::Error::{self, EINVAL};
use rv39_paging::{PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};

use crate::{
    path::Path,
    types::{
        advance_slices, DirEntry, IoSlice, IoSliceMut, Metadata, OpenOptions, Permissions, SeekFrom,
    },
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
pub trait File {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        self.read_at(offset, buffer).await
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let offset = self.seek(SeekFrom::Current(0)).await?;
        self.write_at(offset, buffer).await
    }

    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error>;

    async fn read_at(&self, offset: usize, buffer: &mut [IoSliceMut]) -> Result<usize, Error>;

    async fn write_at(&self, offset: usize, buffer: &mut [IoSlice]) -> Result<usize, Error>;

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

pub struct Seeked<B> {
    position: AtomicUsize,
    backend: B,
}

impl<B: Backend> Seeked<B> {
    pub async fn new(backend: B, append: bool) -> Self {
        let position = if append { backend.len().await } else { 0 };
        Seeked {
            position: position.into(),
            backend,
        }
    }
}

#[async_trait]
impl<B: Backend> File for Seeked<B> {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(pos) => {
                let pos = pos.checked_add(self.backend.len().await.try_into()?);
                pos.ok_or(EINVAL)?.try_into()?
            }
            SeekFrom::Current(pos) => {
                let pos = pos.checked_add(self.position.load(SeqCst).try_into()?);
                pos.ok_or(EINVAL)?.try_into()?
            }
        };
        self.position.store(pos, SeqCst);
        Ok(pos)
    }

    async fn read_at(&self, offset: usize, mut buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let (start, end) = (offset, offset.checked_add(buffer.len()).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let frame = self.backend.commit(start_page, false).await?;

            Ok(copy_from_frame(
                &mut buffer,
                &frame,
                start_offset,
                end_offset,
            ))
        } else {
            let mut read_len = 0;
            {
                let frame = self.backend.commit(start_page, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.backend.commit(index, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            {
                let frame = self.backend.commit(end_page, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, end_offset);
            }

            Ok(read_len)
        }
    }

    async fn write_at(&self, offset: usize, mut buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let (start, end) = (offset, offset.checked_add(buffer.len()).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let frame = self.backend.commit(start_page, true).await?;

            Ok(copy_to_frame(&mut buffer, &frame, start_offset, end_offset))
        } else {
            let mut written_len = 0;
            {
                let frame = self.backend.commit(start_page, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                if self.backend.is_direct() {
                    self.backend.flush(start_page, Some(&frame)).await?;
                }
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.backend.commit(index, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                if self.backend.is_direct() {
                    self.backend.flush(start_page, Some(&frame)).await?;
                }
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            {
                let frame = self.backend.commit(end_page, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, end_offset);
                if self.backend.is_direct() {
                    self.backend.flush(start_page, Some(&frame)).await?;
                }
                written_len += len;
            }

            Ok(written_len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        if self.backend.is_direct() {
            return Ok(());
        }
        let len = self.backend.len().await;
        let count = (len + PAGE_MASK) >> PAGE_SHIFT;
        for index in 0..count {
            self.backend.flush(index, None).await?;
        }
        Ok(())
    }
}

fn offsets(start: usize, end: usize) -> ((usize, usize), (usize, usize)) {
    let start_page = start >> PAGE_SHIFT;
    let start_offset = start - (start_page << PAGE_SHIFT);

    let (end_page, end_offset) = {
        let end_page = end >> PAGE_SHIFT;
        let end_offset = end - (end_page << PAGE_SHIFT);
        if end_offset == 0 {
            (end_page - 1, PAGE_SIZE)
        } else {
            (end_page, end_offset)
        }
    };

    ((start_page, start_offset), (end_page, end_offset))
}

fn copy_from_frame(
    buffer: &mut &mut [IoSliceMut],
    frame: &Frame,
    mut start: usize,
    end: usize,
) -> usize {
    let mut read_len = 0;
    loop {
        let buf = &mut buffer[0];
        let len = buf.len().min(end - start);
        if len == 0 {
            break read_len;
        }
        unsafe {
            let src = frame.as_ptr();
            buf[..len].copy_from_slice(&src.as_ref()[start..][..len]);
        }
        read_len += len;
        start += len;
        advance_slices(buffer, len);
    }
}

fn copy_to_frame(
    buffer: &mut &mut [IoSlice],
    frame: &Frame,
    mut start: usize,
    end: usize,
) -> usize {
    let mut written_len = 0;
    loop {
        let buf = buffer[0];
        let len = buf.len().min(end - start);
        if len == 0 {
            break written_len;
        }
        unsafe {
            let mut src = frame.as_ptr();
            src.as_mut()[start..][..len].copy_from_slice(&buf[..len])
        }
        written_len += len;
        start += len;
        advance_slices(buffer, len);
    }
}
