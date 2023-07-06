pub mod dns;
pub mod tcp;
pub mod udp;

use ksc::Error::{self, EINVAL};
use smoltcp::wire::IpEndpoint;

const BUFFER_CAP: usize = 16 * 1024;
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
            Socket::Udp(socket) => socket.send(buf, remote_endpoint.ok_or(EINVAL)?).await,
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
                let (len, remote) = socket.receive(buf).await;
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

    pub fn endpoint(&self) -> Option<IpEndpoint> {
        match self {
            Socket::Tcp(socket) => socket.local_endpoint(),
            Socket::Udp(socket) => {
                let endpoint = socket.endpoint();
                endpoint.addr.map(|addr| IpEndpoint {
                    addr,
                    port: endpoint.port,
                })
            }
        }
    }

    pub async fn flush(&self) {
        if let Socket::Tcp(socket) = self {
            socket.flush().await
        }
    }
}
