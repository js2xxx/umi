use alloc::{sync::Arc, vec::Vec};
use core::{
    convert::Infallible,
    fmt::{self, Display},
    future::poll_fn,
    mem,
    pin::Pin,
    task::{ready, Context, Poll},
};

use arsc_rs::Arsc;
use futures_util::{task::AtomicWaker, Future, FutureExt};
use hashbrown::HashMap;
use ktime::{Instant, Timer};
use rand_riscv::RandomState;
use smoltcp::{
    iface::{self, Interface, SocketHandle, SocketSet},
    phy::{Device, Loopback, Tracer},
    socket::{dhcpv4, dns},
    wire::{
        DnsQueryType, EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint,
        IpListenEndpoint, Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr,
    },
};
use spin::RwLock;

use super::{
    config::{Config, ConfigV4, ConfigV6, DhcpV4Config, StaticConfigV4, StaticConfigV6},
    driver::{Net, NetExt},
    socket::dns::Error as DnsError,
    time::{duration_to_smoltcp, instant_from_smoltcp, instant_to_smoltcp},
};

const LOCAL_PORT_MIN: u16 = 1025;
const LOCAL_PORT_MAX: u16 = 65535;

pub const LOOPBACK_IPV4: IpCidr = IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address([127, 0, 0, 1]), 8));
pub const LOOPBACK_IPV6: IpCidr = IpCidr::Ipv6(Ipv6Cidr::new(Ipv6Address::LOOPBACK, 128));

fn writer(id: u8, instant: impl Display, packet: impl Display) {
    log::info!("net stack #{id}: at {instant}:");
    log::info!("\t {packet}");
}

pub struct Stack {
    device: Arc<RwLock<dyn Net>>,
    socket: RwLock<SocketStack>,
    state: RwLock<State>,
}

#[derive(Debug)]
pub(in crate::net) struct State {
    link_up: bool,

    dhcpv4: Option<SocketHandle>,

    dns_socket: SocketHandle,
    dns_servers_ipv4: heapless::Vec<Ipv4Address, 3>,
    dns_servers_ipv6: heapless::Vec<Ipv6Address, 3>,
    dns_waker: AtomicWaker,
}

pub(in crate::net) struct SocketStack {
    pub sockets: SocketSet<'static>,
    next_local_port: u16,

    pub loopback: Tracer<Loopback>,
    pub ifaces: HashMap<u8, Interface, RandomState>,

    waker: AtomicWaker,
}

impl fmt::Debug for SocketStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SocketStack")
            .field("sockets", &self.sockets)
            .field("interface", &..)
            .finish()
    }
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stack")
            .field("device", &..)
            .field("socket", &self.socket)
            .field("state", &self.state)
            .finish()
    }
}

impl Stack {
    pub(in crate::net) fn with_mut<T>(
        &self,
        f: impl FnOnce(&mut dyn Net, &mut State, &mut SocketStack) -> T,
    ) -> T {
        ksync::critical(|| {
            let mut socket = self.socket.write();
            let res = f(
                &mut *self.device.write(),
                &mut self.state.write(),
                &mut socket,
            );
            socket.waker.wake();
            res
        })
    }

    pub(in crate::net) fn with_socket_mut<T>(&self, f: impl FnOnce(&mut SocketStack) -> T) -> T {
        ksync::critical(|| {
            let mut socket = self.socket.write();
            let res = f(&mut socket);
            socket.waker.wake();
            res
        })
    }

    pub(in crate::net) fn with_socket<T>(&self, f: impl FnOnce(&SocketStack) -> T) -> T {
        ksync::critical(|| f(&self.socket.read()))
    }
}

impl Stack {
    fn new_iface_config(hardware_addr: HardwareAddress) -> iface::Config {
        let mut config = iface::Config::new(hardware_addr);
        config.random_seed = rand_riscv::seed64();
        config
    }

    fn update_interface(iface: &mut Interface, device: &mut dyn Net) {
        iface.set_hardware_addr(HardwareAddress::Ethernet(EthernetAddress(device.address())))
    }

    pub fn new(device: Arc<RwLock<dyn Net>>, config: Config) -> Arsc<Self> {
        let now = instant_to_smoltcp(ktime::Instant::now());

        let next_local_port = (rand_riscv::seed64() % (LOCAL_PORT_MAX - LOCAL_PORT_MIN) as u64)
            as u16
            + LOCAL_PORT_MIN;

        let mut socket = SocketStack {
            sockets: SocketSet::new(Vec::new()),
            loopback: Tracer::new(Loopback::new(smoltcp::phy::Medium::Ip), |i, p| {
                writer(0, i, p)
            }),
            ifaces: Default::default(),
            waker: AtomicWaker::new(),
            next_local_port,
        };

        let mut loopback_interface = Interface::new(
            Self::new_iface_config(HardwareAddress::Ip),
            &mut socket.loopback,
            now,
        );
        loopback_interface
            .update_ip_addrs(|addrs| *addrs = [LOOPBACK_IPV4, LOOPBACK_IPV6].into_iter().collect());
        socket.ifaces.insert_unique_unchecked(0, loopback_interface);

        ksync::critical(|| {
            let config = Self::new_iface_config(HardwareAddress::Ethernet(EthernetAddress(
                device.read().address(),
            )));
            let iface = Interface::new(config, &mut device.write().with_cx(None), now);
            socket.ifaces.insert_unique_unchecked(1, iface)
        });

        let mut state = State {
            link_up: false,

            dhcpv4: None,

            dns_socket: socket.sockets.add(dns::Socket::new(&[], Vec::new())),
            dns_servers_ipv4: Default::default(),
            dns_servers_ipv6: Default::default(),
            dns_waker: AtomicWaker::new(),
        };

        match config.ipv4 {
            ConfigV4::Static(config) => state.apply_ipv4_config(&mut socket, config),
            ConfigV4::Dhcp(config) => {
                let mut dhcpv4 = dhcpv4::Socket::new();
                state.apply_dhcpv4_config(&mut dhcpv4, config);
                state.dhcpv4 = Some(socket.sockets.add(dhcpv4));
            }
            ConfigV4::None => {}
        }

        if let ConfigV6::Static(config) = config.ipv6 {
            state.apply_ipv6_config(&mut socket, config)
        }

        Arsc::new(Stack {
            device,
            socket: RwLock::new(socket),
            state: RwLock::new(state),
        })
    }

    pub fn run(self: Arsc<Self>) -> StackBackground {
        StackBackground {
            stack: self,
            timer: None,
        }
    }
}

macro_rules! extern_ifaces {
    ($sstack:expr) => {{
        let iter = $sstack.ifaces.iter_mut();
        iter.filter_map(|(&id, iface)| (id != 0).then_some((id, iface)))
    }};
}

impl Stack {
    pub fn max_transmission_unit(&self, iface_id: u8) -> usize {
        if iface_id == 0 {
            self.with_socket(|s| s.loopback.capabilities().max_transmission_unit)
        } else {
            ksync::critical(|| self.device.read().features().max_unit)
        }
    }

    pub async fn dns_query(
        &self,
        name: &str,
        ty: DnsQueryType,
    ) -> Result<heapless::Vec<IpAddress, 1>, DnsError> {
        struct CallOnDrop<F: FnOnce()>(Option<F>);
        impl<F: FnOnce()> Drop for CallOnDrop<F> {
            fn drop(&mut self) {
                if let Some(f) = self.0.take() {
                    f()
                }
            }
        }

        match ty {
            DnsQueryType::A if let Ok(ip) = name.parse().map(IpAddress::Ipv4) => {
                return Ok([ip].into_iter().collect())
            }
            DnsQueryType::Aaaa if let Ok(ip) = name.parse().map(IpAddress::Ipv6) => {
                return Ok([ip].into_iter().collect())
            },
            _ => {}
        }

        let handle = poll_fn(|cx| self.dns_start_query(name, ty, cx)).await?;

        let mut cancel = CallOnDrop(Some(|| self.dns_cancel(handle)));
        let result = poll_fn(|cx| self.dns_get_result(handle, cx)).await?;
        cancel.0 = None;

        Ok(result)
    }

    fn dns_start_query(
        &self,
        name: &str,
        ty: DnsQueryType,
        cx: &mut Context<'_>,
    ) -> Poll<Result<dns::QueryHandle, dns::StartQueryError>> {
        self.with_mut(|_, state, s| {
            let socket = s.sockets.get_mut::<dns::Socket>(state.dns_socket);
            for (_, iface) in extern_ifaces!(s) {
                match socket.start_query(iface.context(), name, ty) {
                    Err(dns::StartQueryError::NoFreeSlot) => {}
                    res => return Poll::Ready(res),
                }
            }
            state.dns_waker.register(cx.waker());
            Poll::Pending
        })
    }

    fn dns_get_result(
        &self,
        handle: dns::QueryHandle,
        cx: &mut Context<'_>,
    ) -> Poll<Result<heapless::Vec<IpAddress, 1>, dns::GetQueryResultError>> {
        self.with_mut(|_, state, s| {
            let socket = s.sockets.get_mut::<dns::Socket>(state.dns_socket);
            match socket.get_query_result(handle) {
                Err(dns::GetQueryResultError::Pending) => {
                    socket.register_query_waker(handle, cx.waker());
                    Poll::Pending
                }
                res => {
                    state.dns_waker.wake();
                    Poll::Ready(res)
                }
            }
        })
    }

    fn dns_cancel(&self, handle: dns::QueryHandle) {
        self.with_mut(|_, state, s| {
            let socket = s.sockets.get_mut::<dns::Socket>(state.dns_socket);
            socket.cancel_query(handle);
            s.waker.wake();
            state.dns_waker.wake();
        })
    }
}

impl State {
    fn apply_ipv4_config(&mut self, s: &mut SocketStack, config: StaticConfigV4) {
        let iface = s.ifaces.get_mut(&1).unwrap();

        iface.update_ip_addrs(|addrs| match addrs.first_mut() {
            Some(addr) => *addr = IpCidr::Ipv4(config.address),
            None => addrs.push(IpCidr::Ipv4(config.address)).unwrap(),
        });

        if let Some(gateway) = config.gateway {
            iface.routes_mut().add_default_ipv4_route(gateway).unwrap();
        } else {
            iface.routes_mut().remove_default_ipv4_route();
        }

        self.dns_servers_ipv4 = config.dns_servers;
        self.update_dns_servers(s);
    }

    fn apply_ipv6_config(&mut self, s: &mut SocketStack, config: StaticConfigV6) {
        let iface = s.ifaces.get_mut(&1).unwrap();

        iface.update_ip_addrs(|addrs| match addrs.first_mut() {
            Some(addr) => *addr = IpCidr::Ipv6(config.address),
            None => addrs.push(IpCidr::Ipv6(config.address)).unwrap(),
        });

        if let Some(gateway) = config.gateway {
            iface.routes_mut().add_default_ipv6_route(gateway).unwrap();
        } else {
            iface.routes_mut().remove_default_ipv6_route();
        }

        self.dns_servers_ipv6 = config.dns_servers;
        self.update_dns_servers(s);
    }

    fn update_dns_servers(&mut self, s: &mut SocketStack) {
        let socket = s.sockets.get_mut::<dns::Socket>(self.dns_socket);
        let servers_ipv4 = self.dns_servers_ipv4.iter().copied().map(IpAddress::Ipv4);
        let servers_ipv6 = self.dns_servers_ipv6.iter().copied().map(IpAddress::Ipv6);
        let servers: heapless::Vec<_, 6> = servers_ipv4.chain(servers_ipv6).collect();
        socket.update_servers(&servers);
    }

    fn apply_dhcpv4_config(&mut self, socket: &mut dhcpv4::Socket, config: DhcpV4Config) {
        socket.set_ignore_naks(config.ignore_naks);
        socket.set_max_lease_duration(config.max_lease_duration.map(duration_to_smoltcp));
        socket.set_ports(config.server_port, config.client_port);
        socket.set_retry_config(config.retry_config);
    }

    fn unapply_dhcpv4_config(&mut self, s: &mut SocketStack) {
        let iface = s.ifaces.get_mut(&1).unwrap();
        iface.update_ip_addrs(|addrs| addrs.clear());
        iface.routes_mut().remove_default_ipv4_route();
        self.dns_servers_ipv4.clear();
    }

    fn poll(
        &mut self,
        cx: &mut Context,
        device: &mut dyn Net,
        s: &mut SocketStack,
    ) -> Option<Instant> {
        s.waker.register(cx.waker());
        Stack::update_interface(s.ifaces.get_mut(&1).unwrap(), device);

        let instant = instant_to_smoltcp(ktime::Instant::now());

        s.ifaces
            .get_mut(&0)
            .unwrap()
            .poll(instant, &mut s.loopback, &mut s.sockets);

        let mut poller = Tracer::new(device.with_cx(Some(cx)), |i, p| writer(1, i, p));
        for (_id, iface) in extern_ifaces!(s) {
            iface.poll(instant, &mut poller, &mut s.sockets);
        }

        let old = mem::replace(&mut self.link_up, device.is_link_up());
        if old != self.link_up {
            let s = if self.link_up { "up" } else { "down" };
            log::info!("Net device link {s}");
        }

        if let Some(dhcpv4) = self.dhcpv4 {
            let socket = s.sockets.get_mut::<dhcpv4::Socket>(dhcpv4);
            if self.link_up {
                match socket.poll() {
                    Some(dhcpv4::Event::Configured(config)) => {
                        let config = StaticConfigV4 {
                            address: config.address,
                            gateway: config.router,
                            dns_servers: config.dns_servers,
                        };
                        self.apply_ipv4_config(s, config)
                    }
                    Some(dhcpv4::Event::Deconfigured) => self.unapply_dhcpv4_config(s),
                    None => {}
                }
            } else if old {
                socket.reset();
                self.unapply_dhcpv4_config(s)
            }
        }

        let ddl = s.ifaces.values_mut().fold(None, |acc, iface| {
            let next = iface.poll_at(instant, &s.sockets);
            match (acc, next) {
                (None, next) => next,
                (acc, None) => acc,
                (Some(acc), Some(next)) => Some(next.min(acc)),
            }
        });
        ddl.map(instant_from_smoltcp)
    }
}

impl SocketStack {
    pub fn next_local_port(&mut self) -> u16 {
        let res = self.next_local_port;
        self.next_local_port = if res == LOCAL_PORT_MAX {
            LOCAL_PORT_MIN
        } else {
            res + 1
        };
        res
    }

    pub fn select_tcp_addr(&mut self, remote: &IpEndpoint, local: &mut IpListenEndpoint) -> u8 {
        let loopback_ipv4 = LOOPBACK_IPV4.contains_addr(&remote.addr);
        let loopback_ipv6 = LOOPBACK_IPV6.contains_addr(&remote.addr);
        match (loopback_ipv4, loopback_ipv6) {
            (true, _) => local.addr = Some(LOOPBACK_IPV4.address()),
            (_, true) => local.addr = Some(LOOPBACK_IPV6.address()),
            _ => {}
        }
        if loopback_ipv4 || loopback_ipv6 {
            0
        } else {
            self.ifaces.keys().find(|&&k| k != 0).copied().unwrap()
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct StackBackground {
    stack: Arsc<Stack>,
    timer: Option<Timer>,
}

impl Future for StackBackground {
    type Output = Infallible;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            if let Some(timer) = self.timer.as_mut() {
                ready!(timer.poll_unpin(cx));
                self.timer = None;
            }
            // log::trace!("Start polling net stack");
            let ddl = self
                .stack
                .with_mut(|device, state, s| state.poll(cx, device, s));
            // log::trace!("End polling net stack with deadline {ddl:?}");
            match ddl {
                Some(ddl) => self.timer = Some(Timer::deadline(ddl)),
                None => break Poll::Pending,
            }
        }
    }
}
