use alloc::boxed::Box;
use core::ops::Range;

use arsc_rs::Arsc;
use async_trait::async_trait;
use futures_lite::future::yield_now;
use kmem::Backend;
use ksc::Error::{self, ENOSPC};
use rv39_paging::PAGE_SHIFT;

use crate::Interrupt;

#[async_trait]
pub trait Block: Backend {
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
            // TODO: use `intr.wait().await`.
            if let Some(false) = intr.try_wait() {
                break;
            }

            self.ack_interrupt();
            yield_now().await;
        }
    }
}

macro_rules! impl_backend_for_block {
    ($type:ident) => {
        #[async_trait]
        impl kmem::Backend for $type {
            #[inline]
            async fn len(&self) -> usize {
                self.capacity_blocks() << self.block_shift()
            }

            async fn commit(
                &self,
                index: usize,
                writable: bool,
            ) -> Result<alloc::sync::Arc<kmem::Frame>, Error> {
                log::trace!("BlockBackend::commit: index = {index}, writable = {writable}");

                let block_iter = $crate::dev::block::block_iter(self, index)?;

                let frame = kmem::Frame::new().ok_or(ENOMEM)?;

                futures_util::future::try_join_all(block_iter.map(|(block, buf_range)| {
                    let mut ptr = frame.as_ptr();
                    let buf = &mut unsafe { ptr.as_mut() }[buf_range];

                    self.read(block, buf)
                }))
                .await?;

                Ok(alloc::sync::Arc::new(frame))
            }

            async fn flush(&self, index: usize, frame: &kmem::Frame) -> Result<(), Error> {
                log::trace!(
                    "BlockBackend::flush: index = {index}, frame = {:?}",
                    frame.base()
                );

                let block_iter = $crate::dev::block::block_iter(self, index)?;

                futures_util::future::try_join_all(block_iter.map(|(block, buf_range)| {
                    let ptr = frame.as_ptr();
                    let buf = &unsafe { ptr.as_ref() }[buf_range];

                    self.write(block, buf)
                }))
                .await?;

                Ok(())
            }
        }
    };
}

fn block_range(index: usize, block_shift: u32) -> Range<usize> {
    let nr_blocks_in_frame_shift = PAGE_SHIFT - block_shift;

    (index << nr_blocks_in_frame_shift)..((index + 1) << nr_blocks_in_frame_shift)
}

pub(crate) fn block_iter(
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
