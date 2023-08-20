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
use futures_util::{future::join_all, task::AtomicWaker, Future, FutureExt};
use ktime::{Instant, InstantExt, Timer};
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
    devices: Vec<Arc<RwLock<dyn Net>>>,
    socket: RwLock<SocketStack>,
    states: RwLock<Vec<State>>,
}

#[derive(Debug)]
pub(in crate::net) struct State {
    link_up: bool,
    local_sockets: SocketSet<'static>,

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
    pub ifaces: Vec<Interface>,

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
            .field("state", &self.states)
            .finish()
    }
}

impl Stack {
    pub(in crate::net) fn with_mut<T>(
        &self,
        f: impl FnOnce(&[Arc<RwLock<dyn Net>>], &mut [State], &mut SocketStack) -> T,
    ) -> T {
        ksync::critical(|| {
            let mut socket = self.socket.write();
            let res = f(&self.devices, &mut self.states.write(), &mut socket);
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

    fn update_interface(iface: &mut Interface, device: &dyn Net) {
        iface.set_hardware_addr(HardwareAddress::Ethernet(EthernetAddress(device.address())))
    }

    pub fn new(pairs: impl IntoIterator<Item = (Arc<RwLock<dyn Net>>, Config)>) -> Arsc<Self> {
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
        socket.ifaces.push(loopback_interface);

        let mut devices = Vec::new();
        let mut states = Vec::new();

        for (device, config) in pairs {
            let c = Self::new_iface_config(HardwareAddress::Ethernet(EthernetAddress(
                device.read().address(),
            )));
            let mut iface = Interface::new(c, &mut device.write().with_cx(None), now);

            let mut local_sockets = SocketSet::new(Vec::new());
            let dns_socket = local_sockets.add(dns::Socket::new(&[], Vec::new()));

            let mut state = State {
                link_up: false,
                local_sockets,

                dhcpv4: None,

                dns_socket,
                dns_servers_ipv4: Default::default(),
                dns_servers_ipv6: Default::default(),
                dns_waker: AtomicWaker::new(),
            };

            match config.ipv4 {
                ConfigV4::Static(config) => state.apply_ipv4_config(&mut iface, config),
                ConfigV4::Dhcp(config) => {
                    let mut dhcpv4 = dhcpv4::Socket::new();
                    state.apply_dhcpv4_config(&mut dhcpv4, config);
                    state.dhcpv4 = Some(state.local_sockets.add(dhcpv4));
                }
                ConfigV4::None => {}
            }

            if let ConfigV6::Static(config) = config.ipv6 {
                state.apply_ipv6_config(&mut iface, config)
            }

            socket.ifaces.push(iface);
            devices.push(device);
            states.push(state);
        }

        Arsc::new(Stack {
            devices,
            socket: RwLock::new(socket),
            states: RwLock::new(states),
        })
    }

    pub fn run(self: Arsc<Self>) -> StackBackground {
        StackBackground {
            stack: self,
            timer: None,
        }
    }
}

impl Stack {
    pub fn max_transmission_unit(&self, iface_id: usize) -> usize {
        if iface_id == 0 {
            self.with_socket(|s| s.loopback.capabilities().max_transmission_unit)
        } else {
            ksync::critical(|| self.devices[iface_id - 1].read().features().max_unit)
        }
    }

    pub async fn dns_query(&self, name: &str, ty: DnsQueryType) -> Vec<IpAddress> {
        struct CallOnDrop<F: FnOnce()>(Option<F>);
        impl<F: FnOnce()> Drop for CallOnDrop<F> {
            fn drop(&mut self) {
                if let Some(f) = self.0.take() {
                    f()
                }
            }
        }

        match ty {
            DnsQueryType::A if let Ok(ip) = name.parse().map(IpAddress::Ipv4) =>
    {             return [ip].into_iter().collect()
            }
            DnsQueryType::Aaaa if let Ok(ip) = name.parse().map(IpAddress::Ipv6)
    => {             return [ip].into_iter().collect()
            },
            _ => {}
        }

        let len = self.devices.len();

        let futures = (1..=len).map(|iface_id| async move {
            let handle = poll_fn(|cx| self.dns_start_query(name, ty, iface_id, cx)).await?;

            let mut cancel = CallOnDrop(Some(|| self.dns_cancel(handle, iface_id)));
            let result = poll_fn(|cx| self.dns_get_result(handle, iface_id, cx)).await?;
            cancel.0 = None;

            Ok::<_, DnsError>(result)
        });

        let results = join_all(futures).await;

        let iter = results.into_iter().filter_map(|res| match res {
            Ok(res) => Some(res),
            Err(err) => {
                log::error!("DNS query error: {err:?}");
                None
            }
        });
        iter.flatten().collect()
    }

    fn dns_start_query(
        &self,
        name: &str,
        ty: DnsQueryType,
        iface_id: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<dns::QueryHandle, dns::StartQueryError>> {
        self.with_mut(|_, state, s| {
            let state = &mut state[iface_id];
            let iface = &mut s.ifaces[iface_id];

            let socket = s.sockets.get_mut::<dns::Socket>(state.dns_socket);
            match socket.start_query(iface.context(), name, ty) {
                Err(dns::StartQueryError::NoFreeSlot) => {
                    state.dns_waker.register(cx.waker());
                    Poll::Pending
                }
                res => Poll::Ready(res),
            }
        })
    }

    fn dns_get_result(
        &self,
        handle: dns::QueryHandle,
        iface_id: usize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<heapless::Vec<IpAddress, 1>, dns::GetQueryResultError>> {
        self.with_mut(|_, state, s| {
            let state = &mut state[iface_id];

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

    fn dns_cancel(&self, handle: dns::QueryHandle, iface_id: usize) {
        self.with_mut(|_, state, s| {
            let state = &mut state[iface_id];

            let socket = s.sockets.get_mut::<dns::Socket>(state.dns_socket);
            socket.cancel_query(handle);
            s.waker.wake();
            state.dns_waker.wake();
        })
    }

    fn poll(&self, cx: &mut Context) -> Option<Instant> {
        ksync::critical(|| {
            let mut write = self.socket.write();
            write.poll(cx, &self.devices, &mut self.states.write())
        })
    }
}

impl State {
    fn apply_ipv4_config(&mut self, iface: &mut Interface, config: StaticConfigV4) {
        log::info!("Applying IPv4 configuration:");
        log::info!("    IP address: {}", config.address);
        iface.update_ip_addrs(|addrs| match addrs.first_mut() {
            Some(addr) => *addr = IpCidr::Ipv4(config.address),
            None => addrs.push(IpCidr::Ipv4(config.address)).unwrap(),
        });

        if let Some(gateway) = config.gateway {
            log::info!("    Gateway: {gateway}");
            iface.routes_mut().add_default_ipv4_route(gateway).unwrap();
        } else {
            iface.routes_mut().remove_default_ipv4_route();
        }

        self.dns_servers_ipv4 = config.dns_servers;
        self.update_dns_servers();
    }

    fn apply_ipv6_config(&mut self, iface: &mut Interface, config: StaticConfigV6) {
        log::info!("Applying IPv6 configuration:");
        log::info!("    IP address: {}", config.address);
        iface.update_ip_addrs(|addrs| match addrs.first_mut() {
            Some(addr) => *addr = IpCidr::Ipv6(config.address),
            None => addrs.push(IpCidr::Ipv6(config.address)).unwrap(),
        });

        if let Some(gateway) = config.gateway {
            log::info!("    Gateway: {gateway}");
            iface.routes_mut().add_default_ipv6_route(gateway).unwrap();
        } else {
            iface.routes_mut().remove_default_ipv6_route();
        }

        self.dns_servers_ipv6 = config.dns_servers;
        self.update_dns_servers();
    }

    fn update_dns_servers(&mut self) {
        let socket = self.local_sockets.get_mut::<dns::Socket>(self.dns_socket);
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

    fn unapply_dhcpv4_config(&mut self, iface: &mut Interface) {
        iface.update_ip_addrs(|addrs| addrs.clear());
        iface.routes_mut().remove_default_ipv4_route();
        self.dns_servers_ipv4.clear();
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

    pub fn select_tcp_addr(&mut self, remote: &IpEndpoint, local: &mut IpListenEndpoint) -> usize {
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
            // TODO: Select a better interface.
            self.ifaces.len() - 1
        }
    }

    fn poll(
        &mut self,
        cx: &mut Context,
        device: &[Arc<RwLock<dyn Net>>],
        state: &mut [State],
    ) -> Option<Instant> {
        self.waker.register(cx.waker());

        let instant = instant_to_smoltcp(ktime::Instant::now());

        for ((iface, dev), state) in self.ifaces.iter_mut().skip(1).zip(device).zip(state) {
            let mut dev = dev.write();
            Stack::update_interface(iface, &*dev);

            let mut poller = Tracer::new(dev.with_cx(Some(cx)), |i, p| writer(1, i, p));
            iface.poll(instant, &mut poller, &mut self.sockets);
            iface.poll(instant, &mut poller, &mut state.local_sockets);

            let old = mem::replace(&mut state.link_up, dev.is_link_up());
            if old != state.link_up {
                let s = if state.link_up { "up" } else { "down" };
                log::info!("Net device {:x?} link {s}", dev.address());
            }

            if let Some(dhcpv4) = state.dhcpv4 {
                let socket = state.local_sockets.get_mut::<dhcpv4::Socket>(dhcpv4);
                if state.link_up {
                    match socket.poll() {
                        Some(dhcpv4::Event::Configured(config)) => {
                            let config = StaticConfigV4 {
                                address: config.address,
                                gateway: config.router,
                                dns_servers: config.dns_servers,
                            };
                            state.apply_ipv4_config(iface, config)
                        }
                        Some(dhcpv4::Event::Deconfigured) => state.unapply_dhcpv4_config(iface),
                        None => {}
                    }
                } else if old {
                    socket.reset();
                    state.unapply_dhcpv4_config(iface)
                }
            }
        }

        self.ifaces[0].poll(instant, &mut self.loopback, &mut self.sockets);

        let ddl = self.ifaces.iter_mut().fold(None, |acc, iface| {
            let next = iface.poll_at(instant, &self.sockets);
            match (acc, next) {
                (None, next) => next,
                (acc, None) => acc,
                (Some(acc), Some(next)) => Some(next.min(acc)),
            }
        });
        ddl.map(instant_from_smoltcp)
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
            let ddl = self.stack.poll(cx);
            // log::trace!("End polling net stack with deadline {ddl:?}");
            match ddl {
                Some(ddl) if ddl.to_su() == (0, 0) => {
                    cx.waker().wake_by_ref();
                    break Poll::Pending;
                }
                Some(ddl) => self.timer = Some(ddl.into()),
                None => break Poll::Pending,
            }
        }
    }
}
