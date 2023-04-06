use futures_util::future::try_join_all;
use ksync::Semaphore;
use spin::lock_api::Mutex;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use crate::{dev::VirtioHal, Interrupt};

pub struct VirtioBlock<H: VirtioHal> {
    inner: Mutex<VirtIOBlk<H, MmioTransport>>,
    intr: Interrupt,
    virt_queue: Semaphore,
}

impl<H: VirtioHal> VirtioBlock<H> {
    pub const SECTOR_SIZE: usize = virtio_drivers::device::blk::SECTOR_SIZE;

    pub fn new(mmio: MmioTransport, intr: Interrupt) -> Option<Self> {
        VirtIOBlk::new(mmio).ok().map(|inner| {
            let virt_queue = Semaphore::new(inner.virt_queue_size() as usize);
            VirtioBlock {
                inner: Mutex::new(inner),
                intr,
                virt_queue,
            }
        })
    }

    pub fn capacity_blocks(&self) -> u64 {
        unsafe { (*self.inner.data_ptr()).capacity() }
    }

    pub fn readonly(&self) -> bool {
        unsafe { (*self.inner.data_ptr()).readonly() }
    }

    pub fn virt_queue_size(&self) -> u16 {
        unsafe { (*self.inner.data_ptr()).virt_queue_size() }
    }

    #[inline]
    fn i<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut VirtIOBlk<H, MmioTransport>) -> T,
    {
        ksync::critical(|| func(&mut self.inner.lock()))
    }

    async fn wait_for_token(&self, token: u16) {
        loop {
            self.intr.wait().await;
            let used = self.i(|inner| {
                inner.ack_interrupt();
                inner.peek_used()
            });
            if used == Some(token) {
                break;
            }
        }
    }

    async fn read_chunk(&self, block: usize, buf: &mut [u8]) -> virtio_drivers::Result {
        assert!(buf.len() <= Self::SECTOR_SIZE);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let res = self.virt_queue.acquire().await;
        let token = self.i(|blk| unsafe { blk.read_block_nb(block, &mut req, buf, &mut resp) })?;

        self.wait_for_token(token).await;

        self.i(|blk| unsafe { blk.complete_read_block(token, &req, buf, &mut resp) })?;
        drop(res);

        resp.status().into()
    }

    async fn write_chunk(&self, block: usize, buf: &[u8]) -> virtio_drivers::Result {
        assert!(buf.len() <= Self::SECTOR_SIZE);

        let mut req = BlkReq::default();
        let mut resp = BlkResp::default();

        let res = self.virt_queue.acquire().await;
        let token = self.i(|blk| unsafe { blk.write_block_nb(block, &mut req, buf, &mut resp) })?;

        self.wait_for_token(token).await;

        self.i(|blk| unsafe { blk.complete_write_block(token, &req, buf, &mut resp) })?;
        drop(res);

        resp.status().into()
    }

    pub async fn read(&self, start_block: usize, buf: &mut [u8]) -> virtio_drivers::Result {
        let iter = (start_block..).zip(buf.chunks_mut(Self::SECTOR_SIZE));
        let tasks = iter.map(|(block, chunk)| self.read_chunk(block, chunk));
        try_join_all(tasks).await?;
        Ok(())
    }

    pub async fn write(&self, start_block: usize, buf: &[u8]) -> virtio_drivers::Result {
        if self.readonly() {
            return Err(virtio_drivers::Error::Unsupported);
        }
        let iter = (start_block..).zip(buf.chunks(Self::SECTOR_SIZE));
        let tasks = iter.map(|(block, chunk)| self.write_chunk(block, chunk));
        try_join_all(tasks).await?;
        Ok(())
    }
}
