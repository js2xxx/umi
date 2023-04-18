use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    borrow::Borrow,
    mem,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use async_trait::async_trait;
use futures_util::future::try_join_all;
use ksc_core::Error::{self, EINVAL};
use rv39_paging::{PAddr, ID_OFFSET, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE};
use spin::{Lazy, Mutex};
use umifs::{
    misc::Zero,
    traits::File,
    types::{advance_slices, IoSlice, IoSliceMut, SeekFrom},
};

use crate::lru::LruCache;

#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    base: PAddr,
    ptr: NonNull<u8>,
}

unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}

impl Frame {
    pub fn new() -> Option<Self> {
        let laddr = crate::frame::frames().allocate(NonZeroUsize::MIN)?;
        unsafe { laddr.write_bytes(0, PAGE_SIZE) };
        Some(Frame {
            base: laddr.to_paddr(ID_OFFSET),
            ptr: laddr.as_non_null().unwrap(),
        })
    }

    pub fn as_ptr(&self) -> NonNull<[u8]> {
        NonNull::slice_from_raw_parts(self.ptr, PAGE_SIZE)
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let laddr = self.base.to_laddr(ID_OFFSET);
        unsafe { crate::frame::frames().deallocate(laddr, NonZeroUsize::MIN) }
    }
}

impl Borrow<PAddr> for Frame {
    fn borrow(&self) -> &PAddr {
        &self.base
    }
}

pub struct FrameInfo {
    frame: Arc<Frame>,
    dirty: bool,
}

pub struct Phys<B> {
    frames: Mutex<LruCache<usize, FrameInfo>>,
    position: AtomicUsize,
    backend: B,
}

impl<B> Phys<B> {
    pub fn new(backend: B, initial_pos: usize) -> Self {
        Phys {
            frames: Mutex::new(LruCache::unbounded()),
            position: initial_pos.into(),
            backend,
        }
    }

    pub fn new_anon() -> Phys<Zero> {
        Phys::new(Zero, 0)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: Backend> Phys<B> {
    pub async fn commit(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error> {
        let frame = ksync::critical(|| {
            self.frames.lock().get_mut(&index).map(|fi| {
                if writable {
                    fi.dirty = true;
                }
                fi.frame.clone()
            })
        });
        if let Some(frame) = frame {
            return Ok(frame);
        }
        let frame = self.backend.commit(index, writable).await?;
        if let Some((index, fi)) = ksync::critical(|| {
            let fi = FrameInfo {
                frame: frame.clone(),
                dirty: false,
            };
            self.frames.lock().push(index, fi)
        }) {
            if fi.dirty {
                self.backend.flush(index, &fi.frame).await?;
            }
        }
        Ok(frame)
    }

    pub async fn flush(&self, index: usize) -> Result<(), Error> {
        let frame = ksync::critical(|| {
            let mut frames = self.frames.lock();
            let fi = frames.get_mut(&index);
            fi.and_then(|fi| mem::replace(&mut fi.dirty, false).then(|| fi.frame.clone()))
        });
        if let Some(frame) = frame {
            self.backend.flush(index, &frame).await?;
        }
        Ok(())
    }

    pub async fn spare(&self, max_count: NonZeroUsize) -> Result<Vec<Frame>, Error> {
        let mut ret = Vec::new();
        let mut dirties = VecDeque::new();

        ksync::critical(|| {
            let mut frames = self.frames.lock();
            let max_trial = frames.len();
            let mut trial = 0;
            while let Some((index, mut fi)) = frames.pop_lru() {
                let frame = match Arc::try_unwrap(fi.frame) {
                    Ok(frame) => frame,
                    Err(frame) => {
                        fi.frame = frame;
                        frames.push(index, fi);
                        continue;
                    }
                };

                if fi.dirty {
                    dirties.push_back((index, frame))
                } else {
                    ret.push(frame)
                }
                if ret.len() >= max_count.get() {
                    break;
                }
                trial += 1;
                if trial >= max_trial {
                    break;
                }
            }
        });

        while ret.len() < max_count.get() {
            match dirties.pop_front() {
                Some((index, frame)) => {
                    self.backend.flush(index, &frame).await?;
                    ret.push(frame)
                }
                None => break,
            }
        }

        ksync::critical(|| {
            let mut frames = self.frames.lock();
            dirties.into_iter().for_each(|(index, frame)| {
                let fi = FrameInfo {
                    frame: Arc::new(frame),
                    dirty: true,
                };
                frames.push(index, fi);
            })
        });

        Ok(ret)
    }
}

impl Default for Phys<Zero> {
    fn default() -> Self {
        Self::new_anon()
    }
}

#[async_trait]
#[allow(clippy::len_without_is_empty)]
pub trait Backend: Send + Sync + 'static {
    async fn len(&self) -> usize;

    async fn commit(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error>;

    async fn flush(&self, index: usize, frame: &Frame) -> Result<(), Error>;
}

#[async_trait]
impl Backend for Zero {
    async fn len(&self) -> usize {
        0
    }

    async fn commit(&self, _: usize, writable: bool) -> Result<Arc<Frame>, Error> {
        static ZERO: Lazy<Arc<Frame>> =
            Lazy::new(|| Arc::new(Frame::new().expect("zero frame init failed")));
        Ok(if writable {
            Arc::new(Frame::new().ok_or(ksc_core::ENOMEM)?)
        } else {
            ZERO.clone()
        })
    }

    #[inline]
    async fn flush(&self, _: usize, _: &Frame) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
impl<B: Backend> File for Phys<B> {
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
            let frame = self.commit(start_page, false).await?;

            Ok(copy_from_frame(
                &mut buffer,
                &frame,
                start_offset,
                end_offset,
            ))
        } else {
            let mut read_len = 0;
            {
                let frame = self.commit(start_page, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.commit(index, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            {
                let frame = self.commit(end_page, false).await?;
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
            let frame = self.commit(start_page, true).await?;

            Ok(copy_to_frame(&mut buffer, &frame, start_offset, end_offset))
        } else {
            let mut written_len = 0;
            {
                let frame = self.commit(start_page, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.commit(index, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            {
                let frame = self.commit(end_page, true).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, end_offset);
                written_len += len;
            }

            Ok(written_len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        let len = self.backend.len().await;
        let count = (len + PAGE_MASK) >> PAGE_SHIFT;
        try_join_all((0..count).map(|index| self.flush(index))).await?;
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
