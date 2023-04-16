use alloc::{
    boxed::Box,
    collections::{btree_map::Entry, BTreeMap},
    sync::Arc,
};
use core::{borrow::Borrow, num::NonZeroUsize, ptr::NonNull};

use async_trait::async_trait;
use ksc_core::Error;
use rv39_paging::{PAddr, ID_OFFSET, PAGE_SIZE};
use spin::{Lazy, RwLock};

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

pub struct Frames<B> {
    frames: RwLock<BTreeMap<usize, Arc<Frame>>>,
    backend: B,
}

impl<B> Frames<B> {
    pub fn new(backend: B) -> Self {
        Frames {
            frames: RwLock::new(BTreeMap::new()),
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
    pub async fn get(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error> {
        if let Some(frame) = ksync::critical(|| self.frames.read().get(&index).cloned()) {
            return Ok(frame);
        }
        let frame = self.backend.commit(index, writable).await?;
        ksync::critical(|| self.frames.write().insert(index, frame.clone()));
        Ok(frame)
    }

    pub async fn flush(&self, index: usize, spare: bool) -> Result<Option<Arc<Frame>>, Error> {
        if spare {
            if let Some(frame) = ksync::critical(|| self.frames.read().get(&index).cloned()) {
                self.backend.flush(index, &frame).await?;
            }
            Ok(None)
        } else {
            ksync::critical(|| async {
                let mut frames = self.frames.write();
                if let Entry::Occupied(ent) = frames.entry(index) {
                    match self.backend.flush(index, ent.get()).await {
                        Ok(()) => Ok(Some(ent.remove())),
                        Err(err) => Err(err),
                    }
                } else {
                    Ok(None)
                }
            })
            .await
        }
    }
}

impl Default for Frames<Zero> {
    fn default() -> Self {
        Self::new_anon()
    }
}

#[async_trait]
pub trait Backend: 'static {
    async fn commit(&self, index: usize, writable: bool) -> Result<Arc<Frame>, Error>;

    async fn flush(&self, index: usize, frame: &Frame) -> Result<(), Error>;
}

pub struct Zero;

#[async_trait]
impl Backend for Zero {
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
