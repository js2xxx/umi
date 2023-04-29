use alloc::{boxed::Box, sync::Arc};
use core::ops::Range;

use arsc_rs::Arsc;
use async_trait::async_trait;
use futures_util::future::try_join_all;
use kmem::{Backend, Frame};
use ksc::Error::{self, ENOMEM, ENOSPC};
use rv39_paging::PAGE_SHIFT;

use crate::Interrupt;

#[async_trait]
pub trait Block: Send + Sync + 'static {
    fn block_shift(&self) -> u32;

    #[inline]
    fn block_size(&self) -> usize {
        1 << self.block_shift()
    }

    fn capacity_blocks(&self) -> usize;

    fn ack_interrupt(&self);

    async fn read(&self, block: usize, buf: &mut [u8]) -> Result<(), Error>;

    async fn write(&self, block: usize, buf: &[u8]) -> Result<(), Error>;

    async fn intr_dispatch(self: Arsc<Self>, intr: Interrupt) {
        loop {
            if !intr.wait().await {
                break;
            }
            self.ack_interrupt()
        }
    }
}

pub struct BlockBackend {
    device: Arsc<dyn Block>,
}

impl BlockBackend {
    pub fn new(device: Arsc<dyn Block>) -> Self {
        BlockBackend { device }
    }

    pub fn device(&self) -> &Arsc<dyn Block> {
        &self.device
    }
}

impl<B: Block> From<Arsc<B>> for BlockBackend {
    fn from(value: Arsc<B>) -> Self {
        BlockBackend::new(value)
    }
}

impl From<Arsc<dyn Block>> for BlockBackend {
    fn from(value: Arsc<dyn Block>) -> Self {
        BlockBackend::new(value)
    }
}

#[async_trait]
impl Backend for BlockBackend {
    #[inline]
    async fn len(&self) -> usize {
        self.device.capacity_blocks() << self.device.block_shift()
    }

    async fn commit(&self, index: usize, _writable: bool) -> Result<Arc<Frame>, Error> {
        let block_iter = block_iter(&*self.device, index)?;

        let frame = Frame::new().ok_or(ENOMEM)?;

        try_join_all(block_iter.map(|(block, buf_range)| {
            let mut ptr = frame.as_ptr();
            let buf = &mut unsafe { ptr.as_mut() }[buf_range];

            self.device.read(block, buf)
        }))
        .await?;

        Ok(Arc::new(frame))
    }

    async fn flush(&self, index: usize, frame: &Frame) -> Result<(), Error> {
        let block_iter = block_iter(&*self.device, index)?;

        try_join_all(block_iter.map(|(block, buf_range)| {
            let ptr = frame.as_ptr();
            let buf = &unsafe { ptr.as_ref() }[buf_range];

            self.device.write(block, buf)
        }))
        .await?;

        Ok(())
    }
}

fn block_range(index: usize, block_shift: u32) -> Range<usize> {
    let nr_blocks_in_frame_shift = PAGE_SHIFT - block_shift;

    (index << nr_blocks_in_frame_shift)..((index + 1) << nr_blocks_in_frame_shift)
}

fn block_iter(
    device: &dyn Block,
    index: usize,
) -> Result<impl Iterator<Item = (usize, Range<usize>)>, Error> {
    let block_shift = device.block_shift();

    let block_range = block_range(index, block_shift);
    if block_range.end > device.capacity_blocks() {
        return Err(ENOSPC);
    }

    Ok(block_range.enumerate().map(move |(index, block)| {
        let buffer_range = (index << block_shift)..((index + 1) << block_shift);
        (block, buffer_range)
    }))
}
