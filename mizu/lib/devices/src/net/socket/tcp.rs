use alloc::vec;
use core::{
    future::poll_fn,
    mem,
    sync::atomic::{AtomicU8, Ordering::SeqCst},
    task::{Context, Poll},
};

use arsc_rs::Arsc;
use ksc::Error::{self, ECONNREFUSED, EEXIST, EINVAL, ENOTCONN};
use managed::ManagedSlice;
use smoltcp::{
    iface::SocketHandle,
    socket::tcp::{self, State},
    wire::{
        IpAddress, IpEndpoint, IpListenEndpoint, ETHERNET_HEADER_LEN, IPV4_HEADER_LEN,
        IPV6_HEADER_LEN, TCP_HEADER_LEN,
    },
};
use spin::{Mutex, RwLock};

use super::BUFFER_CAP;
use crate::net::Stack;

#[derive(Debug)]
pub struct Socket {
    stack: Arsc<Stack>,
    handle: RwLock<SocketHandle>,
    listen: Mutex<Option<IpListenEndpoint>>,
    iface_id: AtomicU8,
}

impl Socket {
    fn with_mut<T>(&self, f: impl FnOnce(SocketHandle, &mut tcp::Socket) -> T) -> T {
        self.stack.with_socket_mut(|s| {
            let handle = self.handle.read();
            f(*handle, s.sockets.get_mut(*handle))
        })
    }

    fn with<T>(&self, f: impl FnOnce(&tcp::Socket) -> T) -> T {
        self.stack.with_socket(|s| {
            let handle = self.handle.read();
            f(s.sockets.get(*handle))
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
            listen: Default::default(),
            iface_id: Default::default(),
        }
    }

    fn poll_for_establishment(&self, cx: &mut Context) -> Poll<Result<SocketHandle, Error>> {
        self.with_mut(|handle, s| match s.state() {
            State::Closed | State::TimeWait => Poll::Ready(Err(ENOTCONN)),
            State::Listen | State::SynSent | State::SynReceived => {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
            _ => Poll::Ready(Ok(handle)),
        })
    }

    pub async fn connect(&self, remote: impl Into<IpEndpoint>) -> Result<(), Error> {
        let remote: IpEndpoint = remote.into();

        let res = self.stack.with_socket_mut(|s| {
            let handle = self.handle.read();
            if s.sockets.get_mut::<tcp::Socket>(*handle).is_open() {
                return Err(tcp::ConnectError::InvalidState);
            }

            let mut local = IpListenEndpoint {
                addr: None,
                port: s.next_local_port(),
            };

            let iface_id = s.select_tcp_addr(&remote, &mut local);
            self.iface_id.store(iface_id, SeqCst);

            let socket = s.sockets.get_mut::<tcp::Socket>(*handle);
            let iface = s.ifaces.get_mut(&iface_id).unwrap();
            socket.connect(iface.context(), remote, local)
        });

        match res {
            Ok(()) => {}
            Err(tcp::ConnectError::InvalidState) => return Err(EEXIST),
            Err(tcp::ConnectError::Unaddressable) => return Err(EINVAL),
        }

        poll_fn(|cx| self.poll_for_establishment(cx)).await?;
        Ok(())
    }

    pub fn bind(&self, local_endpoint: impl Into<IpListenEndpoint>) -> Result<(), Error> {
        let mut endpoint: IpListenEndpoint = local_endpoint.into();
        if endpoint.port == 0 {
            endpoint.port = self.stack.with_socket_mut(|s| s.next_local_port());
        }
        ksync::critical(|| match &mut *self.listen.lock() {
            Some(_) => Err(EINVAL),
            slot @ None => {
                *slot = Some(endpoint);
                Ok(())
            }
        })
    }

    pub fn listen(&self) -> Result<(), Error> {
        let Some(endpoint) = ksync::critical(|| *self.listen.lock()) else {
            return Err(EINVAL);
        };
        self.with_mut(|_, socket| socket.listen(endpoint))
            .map_err(|_| EINVAL)
    }

    pub async fn accept(&self) -> Result<Self, Error> {
        loop {
            let Some(endpoint) = ksync::critical(|| *self.listen.lock()) else {
                break Err(EINVAL);
            };
            let res = poll_fn(|cx| self.poll_for_establishment(cx)).await;
            if let Ok(handle) = res {
                let buf = vec![0u8; BUFFER_CAP];

                let mut socket = tcp::Socket::new(buf.clone().into(), ManagedSlice::Owned(buf));
                self.with(|s| {
                    socket.set_timeout(s.timeout());
                    socket.set_keep_alive(s.keep_alive());
                    socket.set_ack_delay(s.ack_delay());
                    socket.set_hop_limit(s.hop_limit());
                    socket.set_nagle_enabled(s.nagle_enabled());
                    s.local_endpoint().unwrap()
                });
                socket.listen(endpoint).unwrap();

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
                        listen: Default::default(),
                        iface_id: self.iface_id.load(SeqCst).into(),
                    });
                }
            }
        }
    }

    pub fn poll_receive(&self, buf: &mut [u8], cx: &mut Context) -> Poll<Result<usize, Error>> {
        self.with_mut(|_, s| match s.recv_slice(buf) {
            Ok(0) => {
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => Poll::Ready(Ok(n)),
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(0)),
            Err(tcp::RecvError::InvalidState) => Poll::Ready(Err(ECONNREFUSED)),
        })
    }

    pub async fn receive(&self, buf: &mut [u8]) -> Result<usize, Error> {
        poll_fn(|cx| self.poll_receive(buf, cx)).await
    }

    pub fn poll_send(&self, buf: &[u8], cx: &mut Context) -> Poll<Result<usize, Error>> {
        self.with_mut(|_, s| match s.send_slice(buf) {
            Ok(0) => {
                s.register_send_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => Poll::Ready(Ok(n)),
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(ECONNREFUSED)),
        })
    }

    pub async fn send(&self, buf: &[u8]) -> Result<usize, Error> {
        poll_fn(|cx| self.poll_send(buf, cx)).await
    }

    fn poll_wait_for_recv(&self, cx: &mut Context) -> Poll<()> {
        self.with_mut(|_, s| {
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
        self.with_mut(|_, s| {
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
        self.with_mut(|_, s| {
            let waiting_close = s.state() == State::Closed && s.remote_endpoint().is_some();
            // If there are outstanding send operations, register for wake up and wait
            // smoltcp issues wake-ups when octets are dequeued from the send buffer
            if s.send_queue() > 0 || waiting_close {
                s.register_send_waker(cx.waker());
                Poll::Pending
            // No outstanding sends, socket is flushed
            } else {
                Poll::Ready(())
            }
        })
    }

    pub async fn flush(&self) {
        poll_fn(|cx| self.poll_flush(cx)).await
    }

    pub fn listen_endpoint(&self) -> Option<IpListenEndpoint> {
        ksync::critical(|| *self.listen.lock())
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
        self.with_mut(|_, socket| socket.close());
    }

    pub fn abort(&self) {
        self.with_mut(|_, socket| socket.abort());
    }

    pub fn can_send(&self) -> bool {
        self.with(|socket| socket.can_send())
    }

    pub fn can_recv(&self) -> bool {
        self.with(|socket| socket.can_recv())
    }

    pub fn max_segment_size(&self) -> usize {
        let ip_mtu =
            self.stack.max_transmission_unit(self.iface_id.load(SeqCst)) - ETHERNET_HEADER_LEN;
        let tcp_mtu = match self.remote_endpoint() {
            Some(IpEndpoint {
                addr: IpAddress::Ipv4(..),
                ..
            }) => ip_mtu - IPV4_HEADER_LEN,
            Some(IpEndpoint {
                addr: IpAddress::Ipv6(..),
                ..
            }) => ip_mtu - IPV6_HEADER_LEN,
            None => ip_mtu - IPV4_HEADER_LEN.max(IPV6_HEADER_LEN),
        };
        (tcp_mtu - TCP_HEADER_LEN).min(BUFFER_CAP)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.stack
            .with_socket_mut(|s| s.sockets.remove(*self.handle.get_mut()));
    }
}
