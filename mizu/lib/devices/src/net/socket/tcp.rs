use alloc::vec;
use core::{
    future::poll_fn,
    mem,
    task::{Context, Poll},
    time::Duration,
};

use arsc_rs::Arsc;
use managed::ManagedSlice;
use smoltcp::{
    iface::{Interface, SocketHandle},
    socket::tcp,
    wire::{IpEndpoint, IpListenEndpoint},
};
use spin::RwLock;

use super::BUFFER_CAP;
use crate::net::{time::duration_to_smoltcp, Stack};

/// Error returned by TcpSocket read/write functions.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Error {
    /// The connection was reset.
    ///
    /// This can happen on receiving a RST packet, or on timeout.
    ConnectionReset,
}

/// Error returned by [`TcpSocket::connect`] and [`TcpSocket::listen`].
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ConnectError {
    /// The socket is already connected or listening.
    InvalidState,
    /// The remote host rejected the connection with a RST packet.
    ConnectionReset,
    /// No route to host.
    NoRoute,
}

/// Error returned by [`TcpSocket::accept`].
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum AcceptError {
    /// The socket is already connected or listening.
    InvalidState,
}

#[derive(Debug)]
pub struct Socket {
    stack: Arsc<Stack>,
    handle: RwLock<SocketHandle>,
}

impl Socket {
    fn with_mut<T>(
        &self,
        f: impl FnOnce(SocketHandle, &mut tcp::Socket, &mut Interface) -> T,
    ) -> T {
        ksync::critical(|| {
            let handle = self.handle.read();
            self.stack
                .with_socket_mut(|s| f(*handle, s.sockets.get_mut(*handle), &mut s.iface))
        })
    }

    fn with<T>(&self, f: impl FnOnce(&tcp::Socket) -> T) -> T {
        ksync::critical(|| {
            let handle = self.handle.read();
            self.stack.with_socket(|s| f(s.sockets.get(*handle)))
        })
    }
}

impl Socket {
    pub fn new(stack: Arsc<Stack>) -> Self {
        let buf = vec![0u8; BUFFER_CAP];
        let socket = tcp::Socket::new(buf.clone().into(), ManagedSlice::Owned(buf));
        let handle = stack.with_socket_mut(|s| s.sockets.add(socket));
        Socket {
            stack,
            handle: RwLock::new(handle),
        }
    }

    fn poll_for_establishment(&self, cx: &mut Context) -> Poll<Result<SocketHandle, ConnectError>> {
        self.with_mut(|handle, s, _| match s.state() {
            tcp::State::Closed | tcp::State::TimeWait => {
                Poll::Ready(Err(ConnectError::ConnectionReset))
            }
            tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived => {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
            _ => Poll::Ready(Ok(handle)),
        })
    }

    pub async fn connect(&self, remote: impl Into<IpEndpoint>) -> Result<(), ConnectError> {
        let local_port = self.stack.with_socket_mut(|s| s.next_local_port());

        match self.with_mut(|_, socket, i| socket.connect(i.context(), remote, local_port)) {
            Ok(()) => {}
            Err(tcp::ConnectError::InvalidState) => return Err(ConnectError::InvalidState),
            Err(tcp::ConnectError::Unaddressable) => return Err(ConnectError::NoRoute),
        }

        poll_fn(|cx| self.poll_for_establishment(cx)).await?;
        Ok(())
    }

    pub fn listen(&self, local_endpoint: impl Into<IpListenEndpoint>) -> Result<(), ConnectError> {
        match self.with_mut(|_, socket, _| socket.listen(local_endpoint)) {
            Ok(()) => {}
            Err(tcp::ListenError::InvalidState) => return Err(ConnectError::InvalidState),
            Err(tcp::ListenError::Unaddressable) => return Err(ConnectError::NoRoute),
        }
        Ok(())
    }

    pub async fn accept(&self) -> Result<Self, AcceptError> {
        loop {
            if !self.with(|socket| socket.is_listening()) {
                break Err(AcceptError::InvalidState);
            }
            let res = poll_fn(|cx| self.poll_for_establishment(cx)).await;
            if let Ok(handle) = res {
                let buf = vec![0u8; BUFFER_CAP];

                let mut socket = tcp::Socket::new(buf.clone().into(), ManagedSlice::Owned(buf));
                let endpoint = self.with(|s| {
                    socket.set_timeout(s.timeout());
                    socket.set_keep_alive(s.keep_alive());
                    socket.set_ack_delay(s.ack_delay());
                    socket.set_hop_limit(s.hop_limit());
                    socket.set_nagle_enabled(s.nagle_enabled());
                    s.local_endpoint().unwrap()
                });
                let _ = socket.listen(endpoint);

                let conn = ksync::critical(|| {
                    let mut slot = self.handle.write();
                    if *slot != handle {
                        return None;
                    }
                    let next = self.stack.with_socket_mut(|s| s.sockets.add(socket));
                    Some(mem::replace(&mut *slot, next))
                });
                if let Some(conn) = conn {
                    break Ok(Socket {
                        stack: self.stack.clone(),
                        handle: RwLock::new(conn),
                    });
                }
            }
        }
    }

    pub fn poll_receive(&self, buf: &mut [u8], cx: &mut Context) -> Poll<Result<usize, Error>> {
        self.with_mut(|_, s, _| match s.recv_slice(buf) {
            Ok(0) => {
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => Poll::Ready(Ok(n)),
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(0)),
            Err(tcp::RecvError::InvalidState) => Poll::Ready(Err(Error::ConnectionReset)),
        })
    }

    pub async fn receive(&self, buf: &mut [u8]) -> Result<usize, Error> {
        poll_fn(|cx| self.poll_receive(buf, cx)).await
    }

    pub fn poll_send(&self, buf: &[u8], cx: &mut Context) -> Poll<Result<usize, Error>> {
        self.with_mut(|_, s, _| match s.send_slice(buf) {
            Ok(0) => {
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => Poll::Ready(Ok(n)),
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(Error::ConnectionReset)),
        })
    }

    pub async fn send(&self, buf: &mut [u8]) -> Result<usize, Error> {
        poll_fn(|cx| self.poll_send(buf, cx)).await
    }

    pub fn poll_flush(&self, cx: &mut Context) -> Poll<Result<(), Error>> {
        self.with_mut(|_, s, _| {
            let waiting_close = s.state() == tcp::State::Closed && s.remote_endpoint().is_some();
            // If there are outstanding send operations, register for wake up and wait
            // smoltcp issues wake-ups when octets are dequeued from the send buffer
            if s.send_queue() > 0 || waiting_close {
                s.register_send_waker(cx.waker());
                Poll::Pending
            // No outstanding sends, socket is flushed
            } else {
                Poll::Ready(Ok(()))
            }
        })
    }

    pub async fn flush(&self) -> Result<(), Error> {
        poll_fn(|cx| self.poll_flush(cx)).await
    }

    pub fn set_timeout(&self, timeout: Option<Duration>) {
        self.with_mut(|_, socket, _| socket.set_timeout(timeout.map(duration_to_smoltcp)))
    }

    pub fn set_keep_alive(&self, keep_alive: Option<Duration>) {
        self.with_mut(|_, socket, _| socket.set_keep_alive(keep_alive.map(duration_to_smoltcp)))
    }

    pub fn set_ack_delay(&self, ack_delay: Option<Duration>) {
        self.with_mut(|_, socket, _| socket.set_ack_delay(ack_delay.map(duration_to_smoltcp)))
    }

    pub fn set_hop_limit(&self, hop_limit: Option<u8>) {
        self.with_mut(|_, socket, _| socket.set_hop_limit(hop_limit))
    }

    pub fn set_nagle_enabled(&self, nagle_enabled: bool) {
        self.with_mut(|_, socket, _| socket.set_nagle_enabled(nagle_enabled))
    }

    pub fn local_endpoint(&self) -> Option<IpEndpoint> {
        self.with(|socket| socket.local_endpoint())
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.with(|socket| socket.remote_endpoint())
    }

    pub fn is_open(&self) -> bool {
        self.with(|socket| socket.is_open())
    }

    pub fn is_listening(&self) -> bool {
        self.with(|socket| socket.is_listening())
    }

    pub fn close(&self) {
        self.with_mut(|_, socket, _| socket.close());
    }

    pub fn abort(&self) {
        self.with_mut(|_, socket, _| socket.abort());
    }

    pub fn can_send(&self) -> bool {
        self.with(|socket| socket.can_send())
    }

    pub fn can_recv(&self) -> bool {
        self.with(|socket| socket.can_recv())
    }
}
