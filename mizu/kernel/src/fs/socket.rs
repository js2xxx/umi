use alloc::{boxed::Box, sync::Arc};
use core::{iter, ops::Deref, pin::pin, time::Duration};

use arsc_rs::Arsc;
use async_trait::async_trait;
use devices::net::{tcp, udp, Config, Socket, Stack};
use futures_util::future::{select, Either};
use ksc::{
    Error,
    Error::{ENODEV, ENOSYS, ESPIPE},
};
use smoltcp::wire::IpEndpoint;
use spin::{mutex::Mutex, Once};
use umifs::{
    path::Path,
    traits::Entry,
    types::{FileType, Metadata, OpenOptions, Permissions},
};
use umio::{advance_slices, Event, Io, IoPoll, IoSlice, IoSliceMut, SeekFrom};

use crate::trap::poll_with;

static STACK: Once<Arsc<Stack>> = Once::INIT;

fn config() -> Config {
    Default::default()
}

pub(super) fn init_stack() {
    let _ = STACK.call_once(|| {
        let pairs = crate::dev::nets()
            .into_iter()
            .zip(iter::repeat_with(config));
        let stack = Stack::new(pairs);
        let s2 = stack.clone();
        crate::executor().spawn(s2.run()).detach();
        stack
    });
}

pub fn tcp() -> Result<Arc<dyn Entry>, Error> {
    log::trace!("Created new TCP socket");
    Ok(Arc::new(SocketFile::new(Socket::Tcp(tcp::Socket::new(
        STACK.get().cloned().ok_or(ENODEV)?,
    )))))
}

pub fn tcp_accept(socket: Socket) -> Arc<dyn Entry> {
    log::trace!("Accepted new TCP socket");
    Arc::new(SocketFile::new(socket))
}

pub fn udp() -> Result<Arc<dyn Entry>, Error> {
    log::trace!("Created new UDP socket");
    Ok(Arc::new(SocketFile::new(Socket::Udp(udp::Socket::new(
        STACK.get().cloned().ok_or(ENODEV)?,
    )))))
}

#[derive(Debug)]
pub struct SocketFile {
    socket: Socket,
    pub send_timeout: Mutex<Option<Duration>>,
    pub recv_timeout: Mutex<Option<Duration>>,
}

impl SocketFile {
    fn new(socket: Socket) -> Self {
        SocketFile {
            socket,
            send_timeout: Default::default(),
            recv_timeout: Default::default(),
        }
    }

    pub async fn send(
        &self,
        mut buffer: &mut [IoSlice<'_>],
        endpoint: Option<IpEndpoint>,
        nonblock: bool,
    ) -> Result<usize, Error> {
        let timeout = if nonblock {
            Some(Duration::ZERO)
        } else {
            ksync::critical(|| *self.send_timeout.lock())
        };

        let mut sent_len = 0;
        loop {
            let Some(buf) = buffer.first() else {
                break Ok(sent_len);
            };
            let send = poll_with((**self).send(buf, endpoint), timeout);
            match send.await {
                Ok(len) => {
                    sent_len += len;
                    advance_slices(&mut buffer, len)
                }
                Err(_) if sent_len != 0 => break Ok(sent_len),
                Err(err) => break Err(err),
            }
        }
    }

    pub async fn receive(
        &self,
        mut buffer: &mut [IoSliceMut<'_>],
        nonblock: bool,
    ) -> Result<(usize, Option<IpEndpoint>), Error> {
        log::trace!("user recv buffer len = {}", umio::ioslice_len(&buffer));
        log::trace!(
            "socket is {}",
            match &**self {
                Socket::Tcp(_) => "TCP",
                Socket::Udp(_) => "UDP",
            }
        );

        let mut timeout = if nonblock {
            Some(Duration::ZERO)
        } else {
            ksync::critical(|| *self.send_timeout.lock())
        };

        match &**self {
            Socket::Tcp(socket) => {
                let mut received_len = 0;
                loop {
                    let Some(buf) = buffer.first_mut() else {
                        break Ok((received_len, None));
                    };
                    match poll_with(socket.receive(buf), timeout).await {
                        Ok(len) => {
                            received_len += len;
                            if len == 0 {
                                return Ok((received_len, None));
                            }
                            timeout = Some(Duration::ZERO);
                            advance_slices(&mut buffer, len);
                        }
                        Err(_) if received_len != 0 => break Ok((received_len, None)),
                        Err(err) => break Err(err),
                    }
                }
            }
            Socket::Udp(socket) => Ok(match buffer.first_mut() {
                Some(buf) => {
                    let receive = poll_with(socket.receive(buf), timeout);
                    let (received_len, endpoint) = receive.await?;
                    (received_len, Some(endpoint))
                }
                None => (0, None),
            }),
        }
    }
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
        if self.socket.is_closed() {
            return Some(Event::HANG_UP);
        }
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
                    Either::Left((_, recv)) => match ksync::poll_once(recv) {
                        Some(()) => Some(Event::WRITABLE | Event::READABLE),
                        None => Some(Event::WRITABLE),
                    },
                    Either::Right((_, send)) => match ksync::poll_once(send) {
                        Some(()) => Some(Event::READABLE | Event::WRITABLE),
                        None => Some(Event::READABLE),
                    },
                }
            }
        }
    }
}

#[async_trait]
impl Io for SocketFile {
    async fn read(&self, buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        self.receive(buffer, false).await.map(|(len, _)| len)
    }

    async fn write(&self, buffer: &mut [IoSlice]) -> Result<usize, Error> {
        self.send(buffer, None, false).await
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
