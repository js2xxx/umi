use alloc::{boxed::Box, vec::Vec};
use core::{
    fmt, iter, mem,
    pin::Pin,
    task::{ready, Context, Poll},
};

use async_trait::async_trait;
use crossbeam_queue::ArrayQueue;
use futures_util::{stream, Future, FutureExt, StreamExt, TryStreamExt};
use ksc::Error::{self, EINVAL, EIO, ENOBUFS, ENOMEM, EPERM};
use ksync::{
    channel::{
        oneshot,
        oneshot::{Receiver, Sender},
    },
    event::{Event, EventListener},
};
use spin::lock_api::Mutex;
use static_assertions::const_assert;
use virtio_drivers::{
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::mmio::MmioTransport,
};

use super::{block::Block, HalImpl};

type Token = ArrayQueue<Request>;

#[derive(Debug)]
struct Request {
    buf: Vec<u8>,
    dir: Direction,
    req: BlkReq,
    resp: BlkResp,
    ret: Sender<Vec<u8>>,
}

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
                token: iter::repeat_with(|| ArrayQueue::new(1))
                    .take(size)
                    .collect(),
            }
        })
    }

    pub fn ack_interrupt(&self) {
        loop {
            let used = ksync::critical(|| {
                let mut blk = self.device.lock();
                blk.ack_interrupt();
                blk.peek_used()
            });
            let Some(used) = used else { break };

            if let Some(mut request) = self.token[used as usize].pop() {
                // log::trace!(
                //     "VirtioBlock::ack_interrupt: complete request {:?}({used})",
                //     request.dir
                // );
                let _ = self.i(|blk| match request.dir {
                    Direction::Read => unsafe {
                        blk.complete_read_block(
                            used,
                            &request.req,
                            &mut request.buf,
                            &mut request.resp,
                        )
                    },
                    Direction::Write => unsafe {
                        blk.complete_write_block(
                            used,
                            &request.req,
                            &request.buf,
                            &mut request.resp,
                        )
                    },
                });
                self.virt_queue.notify(1);
                let _ = request.ret.send(request.buf);
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

    pub fn read_chunk(&self, block: usize, mut buf: Vec<u8>) -> ChunkOp {
        buf.truncate(Self::SECTOR_SIZE);
        ChunkOp {
            device: self,
            block,
            dir: Direction::Read,
            state: ChunkState::new(buf),
        }
    }

    pub fn write_chunk(&self, block: usize, mut buf: Vec<u8>) -> ChunkOp {
        buf.truncate(Self::SECTOR_SIZE);
        ChunkOp {
            device: self,
            block,
            dir: Direction::Write,
            state: ChunkState::new(buf),
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

enum ChunkState {
    Submitting {
        req: BlkReq,
        resp: BlkResp,
        buf: Vec<u8>,

        listener: Option<EventListener>,
    },
    Waiting {
        ret: Receiver<Vec<u8>>,
    },
    Complete,
}

impl ChunkState {
    fn new(buf: Vec<u8>) -> Self {
        ChunkState::Submitting {
            req: Default::default(),
            resp: Default::default(),
            buf,
            listener: None,
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ChunkOp<'a> {
    device: &'a VirtioBlock,
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
                    buf,
                    listener,
                } => {
                    let res = this.device.i(|blk| match dir {
                        Direction::Read => unsafe { blk.read_block_nb(this.block, req, buf, resp) },
                        Direction::Write => unsafe {
                            blk.write_block_nb(this.block, req, buf, resp)
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

                    let (tx, rx) = oneshot();
                    this.device.token[token as usize]
                        .push(Request {
                            buf: mem::take(buf),
                            dir: this.dir,
                            req: mem::take(req),
                            resp: mem::take(resp),
                            ret: tx,
                        })
                        .unwrap();
                    this.state = ChunkState::Waiting { ret: rx }
                }
                ChunkState::Waiting { ret } => {
                    let buf = ready!(ret.poll_unpin(cx));

                    this.state = ChunkState::Complete;
                    break Poll::Ready(buf.map_err(|_| virtio_drivers::Error::DmaError));
                }
                ChunkState::Complete => unreachable!("polling after complete"),
            }
        }
    }
}
