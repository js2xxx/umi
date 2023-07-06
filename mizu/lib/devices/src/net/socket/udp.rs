use alloc::vec;
use core::{
    future::poll_fn,
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use ksc::Error::{self, EEXIST, EINVAL};
use smoltcp::{
    iface::SocketHandle,
    socket::udp::{self, BindError, PacketBuffer, PacketMetadata},
    wire::{IpEndpoint, IpListenEndpoint},
};

use super::{BUFFER_CAP, META_CAP};
use crate::net::Stack;

#[derive(Debug)]
pub struct Socket {
    stack: Arsc<Stack>,
    handle: SocketHandle,
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
        Socket { stack, handle }
    }

    pub fn bind(&self, endpoint: impl Into<IpListenEndpoint>) -> Result<(), Error> {
        let mut endpoint: IpListenEndpoint = endpoint.into();
        if endpoint.port == 0 {
            endpoint.port = self.stack.with_socket_mut(|s| s.next_local_port());
        }
        match self.with_mut(|socket| socket.bind(endpoint)) {
            Ok(()) => Ok(()),
            Err(BindError::InvalidState) => Err(EEXIST),
            Err(BindError::Unaddressable) => Err(EINVAL),
        }
    }

    pub fn poll_receive(&self, buf: &mut [u8], cx: &mut Context) -> Poll<(usize, IpEndpoint)> {
        self.with_mut(|socket| match socket.recv_slice(buf) {
            Ok((n, meta)) => Poll::Ready((n, meta.endpoint)),
            Err(udp::RecvError::Exhausted) => {
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    pub async fn receive(&self, buf: &mut [u8]) -> (usize, IpEndpoint) {
        poll_fn(|cx| self.poll_receive(buf, cx)).await
    }

    pub fn poll_send(
        &self,
        buf: &[u8],
        remote_endpoint: IpEndpoint,
        cx: &mut Context,
    ) -> Poll<Result<usize, Error>> {
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
        remote_endpoint: impl Into<IpEndpoint>,
    ) -> Result<usize, Error> {
        let remote_endpoint: IpEndpoint = remote_endpoint.into();
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

    pub fn endpoint(&self) -> IpListenEndpoint {
        self.with(|socket| socket.endpoint())
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
