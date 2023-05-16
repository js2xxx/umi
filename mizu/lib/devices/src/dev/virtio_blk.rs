use alloc::boxed::Box;
use core::iter;

use async_trait::async_trait;
use ksc::Error::{self, EINVAL, EIO, ENOBUFS, ENOMEM, EPERM};
use ksync::{event::Event, Semaphore};
use spin::lock_api::Mutex;
use static_assertions::const_assert;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use super::{block::Block, HalImpl};

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
                virt_queue: Semaphore::new(size / 3),
                device: Mutex::new(device),
                event: iter::repeat_with(Event::new).take(size).collect(),
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

    pub fn capacity_blocks(&self) -> usize {
        unsafe { (*self.device.data_ptr()).capacity() as usize }
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
                Some(listener) => {
                    // log::trace!("VirtioBlock::wait_for_token: token = {token}");
                    listener.await
                }
                None => listener = Some(self.event[token as usize].listen()),
            }
        }
    }

    pub async fn read_chunk(&self, block: usize, buf: &mut [u8]) -> virtio_drivers::Result {
        // log::trace!(
        //     "VirtioBlock::read_chunk: block = {block}, buf len = {}",
        //     buf.len()
        // );
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

    pub async fn write_chunk(&self, block: usize, buf: &[u8]) -> virtio_drivers::Result {
        // log::trace!(
        //     "VirtioBlock::write_chunk: block = {block}, buf len = {}",
        //     buf.len()
        // );
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
}

fn virtio_rw_err(err: virtio_drivers::Error) -> Error {
    log::error!("virt IO error: {err:?}");
    match err {
        virtio_drivers::Error::QueueFull => ENOBUFS,
        virtio_drivers::Error::InvalidParam => EINVAL,
        virtio_drivers::Error::DmaError => ENOMEM,
        virtio_drivers::Error::IoError => EIO,
        virtio_drivers::Error::Unsupported => EPERM,
        _ => unreachable!("{err}"),
    }
}

const_assert!(VirtioBlock::SECTOR_SIZE.is_power_of_two());
#[async_trait]
impl Block for VirtioBlock {
    fn block_shift(&self) -> u32 {
        Self::SECTOR_SIZE.trailing_zeros()
    }

    fn capacity_blocks(&self) -> usize {
        self.capacity_blocks()
    }

    fn ack_interrupt(&self) {
        self.ack_interrupt()
    }

    async fn read(&self, block: usize, buf: &mut [u8]) -> Result<usize, Error> {
        let len = buf.len().min(Self::SECTOR_SIZE);
        let res = self.read_chunk(block, &mut buf[..len]).await;
        res.map_err(virtio_rw_err).map(|_| len)
    }

    async fn write(&self, block: usize, buf: &[u8]) -> Result<usize, Error> {
        let len = buf.len().min(Self::SECTOR_SIZE);
        let res = self.write_chunk(block, &buf[..len]).await;
        res.map_err(virtio_rw_err).map(|_| len)
    }
}
impl_io_for_block!(VirtioBlock);
