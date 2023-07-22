pub mod dns;
pub mod tcp;
pub mod udp;

use futures_util::Future;
use ksc::Error::{self, EOPNOTSUPP};
use smoltcp::wire::{IpEndpoint, IpListenEndpoint};

pub const BUFFER_CAP: usize = 212992;
const META_CAP: usize = 8;

#[derive(Debug)]
pub enum Socket {
    Tcp(tcp::Socket),
    Udp(udp::Socket),
}

impl Socket {
    pub async fn send(
        &self,
        buf: &[u8],
        remote_endpoint: Option<IpEndpoint>,
    ) -> Result<usize, Error> {
        match self {
            Socket::Tcp(socket) => socket.send(buf).await,
            Socket::Udp(socket) => socket.send(buf, remote_endpoint).await,
        }
    }

    pub async fn receive(
        &self,
        buf: &mut [u8],
        remote_endpoint: Option<&mut IpEndpoint>,
    ) -> Result<usize, Error> {
        match self {
            Socket::Tcp(socket) => socket.receive(buf).await,
            Socket::Udp(socket) => {
                let (len, remote) = socket.receive(buf).await?;
                if let Some(remote_endpoint) = remote_endpoint {
                    *remote_endpoint = remote;
                }
                Ok(len)
            }
        }
    }

    pub async fn wait_for_send(&self) {
        match self {
            Socket::Tcp(socket) => socket.wait_for_send().await,
            Socket::Udp(socket) => socket.wait_for_send().await,
        }
    }

    pub async fn wait_for_recv(&self) {
        match self {
            Socket::Tcp(socket) => socket.wait_for_recv().await,
            Socket::Udp(socket) => socket.wait_for_recv().await,
        }
    }

    pub async fn connect(&self, endpoint: IpEndpoint) -> Result<(), Error> {
        match self {
            Socket::Tcp(socket) => socket.connect(endpoint).await,
            Socket::Udp(socket) => socket.connect(endpoint),
        }
    }

    pub fn bind(&self, endpoint: IpListenEndpoint) -> Result<(), Error> {
        match self {
            Socket::Tcp(socket) => socket.bind(endpoint),
            Socket::Udp(socket) => socket.bind(endpoint),
        }
    }

    pub fn listen(&self, backlog: usize) -> Result<impl Future<Output = ()> + 'static, Error> {
        match self {
            Socket::Tcp(socket) => socket.listen(backlog),
            Socket::Udp(_) => Err(EOPNOTSUPP),
        }
    }

    pub async fn accept(&self) -> Result<Socket, Error> {
        match self {
            Socket::Tcp(socket) => socket.accept().await.map(Socket::Tcp),
            Socket::Udp(_) => Err(EOPNOTSUPP),
        }
    }

    pub fn listen_endpoint(&self) -> Option<IpListenEndpoint> {
        match self {
            Socket::Tcp(socket) => socket.listen_endpoint(),
            Socket::Udp(socket) => Some(socket.listen_endpoint()),
        }
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        match self {
            Socket::Tcp(socket) => socket.remote_endpoint(),
            Self::Udp(socket) => socket.remote_endpoint(),
        }
    }

    pub async fn flush(&self) {
        if let Socket::Tcp(socket) = self {
            socket.flush().await
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Socket::Tcp(socket) => !socket.is_open(),
            Socket::Udp(_) => false,
        }
    }

    pub async fn close(&self) {
        if let Socket::Tcp(socket) = self {
            socket.close().await
        }
    }
}
