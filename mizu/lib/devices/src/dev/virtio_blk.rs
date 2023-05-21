use alloc::{boxed::Box, vec::Vec};
use core::{
    fmt, iter, mem,
    pin::Pin,
    task::{ready, Context, Poll},
};

use async_trait::async_trait;
use futures_util::{stream, Future, FutureExt, StreamExt, TryStreamExt};
use ksc::Error::{self, EINVAL, EIO, ENOBUFS, ENOMEM, EPERM};
use ksync::event::{Event, EventListener};
use spin::lock_api::Mutex;
use static_assertions::const_assert;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use super::{block::Block, HalImpl};

pub struct VirtioBlock {
    virt_queue: Event,
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
                virt_queue: Event::new(),
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
                self.token[used as usize].receive(used, self);
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

    pub fn read_chunk(&self, block: usize, buf: Vec<u8>) -> ChunkOp {
        ChunkOp {
            device: self,
            buf,
            block,
            dir: Direction::Read,
            state: Default::default(),
        }
    }

    pub fn write_chunk(&self, block: usize, buf: Vec<u8>) -> ChunkOp {
        ChunkOp {
            device: self,
            buf,
            block,
            dir: Direction::Write,
            state: Default::default(),
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

#[derive(Clone, Copy)]
enum Direction {
    Read,
    Write,
}

impl fmt::Debug for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Read => "read",
            Direction::Write => "write",
        })
    }
}

#[allow(unused)]
struct Request {
    buf: Vec<u8>,
    dir: Direction,
    req: BlkReq,
    resp: BlkResp,
}

struct Token {
    event: Event,
    discard: Mutex<Option<Request>>,
}

impl Token {
    /// # Safety
    ///
    /// The function must be called after the request is completed.
    unsafe fn receive(&self, token: u16, device: &VirtioBlock) {
        if let Some(mut request) = ksync::critical(|| mem::take(&mut *self.discard.lock())) {
            // log::trace!(
            //     "VirtioBlock::discard: complete obsolete request {:?}({token})",
            //     request.dir
            // );
            let _ = device.i(|blk| match request.dir {
                Direction::Read => unsafe {
                    blk.complete_read_block(
                        token,
                        &request.req,
                        &mut request.buf,
                        &mut request.resp,
                    )
                },
                Direction::Write => unsafe {
                    blk.complete_write_block(token, &request.req, &request.buf, &mut request.resp)
                },
            });
            device.virt_queue.notify(1);
        } else {
            self.event.notify_additional_relaxed(1)
        }
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

enum ChunkState {
    Submitting {
        req: BlkReq,
        resp: BlkResp,

        listener: Option<EventListener>,
    },
    Waiting {
        token: u16,
        req: BlkReq,
        resp: BlkResp,

        listener: Option<EventListener>,
    },
    Complete,
}

impl Default for ChunkState {
    fn default() -> Self {
        ChunkState::Submitting {
            req: Default::default(),
            resp: Default::default(),
            listener: None,
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ChunkOp<'a> {
    device: &'a VirtioBlock,
    buf: Vec<u8>,
    block: usize,
    dir: Direction,
    state: ChunkState,
}

impl Unpin for ChunkOp<'_> {}

impl Future for ChunkOp<'_> {
    type Output = virtio_drivers::Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let dir = this.dir;

        loop {
            match &mut this.state {
                ChunkState::Submitting {
                    req,
                    resp,
                    listener,
                } => {
                    let res = this.device.i(|blk| match dir {
                        Direction::Read => unsafe {
                            blk.read_block_nb(this.block, req, &mut this.buf, resp)
                        },
                        Direction::Write => unsafe {
                            blk.write_block_nb(this.block, req, &this.buf, resp)
                        },
                    });
                    let token = match res {
                        Ok(token) => token,
                        Err(virtio_drivers::Error::QueueFull) => {
                            match listener {
                                Some(l) => {
                                    // log::trace!("VirtioBlock::poll: wait for virt queue");
                                    ready!(l.poll_unpin(cx));
                                    *listener = None;
                                }
                                None => *listener = Some(this.device.virt_queue.listen()),
                            }
                            continue;
                        }
                        Err(err) => break Poll::Ready(Err(err)),
                    };
                    // log::trace!("VirtioBlock::poll: submitted {dir:?}, token = {token:?}");
                    this.state = ChunkState::Waiting {
                        token,
                        req: mem::take(req),
                        resp: mem::take(resp),
                        listener: None,
                    }
                }
                ChunkState::Waiting {
                    token,
                    req,
                    resp,
                    listener,
                } => {
                    // log::trace!("VirtioBlock::poll: peek used for {dir:?}, token = {token:?}");
                    let used = this.device.i(|blk| blk.peek_used());
                    if used != Some(*token) {
                        match listener {
                            Some(l) => {
                                // log::trace!("VirtioBlock::poll: wait for token = {token}");
                                ready!(l.poll_unpin(cx));
                                *listener = None;
                            }
                            None => {
                                *listener = Some(this.device.token[*token as usize].event.listen())
                            }
                        }
                        continue;
                    }

                    // log::trace!("VirtioBlock::poll: complete {dir:?}, used = {used:?}");
                    match this.device.i(|blk| match dir {
                        Direction::Read => unsafe {
                            blk.complete_read_block(*token, req, &mut this.buf, resp)
                        },
                        Direction::Write => unsafe {
                            blk.complete_write_block(*token, req, &this.buf, resp)
                        },
                    }) {
                        // Sometimes it returns an erroneous not-ready, resulting from writing
                        // `BlkResp` with raw `NOT_READY` value (or even not writing it, leaving
                        // the default `NOT_READY` value). If we
                        // continue the loop on waiting it, the
                        // procedure will be hang on waiting the next never-coming event.
                        //
                        // We are sure that `peek_used` has returned our token, so we ignore it
                        // by far. Maybe it results from a potential
                        // memory access.
                        Ok(()) | Err(virtio_drivers::Error::NotReady) => {}
                        Err(err) => return Poll::Ready(Err(err)),
                    }

                    this.device.virt_queue.notify(1);
                    this.state = ChunkState::Complete;
                    break Poll::Ready(Ok(mem::take(&mut this.buf)));
                }
                ChunkState::Complete => unreachable!("polling after complete"),
            }
        }
    }
}

impl Drop for ChunkOp<'_> {
    fn drop(&mut self) {
        if let ChunkState::Waiting {
            token, req, resp, ..
        } = mem::replace(&mut self.state, ChunkState::Complete)
        {
            // log::trace!(
            //     "VirtioBlock::drop: sending {:?}({token}) to device's discard slot",
            //     self.dir
            // );
            ksync::critical(|| {
                *(&self.device.token)[token as usize].discard.lock() = Some(Request {
                    dir: self.dir,
                    req,
                    resp,
                    buf: mem::take(&mut self.buf),
                })
            })
        }
    }
}
