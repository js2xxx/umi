use alloc::{boxed::Box, sync::Arc};

use async_trait::async_trait;
use futures_lite::future::yield_now;
use ksc::Error;
use umio::Io;

use crate::Interrupt;

#[async_trait]
pub trait Block: Io {
    fn block_shift(&self) -> u32;

    #[inline]
    fn block_size(&self) -> usize {
        1 << self.block_shift()
    }

    fn capacity_blocks(&self) -> usize;

    fn ack_interrupt(&self);

    async fn read(&self, block: usize, buf: &mut [u8]) -> Result<usize, Error>;

    async fn write(&self, block: usize, buf: &[u8]) -> Result<usize, Error>;

    async fn intr_dispatch(self: Arc<Self>, intr: Interrupt) {
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

macro_rules! impl_io_for_block {
    ($type:ident) => {
        #[async_trait]
        impl umio::Io for VirtioBlock {
            async fn seek(&self, whence: umio::SeekFrom) -> Result<usize, Error> {
                match whence {
                    umio::SeekFrom::End(0) => Ok(self.capacity_blocks() << self.block_shift()),
                    umio::SeekFrom::Start(0) | umio::SeekFrom::Current(0) => Ok(0),
                    _ => Err(ksc::ENOSYS),
                }
            }

            async fn stream_len(&self) -> Result<usize, Error> {
                Ok(self.capacity_blocks() << self.block_shift())
            }

            async fn read_at(
                &self,
                offset: usize,
                mut buffer: &mut [umio::IoSliceMut],
            ) -> Result<usize, ksc::Error> {
                if offset & ((1 << self.block_shift()) - 1) != 0 {
                    return Ok(0);
                }
                let mut block = offset >> self.block_shift();
                let mut read_len = 0;
                loop {
                    if buffer.is_empty() {
                        break Ok(read_len);
                    }
                    let buf = &mut buffer[0];
                    let len = Block::read(self, block, buf).await?;
                    read_len += len;
                    umio::advance_slices(&mut buffer, len);
                    if len < Self::SECTOR_SIZE {
                        break Ok(read_len);
                    }
                    block += len >> self.block_shift();
                }
            }

            async fn write_at(
                &self,
                offset: usize,
                mut buffer: &mut [umio::IoSlice],
            ) -> Result<usize, ksc::Error> {
                if offset & ((1 << self.block_shift()) - 1) != 0 {
                    return Ok(0);
                }
                let mut block = offset >> self.block_shift();
                let mut written_len = 0;
                loop {
                    if buffer.is_empty() {
                        break Ok(written_len);
                    }
                    let buf = buffer[0];
                    let len = Block::write(self, block, buf).await?;
                    written_len += len;
                    umio::advance_slices(&mut buffer, len);
                    if len < Self::SECTOR_SIZE {
                        break Ok(written_len);
                    }
                    block += len >> self.block_shift();
                }
            }

            async fn flush(&self) -> Result<(), Error> {
                Ok(())
            }
        }
    };
}
