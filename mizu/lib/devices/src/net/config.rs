use core::time::Duration;

use heapless::Vec;
use smoltcp::{
    socket::dhcpv4::RetryConfig,
    wire::{Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr},
};

/// Static IP address configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticConfigV4 {
    /// IP address and subnet mask.
    pub address: Ipv4Cidr,
    /// Default gateway.
    pub gateway: Option<Ipv4Address>,
    /// DNS servers.
    pub dns_servers: Vec<Ipv4Address, 3>,
}

/// Static IPv6 address configuration
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticConfigV6 {
    /// IP address and subnet mask.
    pub address: Ipv6Cidr,
    /// Default gateway.
    pub gateway: Option<Ipv6Address>,
    /// DNS servers.
    pub dns_servers: Vec<Ipv6Address, 3>,
}

/// DHCP configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpV4Config {
    /// Maximum lease duration.
    ///
    /// If not set, the lease duration specified by the server will be used.
    /// If set, the lease duration will be capped at this value.
    pub max_lease_duration: Option<Duration>,
    /// Retry configuration.
    pub retry_config: RetryConfig,
    /// Ignore NAKs from DHCP servers.
    ///
    /// This is not compliant with the DHCP RFCs, since theoretically we must
    /// stop using the assigned IP when receiving a NAK. This can increase
    /// reliability on broken networks with buggy routers or rogue DHCP servers,
    /// however.
    pub ignore_naks: bool,
    /// Server port. This is almost always 67. Do not change unless you know
    /// what you're doing.
    pub server_port: u16,
    /// Client port. This is almost always 68. Do not change unless you know
    /// what you're doing.
    pub client_port: u16,
}

impl Default for DhcpV4Config {
    fn default() -> Self {
        Self {
            max_lease_duration: Default::default(),
            retry_config: Default::default(),
            ignore_naks: Default::default(),
            server_port: smoltcp::wire::DHCP_SERVER_PORT,
            client_port: smoltcp::wire::DHCP_CLIENT_PORT,
        }
    }
}

/// Network stack configuration.
#[derive(Default)]
pub struct Config {
    /// IPv4 configuration
    pub ipv4: ConfigV4,
    /// IPv6 configuration
    pub ipv6: ConfigV6,
}

impl Config {
    /// IPv4 configuration with static addressing.
    pub fn ipv4_static(config: StaticConfigV4) -> Self {
        Self {
            ipv4: ConfigV4::Static(config),
            ipv6: ConfigV6::None,
        }
    }

    /// IPv6 configuration with static addressing.
    pub fn ipv6_static(config: StaticConfigV6) -> Self {
        Self {
            ipv4: ConfigV4::None,
            ipv6: ConfigV6::Static(config),
        }
    }

    /// IPv6 configuration with dynamic addressing.
    ///
    /// # Example
    /// ```rust
    /// let _cfg = Config::dhcpv4(Default::default());
    /// ```
    pub fn dhcpv4(config: DhcpV4Config) -> Self {
        Self {
            ipv4: ConfigV4::Dhcp(config),
            ipv6: ConfigV6::None,
        }
    }
}

/// Network stack IPv4 configuration.
pub enum ConfigV4 {
    /// Use a static IPv4 address configuration.
    Static(StaticConfigV4),
    /// Use DHCP to obtain an IP address configuration.
    Dhcp(DhcpV4Config),
    /// Do not configure IPv6.
    None,
}

impl Default for ConfigV4 {
    fn default() -> Self {
        ConfigV4::Dhcp(Default::default())
    }
}

/// Network stack IPv6 configuration.
#[derive(Default)]
pub enum ConfigV6 {
    /// Use a static IPv6 address configuration.
    Static(StaticConfigV6),
    /// Do not configure IPv6.
    #[default]
    None,
}
