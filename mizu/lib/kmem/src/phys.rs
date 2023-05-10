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
use ksc_core::Error::{self, EINVAL, ENOMEM, EPERM};
use ksync::Mutex;
use rand_riscv::RandomState;
use rv39_paging::{PAddr, ID_OFFSET, PAGE_SHIFT, PAGE_SIZE};
use umifs::misc::Zero;
use umio::{advance_slices, ioslice_len, Io, IoExt, IoSlice, IoSliceMut, SeekFrom};

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

    pub fn base(&self) -> PAddr {
        self.base
    }

    pub fn as_ptr(&self) -> NonNull<[u8]> {
        NonNull::slice_from_raw_parts(self.ptr, PAGE_SIZE)
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { self.as_ptr().as_ref() }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { self.as_ptr().as_mut() }
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
    pin_count: usize,
}

pub struct Phys {
    frames: Mutex<LruCache<usize, FrameInfo>>,
    position: AtomicUsize,
    cow: bool,
    backend: Arc<dyn Io>,
}

impl Phys {
    pub fn new(backend: Arc<dyn Io>, initial_pos: usize, cow: bool) -> Self {
        Phys {
            frames: Mutex::new(LruCache::unbounded_with_hasher(RandomState::new())),
            position: initial_pos.into(),
            cow,
            backend,
        }
    }

    pub fn clone_as(&self, cow: bool) -> Result<Self, Error> {
        if self.cow && !cow {
            return Err(EPERM);
        }
        // TODO: Reuse frames among COW physes.
        Ok(Phys::new(self.backend.clone(), 0, cow))
    }

    pub fn is_cow(&self) -> bool {
        self.cow
    }
}

impl Phys {
    pub fn new_anon(cow: bool) -> Phys {
        Phys::new(Arc::new(Zero), 0, cow)
    }
}

impl Phys {
    pub async fn commit(
        &self,
        index: usize,
        writable: bool,
        pin: bool,
    ) -> Result<Arc<Frame>, Error> {
        log::trace!(
            "Phys::commit index = {index} {} {}",
            if writable { "writable" } else { "" },
            if pin { "pin" } else { "" }
        );
        let mut frames = self.frames.lock().await;

        let frame = frames.get_mut(&index).map(|fi| {
            if writable {
                fi.dirty = !self.cow;
                fi.pin_count += pin as usize;
            }
            fi.frame.clone()
        });
        if let Some(frame) = frame {
            return Ok(frame);
        }

        let frame = commit(&self.backend, index).await?;

        let fi = FrameInfo {
            frame: frame.clone(),
            dirty: false,
            pin_count: pin as usize,
        };

        let old_entry = {
            let mut data = frames.push(index, fi);

            let index = match data.as_ref() {
                None => return Ok(frame),
                Some(&(index, _)) => index,
            };
            let mut looped = false;

            loop {
                data = match data {
                    Some((i, _)) if i == index && looped => return Err(ENOMEM),
                    // Find a frame that is not pinned.
                    Some((index, fi)) if fi.pin_count > 0 => frames.push(index, fi),
                    Some(data) => break Some(data),
                    None => break None,
                };
                looped = true;
            }
        };

        if let Some((index, fi)) = old_entry {
            debug_assert!(fi.pin_count == 0);
            if fi.dirty {
                flush(&self.backend, index, &fi.frame).await?;
            }
        }
        Ok(frame)
    }

    pub async fn flush(
        &self,
        index: usize,
        force_dirty: Option<bool>,
        unpin: bool,
    ) -> Result<(), Error> {
        let frame = {
            let mut frames = self.frames.lock().await;

            let fi = frames.get_mut(&index);
            fi.and_then(|fi| {
                fi.pin_count -= unpin as usize;
                force_dirty
                    .map(|d| d & !self.cow)
                    .unwrap_or_else(|| mem::replace(&mut fi.dirty, false))
                    .then(|| fi.frame.clone())
            })
        };
        if let Some(frame) = frame {
            flush(&self.backend, index, &frame).await?;
        }
        Ok(())
    }

    pub async fn flush_all(&self) -> Result<(), Error> {
        let frames = {
            let mut frames = self.frames.lock().await;

            let iter = frames.iter_mut();
            iter.filter_map(|(&index, fi)| {
                mem::replace(&mut fi.dirty, false).then(|| (index, fi.frame.clone()))
            })
            .collect::<Vec<_>>()
        };

        let flush_fn = |(index, frame): (usize, Arc<Frame>)| async move {
            flush(&self.backend, index, &frame).await
        };
        try_join_all(frames.into_iter().map(flush_fn)).await?;
        Ok(())
    }

    pub async fn spare(&self, max_count: NonZeroUsize) -> Result<Vec<Frame>, Error> {
        let mut ret = Vec::new();
        let mut dirties = VecDeque::new();

        {
            let mut frames = self.frames.lock().await;
            let max_trial = frames.len();
            let mut trial = 0;
            while let Some((index, mut fi)) = frames.pop_lru() {
                let (frame, dirty, pinc) = match Arc::try_unwrap(fi.frame) {
                    Ok(frame) => (frame, fi.dirty, fi.pin_count),
                    Err(frame) => {
                        fi.frame = frame;
                        frames.push(index, fi);
                        continue;
                    }
                };

                if fi.dirty {
                    dirties.push_back((index, frame, dirty, pinc))
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
        };

        while ret.len() < max_count.get() {
            match dirties.pop_front() {
                Some((index, frame, ..)) => {
                    flush(&self.backend, index, &frame).await?;
                    ret.push(frame)
                }
                None => break,
            }
        }

        {
            let mut frames = self.frames.lock().await;
            dirties.into_iter().for_each(|(index, frame, dirty, pinc)| {
                let fi = FrameInfo {
                    frame: Arc::new(frame),
                    dirty,
                    pin_count: pinc,
                };
                frames.push(index, fi);
            })
        };

        Ok(ret)
    }
}

impl Drop for Phys {
    fn drop(&mut self) {
        let cache = self.frames.get_mut();
        if cache.iter().any(|(_, fi)| fi.dirty) {
            log::warn!(
                r"Physical memory may have not been flushed into its backend. 
Use `spare(NonZeroUsize::MAX)` to explicit flush all the data."
            );
        }
    }
}

#[async_trait]
impl Io for Phys {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(pos) => {
                let len = self.backend.seek(umio::SeekFrom::End(0)).await?;
                let pos = pos.checked_add(len.try_into()?);
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
        log::trace!(
            "Phys::read_at {offset:#x}, buffer len = {}",
            ioslice_len(&buffer)
        );

        let ioslice_len = ioslice_len(&buffer);
        let (start, end) = (offset, offset.checked_add(ioslice_len).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let frame = self.commit(start_page, false, false).await?;

            Ok(copy_from_frame(
                &mut buffer,
                &frame,
                start_offset,
                end_offset,
            ))
        } else {
            let mut read_len = 0;
            {
                let frame = self.commit(start_page, false, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.commit(index, false, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                if buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            {
                let frame = self.commit(end_page, false, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, end_offset);
            }

            Ok(read_len)
        }
    }

    async fn write_at(&self, offset: usize, mut buffer: &mut [IoSlice]) -> Result<usize, Error> {
        log::trace!(
            "Phys::write_at {offset:#x}, buffer len = {}",
            ioslice_len(&buffer)
        );

        let ioslice_len = ioslice_len(&buffer);
        let (start, end) = (offset, offset.checked_add(ioslice_len).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let frame = self.commit(start_page, true, false).await?;

            Ok(copy_to_frame(&mut buffer, &frame, start_offset, end_offset))
        } else {
            let mut written_len = 0;
            {
                let frame = self.commit(start_page, true, false).await?;
                let len = copy_to_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let frame = self.commit(index, true, false).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            {
                let frame = self.commit(end_page, true, false).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, end_offset);
                written_len += len;
            }

            Ok(written_len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        self.flush_all().await
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
        if buffer.is_empty() {
            break read_len;
        }
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
        if buffer.is_empty() {
            break written_len;
        }
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

async fn commit<T: Io + ?Sized>(io: impl AsRef<T>, index: usize) -> Result<Arc<Frame>, Error> {
    let io = io.as_ref();
    let offset = index << PAGE_SHIFT;
    let end = (offset + PAGE_SIZE).min(io.stream_len().await?);
    let len = end - offset;

    let mut frame = Frame::new().ok_or(ENOMEM)?;
    io.read_exact_at(offset, &mut frame.as_mut_slice()[..len])
        .await?;
    Ok(Arc::new(frame))
}

async fn flush<T: Io + ?Sized>(
    io: impl AsRef<T>,
    index: usize,
    frame: &Frame,
) -> Result<(), Error> {
    let io = io.as_ref();
    io.write_all_at(index << PAGE_SHIFT, frame.as_slice()).await
}
