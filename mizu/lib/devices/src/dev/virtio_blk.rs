use alloc::{boxed::Box, vec::Vec};
use core::{
    iter, mem,
    pin::Pin,
    task::{ready, Context, Poll},
};

use async_trait::async_trait;
use futures_util::{stream, Future, FutureExt, StreamExt, TryStreamExt};
use ksc::Error::{self, EINVAL, EIO, ENOBUFS, ENOMEM, EPERM};
use ksync::{
    event::{Event, EventListener},
    Semaphore,
};
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
    token: Box<[Token]>,
}

unsafe impl Send for VirtioBlock {}
unsafe impl Sync for VirtioBlock {}

impl VirtioBlock {
    pub const SECTOR_SIZE: usize = virtio_drivers::device::blk::SECTOR_SIZE;
    const OP_COUNT: usize = 3;

    pub fn new(mmio: MmioTransport) -> Result<Self, virtio_drivers::Error> {
        VirtIOBlk::new(mmio).map(|device| {
            let size = device.virt_queue_size() as usize;

            VirtioBlock {
                virt_queue: Semaphore::new(size / Self::OP_COUNT),
                device: Mutex::new(device),
                token: iter::repeat_with(Token::default).take(size).collect(),
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
            unsafe {
                self.token[used as usize].receive();
            }
        }
    }

    pub fn capacity_blocks(&self) -> usize {
        unsafe { (*self.device.data_ptr()).capacity() as usize }
    }

    pub fn readonly(&self) -> bool {
        unsafe { (*self.device.data_ptr()).readonly() }
    }

    pub fn max_concurrents(&self) -> usize {
        unsafe { (*self.device.data_ptr()).virt_queue_size() as usize / Self::OP_COUNT }
    }

    #[inline]
    fn i<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut VirtIOBlk<HalImpl, MmioTransport>) -> T,
    {
        ksync::critical(|| func(&mut self.device.lock()))
    }

    pub fn read_chunk(&self, block: usize, buf: Vec<u8>) -> ReadChunk {
        ReadChunk {
            device: self,
            buf,
            block,
            state: ChunkState::Acquiring(self.virt_queue.acquire()),
        }
    }

    pub fn write_chunk(&self, block: usize, buf: Vec<u8>) -> WriteChunk {
        WriteChunk {
            device: self,
            buf,
            block,
            state: ChunkState::Acquiring(self.virt_queue.acquire()),
        }
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
        let iter = stream::iter(buf.chunks_mut(Self::SECTOR_SIZE).zip(block..)).map(Ok);
        let fut = iter.try_for_each_concurrent(Some(self.max_concurrents()), |(buf, block)| {
            self.read_chunk(block, buf.to_vec())
                .map(|res| res.map(|res| buf.copy_from_slice(&res)))
        });
        fut.await.map(|_| buf.len()).map_err(virtio_rw_err)
    }

    async fn write(&self, block: usize, buf: &[u8]) -> Result<usize, Error> {
        let iter = stream::iter(buf.chunks(Self::SECTOR_SIZE).zip(block..)).map(Ok);
        let fut = iter.try_for_each_concurrent(Some(self.max_concurrents()), |(buf, block)| {
            self.write_chunk(block, buf.to_vec())
                .map(|res| res.map(drop))
        });
        fut.await.map(|_| buf.len()).map_err(virtio_rw_err)
    }
}
impl_io_for_block!(VirtioBlock);

#[allow(unused)]
struct Request {
    req: BlkReq,
    resp: BlkResp,
    buf: Vec<u8>,
}

struct Token {
    event: Event,
    discard: Mutex<Option<Request>>,
}

impl Token {
    /// # Safety
    ///
    /// The function must be called after the request is completed.
    unsafe fn receive(&self) {
        ksync::critical(|| *self.discard.lock() = None);
        self.event.notify_additional_relaxed(1)
    }
}

impl Default for Token {
    fn default() -> Self {
        Token {
            event: Event::new(),
            discard: Mutex::new(None),
        }
    }
}

enum ChunkState<'a> {
    Acquiring(ksync::Acquire<'a>),
    Waiting {
        token: u16,
        req: BlkReq,
        resp: BlkResp,

        _guard: ksync::SemaphoreGuard<'a>,
        listener: Option<EventListener>,
    },
    Complete,
}

impl ChunkState<'_> {
    fn poll<Submit, Peek, Complete>(
        &mut self,
        cx: &mut Context<'_>,
        tokens: &[Token],
        buf: &mut Vec<u8>,
        submit: Submit,
        peek: Peek,
        complete: Complete,
    ) -> Poll<virtio_drivers::Result<Vec<u8>>>
    where
        Submit: Fn(&mut BlkReq, &mut BlkResp, &mut [u8]) -> virtio_drivers::Result<u16>,
        Peek: Fn() -> Option<u16>,
        Complete: Fn(u16, &BlkReq, &mut BlkResp, &mut [u8]) -> virtio_drivers::Result,
    {
        loop {
            match self {
                ChunkState::Acquiring(acq) => {
                    let _guard = ready!(acq.poll_unpin(cx));

                    let mut req = BlkReq::default();
                    let mut resp = BlkResp::default();

                    let token = submit(&mut req, &mut resp, buf)?;
                    // log::trace!("VirtioBlock::poll: submitted, token = {token:?}");
                    *self = ChunkState::Waiting {
                        _guard,
                        token,
                        req,
                        resp,
                        listener: None,
                    }
                }
                ChunkState::Waiting {
                    _guard,
                    token,
                    req,
                    resp,
                    listener,
                } => {
                    // log::trace!("VirtioBlock::poll: peek_used, token = {token:?}");
                    let used = peek();
                    if used != Some(*token) {
                        match listener {
                            Some(l) => {
                                // log::trace!("VirtioBlock::poll: wait for token = {token}");
                                ready!(l.poll_unpin(cx));
                                *listener = None;
                            }
                            None => *listener = Some(tokens[*token as usize].event.listen()),
                        }
                        continue;
                    }

                    // log::trace!("VirtioBlock::poll: complete, used = {used:?}");
                    match complete(*token, req, resp, buf) {
                        // Sometimes it returns an erroneous not-ready, resulting from writing
                        // `BlkResp` with raw `NOT_READY` value (or even not writing it, leaving the
                        // default `NOT_READY` value). If we continue the loop on waiting it, the
                        // procedure will be hang on waiting the next never-coming event.
                        //
                        // We are sure that `peek_used` has returned our token, so we ignore it by
                        // far. Maybe it results from a potential memory access.
                        Ok(()) | Err(virtio_drivers::Error::NotReady) => {}
                        Err(err) => return Poll::Ready(Err(err)),
                    }

                    *self = ChunkState::Complete;
                    break Poll::Ready(Ok(mem::take(buf)));
                }
                ChunkState::Complete => unreachable!("polling after complete"),
            }
        }
    }

    fn discard(&mut self, tokens: &[Token], buf: &mut Vec<u8>) {
        let state = mem::replace(self, ChunkState::Complete);
        if let ChunkState::Waiting {
            token, req, resp, ..
        } = state
        {
            ksync::critical(|| {
                *tokens[token as usize].discard.lock() = Some(Request {
                    req,
                    resp,
                    buf: mem::take(buf),
                })
            })
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReadChunk<'a> {
    device: &'a VirtioBlock,
    buf: Vec<u8>,
    block: usize,
    state: ChunkState<'a>,
}

impl Unpin for ReadChunk<'_> {}

impl Future for ReadChunk<'_> {
    type Output = virtio_drivers::Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        this.state.poll(
            cx,
            &this.device.token,
            &mut this.buf,
            |req, resp, buf| {
                this.device
                    .i(|blk| unsafe { blk.read_block_nb(this.block, req, buf, resp) })
            },
            || this.device.i(|blk| blk.peek_used()),
            |token, req, resp, buf| {
                this.device
                    .i(|blk| unsafe { blk.complete_read_block(token, req, buf, resp) })
            },
        )
    }
}

impl Drop for ReadChunk<'_> {
    fn drop(&mut self) {
        self.state.discard(&self.device.token, &mut self.buf);
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct WriteChunk<'a> {
    device: &'a VirtioBlock,
    buf: Vec<u8>,
    block: usize,
    state: ChunkState<'a>,
}

impl Unpin for WriteChunk<'_> {}

impl Future for WriteChunk<'_> {
    type Output = virtio_drivers::Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        this.state.poll(
            cx,
            &this.device.token,
            &mut this.buf,
            |req, resp, buf| {
                this.device
                    .i(|blk| unsafe { blk.write_block_nb(this.block, req, buf, resp) })
            },
            || this.device.i(|blk| blk.peek_used()),
            |token, req, resp, buf| {
                this.device
                    .i(|blk| unsafe { blk.complete_write_block(token, req, buf, resp) })
            },
        )
    }
}

impl Drop for WriteChunk<'_> {
    fn drop(&mut self) {
        self.state.discard(&self.device.token, &mut self.buf);
    }
}
