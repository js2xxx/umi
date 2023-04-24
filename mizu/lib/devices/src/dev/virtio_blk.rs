use alloc::boxed::Box;
use core::iter;

use futures_util::future::try_join_all;
use ksync::{event::Event, Semaphore};
use spin::lock_api::Mutex;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use super::HalImpl;

pub struct VirtioBlock {
    virt_queue: Semaphore,
    device: Mutex<VirtIOBlk<HalImpl, MmioTransport>>,
    event: Box<[Event]>,
}

unsafe impl Send for VirtioBlock {}
unsafe impl Sync for VirtioBlock {}

impl VirtioBlock {
    pub const SECTOR_SIZE: usize = virtio_drivers::device::blk::SECTOR_SIZE;

    pub fn new(mmio: MmioTransport) -> Result<Self, virtio_drivers::Error> {
        VirtIOBlk::new(mmio).map(|device| {
            let size = device.virt_queue_size() as usize;

            VirtioBlock {
                device: Mutex::new(device),
                event: iter::repeat_with(Event::new).take(size).collect(),

                virt_queue: Semaphore::new(size),
            }
        })
    }

    pub fn ack_interrupt(&self) {
        let used = ksync::critical(|| {
            let mut blk = self.device.lock();
            blk.ack_interrupt();
            blk.peek_used()
        });
        if let Some(used) = used {
            self.event[used as usize].notify_additional(1);
        }
    }

    pub fn capacity_blocks(&self) -> u64 {
        unsafe { (*self.device.data_ptr()).capacity() }
    }

    pub fn readonly(&self) -> bool {
        unsafe { (*self.device.data_ptr()).readonly() }
    }

    pub fn virt_queue_size(&self) -> u16 {
        unsafe { (*self.device.data_ptr()).virt_queue_size() }
    }

    #[inline]
    fn i<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut VirtIOBlk<HalImpl, MmioTransport>) -> T,
    {
        ksync::critical(|| func(&mut self.device.lock()))
    }

    async fn wait_for_token(&self, token: u16) {
        let mut listener = None;
        loop {
            let used = self.i(|blk| blk.peek_used());
            if used == Some(token) {
                break;
            }
            match listener.take() {
                Some(listener) => listener.await,
                None => listener = Some(self.event[token as usize].listen()),
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
        if buf.len() <= Self::SECTOR_SIZE {
            return self.read_chunk(start_block, buf).await;
        }
        let iter = (start_block..).zip(buf.chunks_mut(Self::SECTOR_SIZE));
        let tasks = iter.map(|(block, chunk)| self.read_chunk(block, chunk));
        try_join_all(tasks).await?;
        Ok(())
    }

    pub async fn write(&self, start_block: usize, buf: &[u8]) -> virtio_drivers::Result {
        if self.readonly() {
            return Err(virtio_drivers::Error::Unsupported);
        }
        if buf.len() <= Self::SECTOR_SIZE {
            return self.write_chunk(start_block, buf).await;
        }
        let iter = (start_block..).zip(buf.chunks(Self::SECTOR_SIZE));
        let tasks = iter.map(|(block, chunk)| self.write_chunk(block, chunk));
        try_join_all(tasks).await?;
        Ok(())
    }
}
