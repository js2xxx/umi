use ksync::event::Event;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use crate::{dev::VirtioHal, Interrupt};

pub struct VirtioBlock<H: VirtioHal> {
    inner: VirtIOBlk<H, MmioTransport>,
    intr: Interrupt,
    can_submit: Event,
}

impl<H: VirtioHal> VirtioBlock<H> {
    pub const SECTOR_SIZE: usize = virtio_drivers::device::blk::SECTOR_SIZE;

    pub fn new(mmio: MmioTransport, intr: Interrupt) -> Option<Self> {
        VirtIOBlk::new(mmio)
            .map(|inner| VirtioBlock {
                inner,
                intr,
                can_submit: Event::new(),
            })
            .ok()
    }

    pub fn capacity_blocks(&self) -> u64 {
        self.inner.capacity()
    }

    #[inline]
    async fn submit<F>(&mut self, mut submit: F) -> virtio_drivers::Result<u16>
    where
        F: FnMut(&mut VirtIOBlk<H, MmioTransport>) -> virtio_drivers::Result<u16>,
    {
        let mut can_submit = None;
        loop {
            match submit(&mut self.inner) {
                Ok(token) => break Ok(token),
                Err(virtio_drivers::Error::QueueFull) => match can_submit.take() {
                    Some(can_submit) => can_submit.await,
                    None => can_submit = Some(self.can_submit.listen()),
                },
                Err(err) => break Err(err),
            }
        }
    }

    async fn read_chunk(&mut self, block: usize, buf: &mut [u8]) -> virtio_drivers::Result {
        assert!(buf.len() <= Self::SECTOR_SIZE);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();
        let token = self
            .submit(|inner| unsafe { inner.read_block_nb(block, &mut req, buf, &mut resp) })
            .await?;

        while self.inner.peek_used() != Some(token) {
            self.intr.wait().await;
            self.inner.ack_interrupt();
        }

        unsafe { self.inner.complete_read_block(token, &req, buf, &mut resp) }?;
        self.can_submit.notify_additional(1);

        resp.status().into()
    }

    async fn write_chunk(&mut self, block: usize, buf: &[u8]) -> virtio_drivers::Result {
        assert!(buf.len() <= Self::SECTOR_SIZE);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();
        let token = self
            .submit(|inner| unsafe { inner.write_block_nb(block, &mut req, buf, &mut resp) })
            .await?;

        while self.inner.peek_used() != Some(token) {
            self.intr.wait().await;
            self.inner.ack_interrupt();
        }

        unsafe { self.inner.complete_write_block(token, &req, buf, &mut resp) }?;
        self.can_submit.notify_additional(1);

        resp.status().into()
    }

    pub async fn read(&mut self, mut block: usize, buf: &mut [u8]) -> virtio_drivers::Result {
        for chunk in buf.chunks_mut(Self::SECTOR_SIZE) {
            self.read_chunk(block, chunk).await?;
            block += 1;
        }
        Ok(())
    }

    pub async fn write(&mut self, mut block: usize, buf: &[u8]) -> virtio_drivers::Result {
        for chunk in buf.chunks(Self::SECTOR_SIZE) {
            self.write_chunk(block, chunk).await?;
            block += 1;
        }
        Ok(())
    }
}
