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

use super::{BUFFER_CAP, META_CAP};
use crate::net::Stack;

#[derive(Debug)]
pub struct Socket {
    stack: Arsc<Stack>,
    handle: SocketHandle,
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

    pub fn connect(&self, remote: impl Into<IpEndpoint>) -> Result<(), Error> {
        let mut endpoint: IpEndpoint = remote.into();
        if endpoint.port == 0 {
            endpoint.port = self.stack.with_socket_mut(|s| s.next_local_port());
        }
        self.with_mut(|socket| socket.connect(endpoint))
            .map_err(|_| EINVAL)
    }

    pub fn poll_receive(
        &self,
        buf: &mut [u8],
        cx: &mut Context,
    ) -> Poll<Result<(usize, IpEndpoint), Error>> {
        if !self.bound.load(SeqCst) {
            self.bind(IpListenEndpoint::default())?;
        }
        loop {
            if let Some(res) = self.with_mut(|socket| match socket.recv_slice(buf) {
                Ok((n, meta)) if Some(meta.endpoint) == socket.remote_endpoint() => {
                    Some(Poll::Ready(Ok((n, meta.endpoint))))
                }
                Ok(_) => None,
                Err(udp::RecvError::Exhausted) => {
                    socket.register_recv_waker(cx.waker());
                    Some(Poll::Pending)
                }
            }) {
                break res;
            }
        }
    }

    pub async fn receive(&self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), Error> {
        poll_fn(|cx| self.poll_receive(buf, cx)).await
    }

    pub fn poll_send(
        &self,
        buf: &[u8],
        remote_endpoint: Option<IpEndpoint>,
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
        let remote_endpoint = remote_endpoint.map(Into::into);
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

    pub fn poll_flush(&self, cx: &mut Context) -> Poll<()> {
        self.with_mut(|s| {
            if s.send_queue() == 0 {
                Poll::Ready(())
            } else {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    pub async fn flush(&self) {
        poll_fn(|cx| self.poll_flush(cx)).await
    }

    pub fn listen_endpoint(&self) -> IpListenEndpoint {
        self.with(|socket| socket.endpoint())
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.with(|socket| socket.remote_endpoint())
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
