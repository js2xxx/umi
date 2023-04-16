use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use core::{borrow::Borrow, mem, num::NonZeroUsize, ptr::NonNull};

use async_trait::async_trait;
use ksc_core::Error;
use rv39_paging::{PAddr, ID_OFFSET, PAGE_SIZE};
use spin::{Lazy, Mutex};

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

pub struct Frames<B> {
    frames: Mutex<LruCache<usize, FrameInfo>>,
    backend: B,
}

impl<B> Frames<B> {
    pub fn new(backend: B) -> Self {
        Frames {
            frames: Mutex::new(LruCache::unbounded()),
            backend,
        }
    }

    pub fn new_anon() -> Frames<Zero> {
        Frames::new(Zero)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: Backend> Frames<B> {
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
                self.backend.flush(index, Some(&fi.frame)).await?;
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
            self.backend.flush(index, Some(&frame)).await?;
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
                    self.backend.flush(index, Some(&frame)).await?;
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

impl Default for Frames<Zero> {
    fn default() -> Self {
        Self::new_anon()
    }
}

#[async_trait]
#[allow(clippy::len_without_is_empty)]
pub trait Backend: Send + Sync + 'static {
    async fn len(&self) -> usize;

    #[inline]
    fn is_direct(&self) -> bool {
        true
    }

    async fn commit(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error>;

    async fn flush(&self, index: usize, frame: Option<&Frame>) -> Result<(), Error>;

    #[inline]
    async fn spare(&self, max_count: NonZeroUsize) -> Result<Vec<Frame>, Error> {
        let _ = max_count;
        Ok(Vec::new())
    }
}

#[async_trait]
impl<B: Backend> Backend for Frames<B> {
    async fn len(&self) -> usize {
        self.backend().len().await
    }

    fn is_direct(&self) -> bool {
        false
    }

    async fn commit(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error> {
        self.commit(index, writable).await
    }

    async fn flush(&self, index: usize, frame: Option<&Frame>) -> Result<(), Error> {
        assert!(frame.is_none(), "nesting LRU frames is not supported");
        self.flush(index).await
    }

    async fn spare(&self, max_count: NonZeroUsize) -> Result<Vec<Frame>, Error> {
        self.spare(max_count).await
    }
}

pub struct Zero;

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
    async fn flush(&self, _: usize, _: Option<&Frame>) -> Result<(), Error> {
        Ok(())
    }
}
