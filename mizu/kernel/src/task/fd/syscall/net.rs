use alloc::{boxed::Box, sync::Arc};
use core::{mem, pin::Pin};

use co_trap::UserCx;
use devices::net::Socket;
use kmem::Virt;
use ksc::{
    async_handler,
    Error::{self, EAFNOSUPPORT, EINVAL, ENOTSOCK},
};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address, Ipv6Address};
use umifs::types::{OpenOptions, Permissions};
use umio::IntoAnyExt;
use zerocopy::{AsBytes, FromBytes};

use crate::{
    fs::socket::{self, SocketFile},
    mem::{In, InOut, Out, UserBuffer, UserPtr},
    syscall::ScRet,
    task::{fd::Files, TaskState},
};

const AF_INET: u16 = 2; // Internet IP Protocol
const AF_INET6: u16 = 10; // IP version 6

#[derive(Debug, Clone, Copy, FromBytes, AsBytes, Default)]
#[repr(C, packed)]
struct SockAddrIpv4 {
    // Big endian.
    port: [u8; 2],
    // Big endian.
    addr: [u8; 4],
}

impl From<SockAddrIpv4> for IpEndpoint {
    fn from(mut value: SockAddrIpv4) -> Self {
        let port = u16::from_be_bytes(value.port);
        value.addr.reverse();
        IpEndpoint {
            port,
            addr: IpAddress::Ipv4(Ipv4Address(value.addr)),
        }
    }
}

impl From<SockAddrIpv4> for IpListenEndpoint {
    fn from(mut value: SockAddrIpv4) -> Self {
        let port = u16::from_be_bytes(value.port);
        value.addr.reverse();
        IpListenEndpoint {
            port,
            addr: Some(IpAddress::Ipv4(Ipv4Address(value.addr))),
        }
    }
}

#[derive(Debug, Clone, Copy, FromBytes, AsBytes, Default)]
#[repr(C, packed)]
struct SockAddrIpv6 {
    // Big endian.
    port: [u8; 2],
    flow_info: [u8; 4],
    // Big endian.
    addr: [u8; 16],
}

impl From<SockAddrIpv6> for IpEndpoint {
    fn from(mut value: SockAddrIpv6) -> Self {
        let port = u16::from_be_bytes(value.port);
        value.addr.reverse();
        IpEndpoint {
            port,
            addr: IpAddress::Ipv6(Ipv6Address(value.addr)),
        }
    }
}

impl From<SockAddrIpv6> for IpListenEndpoint {
    fn from(mut value: SockAddrIpv6) -> Self {
        let port = u16::from_be_bytes(value.port);
        value.addr.reverse();
        IpListenEndpoint {
            port,
            addr: Some(IpAddress::Ipv6(Ipv6Address(value.addr))),
        }
    }
}

async fn endpoint<T>(
    virt: Pin<&Virt>,
    mut addr: UserPtr<u16, In>,
    len: usize,
) -> Result<Option<T>, Error>
where
    T: From<SockAddrIpv4> + From<SockAddrIpv6>,
{
    if addr.is_null() {
        return Ok(None);
    }
    let family = addr.read(virt).await?;
    match family {
        AF_INET => {
            if len < mem::size_of::<SockAddrIpv4>() + mem::size_of::<u16>() {
                return Err(EINVAL);
            }
            addr.advance(mem::size_of::<u16>());
            let addr = addr.cast::<SockAddrIpv4>().read(virt).await?;
            Ok(Some(addr.into()))
        }
        AF_INET6 => {
            if len < mem::size_of::<SockAddrIpv6>() + mem::size_of::<u16>() {
                return Err(EINVAL);
            }
            addr.advance(mem::size_of::<u16>());
            let addr = addr.cast::<SockAddrIpv6>().read(virt).await?;
            Ok(Some(addr.into()))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

async fn write_endpoint(
    virt: Pin<&Virt>,
    endpoint: IpEndpoint,
    mut ptr: UserPtr<u16, Out>,
    len: UserPtr<usize, InOut>,
) -> Result<(), Error> {
    let buf_len = len.read(virt).await?;
    if buf_len < mem::size_of::<u16>() {
        return Err(EINVAL);
    }
    match endpoint.addr {
        IpAddress::Ipv4(addr) => {
            ptr.write(virt, AF_INET).await?;
            ptr.advance(mem::size_of::<u16>());

            let mut sav4 = SockAddrIpv4 {
                port: endpoint.port.to_be_bytes(),
                addr: addr.0,
            };
            sav4.addr.reverse();
            let len = sav4.as_bytes().len().min(buf_len - mem::size_of::<u16>());
            ptr.cast::<u8>()
                .write_slice(virt, &sav4.as_bytes()[..len], false)
                .await?;
            Ok(())
        }
        IpAddress::Ipv6(addr) => {
            ptr.write(virt, AF_INET).await?;
            ptr.advance(mem::size_of::<u16>());

            let mut sav6 = SockAddrIpv6 {
                port: endpoint.port.to_be_bytes(),
                addr: addr.0,
                ..Default::default()
            };
            sav6.addr.reverse();
            let len = sav6.as_bytes().len().min(buf_len - mem::size_of::<u16>());
            ptr.cast::<u8>()
                .write_slice(virt, &sav6.as_bytes()[..len], false)
                .await?;
            Ok(())
        }
    }
}

async fn sock<'a>(
    files: &Files,
    fd: i32,
    storage: &'a mut Option<Arc<SocketFile>>,
) -> Result<&'a Socket, Error> {
    Ok(&*storage.insert(files.get(fd).await?.downcast().ok_or(ENOTSOCK)?))
}

#[async_handler]
pub async fn socket(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(u16, i32, i32) -> Result<i32, Error>>,
) -> ScRet {
    const SOCK_DGRAM: i32 = 1;
    const SOCK_STREAM: i32 = 2;
    const SOCK_CLOEXEC: i32 = OpenOptions::CLOEXEC.bits();
    // const SOCK_NONBLOCK: i32 = OpenOptions::NONBLOCK.bits();

    let (domain, ty, protocol) = cx.args();
    let fut = async {
        let socket = match domain {
            AF_INET | AF_INET6 => match ty & 0xf {
                SOCK_DGRAM if protocol == 0 => socket::udp()?,
                SOCK_STREAM if protocol == 0 => socket::tcp()?,
                _ => return Err(EINVAL),
            },
            _ => return Err(EINVAL),
        };
        let close_on_exec = ty & SOCK_CLOEXEC == 0;
        let perm = Permissions::all_same(true, true, false);
        ts.files.open(socket, perm, close_on_exec).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn sendto(
    ts: &mut TaskState,
    cx: UserCx<
        '_,
        fn(i32, UserBuffer, usize, i32, UserPtr<u16, In>, usize) -> Result<usize, Error>,
    >,
) -> ScRet {
    let (fd, buf, len, _flags, addr, addr_len) = cx.args();
    let fut = async {
        let buf = buf.as_slice(ts.virt.as_ref(), len).await?.concat();
        let endpoint = endpoint(ts.virt.as_ref(), addr, addr_len).await?;
        let mut storage = None;
        let socket = sock(&ts.files, fd, &mut storage).await?;
        socket.send(&buf, endpoint).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn connect(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u16, In>, usize) -> Result<(), Error>>,
) -> ScRet {
    let (fd, addr, len) = cx.args();
    let fut = async {
        let endpoint = endpoint(ts.virt.as_ref(), addr, len).await?.ok_or(EINVAL)?;
        let mut storage = None;
        let socket = sock(&ts.files, fd, &mut storage).await?;
        socket.connect(endpoint).await?;
        Ok(())
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn bind(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u16, In>, usize) -> Result<(), Error>>,
) -> ScRet {
    let (fd, addr, len) = cx.args();
    let fut = async {
        let endpoint = endpoint(ts.virt.as_ref(), addr, len).await?.ok_or(EINVAL)?;
        let mut storage = None;
        let socket = sock(&ts.files, fd, &mut storage).await?;
        socket.bind(endpoint)
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn listen(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, i32) -> Result<(), Error>>,
) -> ScRet {
    let (fd, _backlog) = cx.args();
    let fut = async {
        let mut storage = None;
        let socket = sock(&ts.files, fd, &mut storage).await?;
        socket.listen()
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}

#[async_handler]
pub async fn accept(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(i32, UserPtr<u16, Out>, UserPtr<usize, InOut>) -> Result<i32, Error>>,
) -> ScRet {
    let (fd, addr, len) = cx.args();
    let fut = async {
        let mut storage = None;
        let socket = sock(&ts.files, fd, &mut storage).await?;
        let new = socket.accept().await?;
        if let Some(remote) = new.remote_endpoint() {
            write_endpoint(ts.virt.as_ref(), remote, addr, len).await?;
        }
        let perm = Permissions::all_same(true, true, false);
        ts.files.open(socket::tcp_accept(new), perm, false).await
    };
    cx.ret(fut.await);
    ScRet::Continue(None)
}
