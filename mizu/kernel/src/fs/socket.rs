use alloc::{boxed::Box, sync::Arc};
use core::{ops::Deref, pin::pin};

use arsc_rs::Arsc;
use async_trait::async_trait;
use devices::net::{tcp, udp, Config, Socket, Stack, StaticConfigV4};
use futures_util::future::{select, Either};
use ksc::{
    Error,
    Error::{ENODEV, ENOSYS, ESPIPE},
};
use smoltcp::wire::{Ipv4Address, Ipv4Cidr};
use spin::Once;
use umifs::{
    path::Path,
    traits::Entry,
    types::{FileType, Metadata, OpenOptions, Permissions},
};
use umio::{advance_slices, Event, Io, IoPoll, IoSlice, IoSliceMut, SeekFrom};

static STACK: Once<Arsc<Stack>> = Once::INIT;

#[cfg(feature = "qemu-virt")]
fn config() -> Config {
    Config {
        ipv4: devices::net::ConfigV4::Static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(10, 0, 2, 15), 24),
            gateway: Some(Ipv4Address::new(10, 0, 2, 2)),
            dns_servers: [Ipv4Address::new(8, 8, 8, 8)].into_iter().collect(),
        }),
        ipv6: devices::net::ConfigV6::None,
    }
}
#[cfg(not(feature = "qemu-virt"))]
fn config() -> Config {
    Default::default()
}

pub fn init_stack() {
    let _ = STACK.try_call_once(|| {
        if let Some(dev) = crate::dev::net(0) {
            return Ok(Stack::new(dev, config()));
        }
        Err(())
    });
}

#[allow(dead_code)]
pub fn tcp() -> Result<Arc<dyn Entry>, Error> {
    Ok(Arc::new(SocketFile {
        socket: Socket::Tcp(tcp::Socket::new(STACK.get().cloned().ok_or(ENODEV)?)),
    }))
}

#[allow(dead_code)]
pub fn udp() -> Result<Arc<dyn Entry>, Error> {
    Ok(Arc::new(SocketFile {
        socket: Socket::Udp(udp::Socket::new(STACK.get().cloned().ok_or(ENODEV)?)),
    }))
}

#[derive(Debug)]
pub struct SocketFile {
    socket: Socket,
}

impl Deref for SocketFile {
    type Target = Socket;

    fn deref(&self) -> &Socket {
        &self.socket
    }
}

#[async_trait]
impl Entry for SocketFile {
    async fn open(
        self: Arc<Self>,
        _: &Path,
        _: OpenOptions,
        _: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        Err(ENOSYS)
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::SOCK,
            len: 0,
            offset: 0,
            perm: Permissions::all_same(true, true, false),
            block_size: 0,
            block_count: 0,
            times: Default::default(),
        }
    }
}

#[async_trait]
impl IoPoll for SocketFile {
    async fn event(&self, expected: Event) -> Option<Event> {
        let send = expected.contains(Event::WRITABLE);
        let recv = expected.contains(Event::READABLE);
        match (send, recv) {
            (true, false) => {
                self.socket.wait_for_send().await;
                Some(Event::WRITABLE)
            }
            (false, true) => {
                self.socket.wait_for_recv().await;
                Some(Event::READABLE)
            }
            (false, false) => None,
            (true, true) => {
                let send = pin!(self.socket.wait_for_send());
                let recv = pin!(self.socket.wait_for_recv());
                match select(send, recv).await {
                    Either::Left(_) => Some(Event::WRITABLE),
                    Either::Right(_) => Some(Event::READABLE),
                }
            }
        }
    }
}

#[async_trait]
impl Io for SocketFile {
    async fn read(&self, mut buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        let mut read_len = 0;
        loop {
            let Some(buf) = buffer.first_mut() else {
                break Ok(read_len);
            };
            let len = self.socket.receive(buf, None).await?;
            read_len += len;
            advance_slices(&mut buffer, len)
        }
    }

    async fn write(&self, mut buffer: &mut [IoSlice]) -> Result<usize, Error> {
        let mut written_len = 0;
        loop {
            let Some(buf) = buffer.first() else {
                break Ok(written_len);
            };
            let len = self.socket.send(buf, None).await?;
            written_len += len;
            advance_slices(&mut buffer, len)
        }
    }

    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(ESPIPE)
    }

    async fn flush(&self) -> Result<(), Error> {
        self.socket.flush().await;
        Ok(())
    }
}
