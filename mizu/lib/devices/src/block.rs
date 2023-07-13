use alloc::boxed::Box;

use async_trait::async_trait;
use ksc::Error;
use umio::Io;

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
}

#[macro_export]
macro_rules! impl_io_for_block {
    ($type:ident) => {
        #[async_trait]
        impl umio::Io for $type {
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
                let block_shift = self.block_shift();
                if offset & ((1 << block_shift) - 1) != 0 {
                    return Ok(0);
                }
                let cap = self.capacity_blocks();
                let mut block = offset >> block_shift;
                let mut read_len = 0;
                loop {
                    if buffer.is_empty() || block >= cap {
                        break Ok(read_len);
                    }
                    let buf = &mut buffer[0];
                    let len = buf.len().min((cap - block) << block_shift);
                    let actual_len = Block::read(self, block, &mut buf[..len]).await?;
                    read_len += actual_len;
                    if actual_len < len || len < buf.len() {
                        break Ok(read_len);
                    }
                    umio::advance_slices(&mut buffer, actual_len);
                    block += actual_len >> block_shift;
                }
            }

            async fn write_at(
                &self,
                offset: usize,
                mut buffer: &mut [umio::IoSlice],
            ) -> Result<usize, ksc::Error> {
                let block_shift = self.block_shift();
                if offset & ((1 << block_shift) - 1) != 0 {
                    return Ok(0);
                }
                let cap = self.capacity_blocks();
                let mut block = offset >> block_shift;
                let mut written_len = 0;
                loop {
                    if buffer.is_empty() || block >= cap {
                        break Ok(written_len);
                    }
                    let buf = buffer[0];
                    let len = buf.len().min((cap - block) << block_shift);
                    let actual_len = Block::write(self, block, &buf[..len]).await?;
                    written_len += actual_len;
                    if actual_len < len || len < buf.len() {
                        break Ok(written_len);
                    }
                    umio::advance_slices(&mut buffer, actual_len);
                    block += actual_len >> block_shift;
                }
            }

            async fn flush(&self) -> Result<(), Error> {
                Ok(())
            }
        }
    };
}
