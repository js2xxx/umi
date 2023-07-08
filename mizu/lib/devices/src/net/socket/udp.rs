use alloc::vec;
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use ksc::Error::{self, EINVAL};
use smoltcp::{
    iface::SocketHandle,
    socket::udp::{self, PacketBuffer, PacketMetadata},
    wire::{IpEndpoint, IpListenEndpoint},
};
use spin::Mutex;

use super::{BUFFER_CAP, META_CAP};
use crate::net::Stack;

#[derive(Debug)]
pub struct Socket {
    stack: Arsc<Stack>,
    handle: SocketHandle,
    remote: Mutex<Option<IpEndpoint>>,
    bound: AtomicBool,
}

impl Socket {
    fn with_mut<T>(&self, f: impl FnOnce(&mut udp::Socket) -> T) -> T {
        self.stack
            .with_socket_mut(|s| f(s.sockets.get_mut(self.handle)))
    }
    fn with<T>(&self, f: impl FnOnce(&udp::Socket) -> T) -> T {
        self.stack.with_socket(|s| f(s.sockets.get(self.handle)))
    }
}

impl Socket {
    pub fn new(stack: Arsc<Stack>) -> Self {
        let meta = vec![PacketMetadata::EMPTY; META_CAP];
        let buf = vec![0; BUFFER_CAP];

        let socket = udp::Socket::new(
            PacketBuffer::new(meta.clone(), buf.clone()),
            PacketBuffer::new(meta, buf),
        );
        let handle = stack.with_socket_mut(|s| s.sockets.add(socket));
        Socket {
            stack,
            handle,
            remote: Default::default(),
            bound: Default::default(),
        }
    }

    pub fn bind(&self, endpoint: impl Into<IpListenEndpoint>) -> Result<(), Error> {
        let mut endpoint: IpListenEndpoint = endpoint.into();
        if endpoint.port == 0 {
            endpoint.port = self.stack.with_socket_mut(|s| s.next_local_port());
        }
        self.with_mut(|socket| socket.bind(endpoint))
            .map_or(Err(EINVAL), |_| {
                self.bound.store(true, SeqCst);
                Ok(())
            })
    }

    pub fn connect(&self, remote: impl Into<IpEndpoint>) {
        ksync::critical(|| *self.remote.lock() = Some(remote.into()))
    }

    pub fn poll_receive(&self, buf: &mut [u8], cx: &mut Context) -> Poll<Result<(usize, IpEndpoint), Error>> {
        if !self.bound.load(SeqCst) {
            self.bind(IpListenEndpoint::default())?;
        }
        self.with_mut(|socket| match socket.recv_slice(buf) {
            Ok((n, meta)) => Poll::Ready(Ok((n, meta.endpoint))),
            Err(udp::RecvError::Exhausted) => {
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    pub async fn receive(&self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), Error> {
        loop {
            let (len, endpoint) = poll_fn(|cx| self.poll_receive(buf, cx)).await?;
            let remote = ksync::critical(|| *self.remote.lock());
            if remote.is_none() || Some(endpoint) == remote {
                break Ok((len, endpoint));
            }
        }
    }

    pub fn poll_send(
        &self,
        buf: &[u8],
        remote_endpoint: IpEndpoint,
        cx: &mut Context,
    ) -> Poll<Result<usize, Error>> {
        if !self.bound.load(SeqCst) {
            self.bind(IpListenEndpoint::default())?;
        }
        self.with_mut(|socket| match socket.send_slice(buf, remote_endpoint) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(udp::SendError::BufferFull) => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            Err(udp::SendError::Unaddressable) => Poll::Ready(Err(EINVAL)),
        })
    }

    pub async fn send(
        &self,
        buf: &[u8],
        remote_endpoint: Option<impl Into<IpEndpoint>>,
    ) -> Result<usize, Error> {
        let remote_endpoint: IpEndpoint = match remote_endpoint {
            Some(remote) => remote.into(),
            None => ksync::critical(|| self.remote.lock().ok_or(EINVAL))?,
        };
        poll_fn(|cx| self.poll_send(buf, remote_endpoint, cx)).await
    }

    fn poll_wait_for_recv(&self, cx: &mut Context) -> Poll<()> {
        self.with_mut(|s| {
            if s.can_recv() {
                Poll::Ready(())
            } else {
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    pub async fn wait_for_recv(&self) {
        poll_fn(|cx| self.poll_wait_for_recv(cx)).await
    }

    fn poll_wait_for_send(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.with_mut(|s| {
            if s.can_send() {
                Poll::Ready(())
            } else {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    pub async fn wait_for_send(&self) {
        poll_fn(|cx| self.poll_wait_for_send(cx)).await
    }

    pub fn listen_endpoint(&self) -> IpListenEndpoint {
        self.with(|socket| socket.endpoint())
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        ksync::critical(|| *self.remote.lock())
    }

    pub fn is_open(&self) -> bool {
        self.with(|socket| socket.is_open())
    }

    pub fn close(&self) {
        self.with_mut(|socket| socket.close())
    }

    pub fn can_send(&self) -> bool {
        self.with(|socket| socket.can_send())
    }

    pub fn can_recv(&self) -> bool {
        self.with(|socket| socket.can_recv())
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.stack
            .with_socket_mut(|s| s.sockets.remove(self.handle));
    }
}
