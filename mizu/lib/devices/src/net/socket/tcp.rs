use alloc::vec;
use core::{
    future::poll_fn,
    mem,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
    task::{Context, Poll},
    time::Duration,
};

use arsc_rs::Arsc;
use crossbeam_queue::SegQueue;
use futures_util::{
    future::{select, Either},
    task::AtomicWaker,
    Future,
};
use ksc::Error::{self, ECONNREFUSED, EEXIST, EINVAL, ENOTCONN};
use ksync::{
    channel::{
        mpmc::{Receiver, Sender},
        unbounded,
    },
    event::Event,
};
use ktime::Timer;
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
    inner: Arsc<Inner>,
    accept: Mutex<Option<Receiver<SegQueue<Socket>>>>,
}

#[derive(Debug)]
struct Inner {
    stack: Arsc<Stack>,
    handle: RwLock<SocketHandle>,
    iface_id: AtomicUsize,

    listen: Mutex<Option<IpListenEndpoint>>,
    backlog: AtomicUsize,
    backlog_event: Event,

    accept_waker: AtomicWaker,
}

impl Inner {
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

    fn poll_for_close(&self, cx: &mut Context) -> Poll<()> {
        self.with_mut(|_, s| match s.state() {
            State::Closed | State::TimeWait if s.send_queue() == 0 => Poll::Ready(()),
            _ => {
                s.register_send_waker(cx.waker());
                s.register_recv_waker(cx.waker());
                Poll::Pending
            }
        })
    }

    async fn close(&self) {
        const FIN_TIMEOUT: Duration = Duration::from_millis(300);

        self.with_mut(|_, socket| socket.close());
        let close = poll_fn(|cx| self.poll_for_close(cx));
        if let Either::Right((_, fut)) = select(close, Timer::after(FIN_TIMEOUT)).await {
            ksync::poll_once(fut);
        }
    }

    async fn close_event(&self, tx: &Sender<SegQueue<Socket>>) {
        let mut listener = None;
        while !tx.is_closed() {
            match listener.take() {
                Some(listener) => listener.await,
                None => listener = Some(self.backlog_event.listen()),
            }
        }
    }

    async fn accept_task(
        self: Arsc<Self>,
        endpoint: IpListenEndpoint,
        tx: Sender<SegQueue<Socket>>,
    ) {
        log::trace!("Creating accept queue");
        while !tx.is_closed() {
            let mut listener = None;
            while tx.len() >= self.backlog.load(SeqCst) {
                match listener.take() {
                    Some(listener) => listener.await,
                    None => listener = Some(self.backlog_event.listen()),
                }
            }

            let establishment = poll_fn(|cx| self.poll_for_establishment(cx));
            let closed = pin!(self.close_event(&tx));

            let Either::Left((res, _)) = select(establishment, closed).await else { break };

            log::trace!("Accepted new connection");

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
                    let data = Socket {
                        inner: Arsc::new(Inner {
                            stack: self.stack.clone(),
                            handle: RwLock::new(conn),
                            iface_id: self.iface_id.load(SeqCst).into(),

                            listen: Default::default(),
                            backlog: Default::default(),
                            backlog_event: Default::default(),

                            accept_waker: Default::default(),
                        }),
                        accept: Default::default(),
                    };
                    let send = &tx.send(data).await;
                    self.accept_waker.wake();
                    if send.is_err() {
                        break;
                    }
                }
            }
        }
        log::trace!("Accept queue destroyed");
        self.close().await
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stack
            .with_socket_mut(|s| s.sockets.remove(*self.handle.read()));
    }
}

impl Socket {
    pub fn new(stack: Arsc<Stack>) -> Self {
        let buf = vec![0u8; BUFFER_CAP];
        let socket = tcp::Socket::new(buf.clone().into(), ManagedSlice::Owned(buf));
        let handle = stack.with_socket_mut(|s| s.sockets.add(socket));
        Socket {
            inner: Arsc::new(Inner {
                stack,
                handle: RwLock::new(handle),
                iface_id: Default::default(),

                listen: Default::default(),
                backlog: Default::default(),
                backlog_event: Default::default(),

                accept_waker: Default::default(),
            }),
            accept: Default::default(),
        }
    }

    pub async fn connect(&self, remote: impl Into<IpEndpoint>) -> Result<(), Error> {
        let remote: IpEndpoint = remote.into();

        let res = self.inner.stack.with_socket_mut(|s| {
            let handle = self.inner.handle.read();
            if s.sockets.get_mut::<tcp::Socket>(*handle).is_open() {
                return Err(tcp::ConnectError::InvalidState);
            }

            let mut local = IpListenEndpoint {
                addr: None,
                port: s.next_local_port(),
            };

            let iface_id = s.select_tcp_addr(&remote, &mut local);
            self.inner.iface_id.store(iface_id, SeqCst);

            let socket = s.sockets.get_mut::<tcp::Socket>(*handle);
            let iface = s.ifaces.get_mut(iface_id).unwrap();
            socket.connect(iface.context(), remote, local)
        });

        match res {
            Ok(()) => {}
            Err(tcp::ConnectError::InvalidState) => return Err(EEXIST),
            Err(tcp::ConnectError::Unaddressable) => return Err(EINVAL),
        }

        poll_fn(|cx| self.inner.poll_for_establishment(cx)).await?;
        Ok(())
    }

    pub fn bind(&self, local_endpoint: impl Into<IpListenEndpoint>) -> Result<(), Error> {
        let mut endpoint: IpListenEndpoint = local_endpoint.into();
        if endpoint.port == 0 {
            endpoint.port = self.inner.stack.with_socket_mut(|s| s.next_local_port());
        }
        ksync::critical(|| match &mut *self.inner.listen.lock() {
            Some(_) => Err(EINVAL),
            slot @ None => {
                *slot = Some(endpoint);
                Ok(())
            }
        })
    }

    pub fn listen(&self, backlog: usize) -> Result<impl Future<Output = ()> + 'static, Error> {
        let endpoint = ksync::critical(|| {
            let mut slot = self.inner.listen.lock();
            *slot.get_or_insert_with(|| IpListenEndpoint {
                addr: None,
                port: self.inner.stack.with_socket_mut(|s| s.next_local_port()),
            })
        });

        self.inner
            .with_mut(|_, socket| socket.listen(endpoint))
            .map_err(|_| EINVAL)?;

        self.inner.backlog.store(backlog, SeqCst);
        let inner = self.inner.clone();
        let (tx, rx) = unbounded();
        ksync::critical(|| *self.accept.lock() = Some(rx));

        Ok(inner.accept_task(endpoint, tx))
    }

    pub async fn accept(&self) -> Result<Self, Error> {
        let Some(rx) = ksync::critical(|| self.accept.lock().clone()) else {
            return Err(EINVAL);
        };
        let socket = rx.recv().await.unwrap();
        self.inner.backlog_event.notify(1);
        Ok(socket)
    }

    pub fn poll_receive(&self, buf: &mut [u8], cx: &mut Context) -> Poll<Result<usize, Error>> {
        self.inner.with_mut(|_, s| match s.recv_slice(buf) {
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
        self.inner.with_mut(|_, s| match s.send_slice(buf) {
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
        self.inner.with_mut(|_, s| {
            if let Some(accept) = &*self.accept.lock() {
                log::trace!("Accept queue: {}", !accept.is_empty());
                if !accept.is_empty() {
                    return Poll::Ready(());
                }
                self.inner.accept_waker.register(cx.waker());
            } else {
                log::trace!("Wake up for recv, polling: {} {}", s.state(), s.can_recv());
                if s.can_recv()
                    || matches!(
                        s.state(),
                        State::FinWait1 | State::FinWait2 | State::CloseWait
                    )
                {
                    return Poll::Ready(());
                }
                s.register_recv_waker(cx.waker());
            }
            Poll::Pending
        })
    }

    pub async fn wait_for_recv(&self) {
        poll_fn(|cx| self.poll_wait_for_recv(cx)).await
    }

    fn poll_wait_for_send(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.inner.with_mut(|_, s| {
            if let Some(accept) = &*self.accept.lock() {
                log::trace!("Accept queue: {}", !accept.is_empty());
                if !accept.is_empty() {
                    return Poll::Ready(());
                }
                self.inner.accept_waker.register(cx.waker());
            } else {
                log::trace!("Wake up for send, polling: {} {}", s.state(), s.can_send());
                if s.can_send() {
                    return Poll::Ready(());
                }
                s.register_send_waker(cx.waker());
            }
            Poll::Pending
        })
    }

    pub async fn wait_for_send(&self) {
        poll_fn(|cx| self.poll_wait_for_send(cx)).await
    }

    pub fn poll_flush(&self, cx: &mut Context) -> Poll<()> {
        self.inner.with_mut(|_, s| {
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
        ksync::critical(|| *self.inner.listen.lock())
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.inner.with(|socket| socket.remote_endpoint())
    }

    pub fn is_open(&self) -> bool {
        self.inner.with(|socket| socket.is_open())
    }

    pub fn is_listening(&self) -> bool {
        self.inner.with(|socket| socket.is_listening())
    }

    pub async fn close(&self) {
        let ret = ksync::critical(|| self.accept.lock().take());
        if ret.is_some() {
            log::trace!("Closing a listening socket");
            self.inner.backlog_event.notify(1);
        } else {
            log::trace!("Closing a connection socket");
            self.inner.close().await
        }
    }

    pub fn abort(&self) {
        self.inner.with_mut(|_, socket| socket.abort());
    }

    pub fn can_send(&self) -> bool {
        self.inner.with(|socket| socket.can_send())
    }

    pub fn can_recv(&self) -> bool {
        self.inner.with(|socket| socket.can_recv())
    }

    pub fn max_segment_size(&self) -> usize {
        let ip_mtu = self
            .inner
            .stack
            .max_transmission_unit(self.inner.iface_id.load(SeqCst))
            - ETHERNET_HEADER_LEN;
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
        if self.accept.get_mut().is_some() {
            log::trace!("Dropping a listening socket");
            self.inner.backlog_event.notify(1);
        }
    }
}
