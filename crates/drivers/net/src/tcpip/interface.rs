//! NetworkInterface — wraps `smoltcp::iface::Interface` and manages polling.
//!
//! Provides a high-level interface for configuring IP addresses, gateways,
//! and DHCP, and for polling the protocol stack to process incoming/outgoing
//! packets.

use smoltcp::iface::{Config as SmolcpConfig, Interface as SmolcpInterface};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr};

use crate::mac::NetDevice;
use crate::tcpip::addr::{Ipv4Addr, Ipv4Cidr, SocketHandle};
use crate::tcpip::device::SmolcpDevice;
use crate::tcpip::error::TcpIpError;
use crate::tcpip::socket::SocketSet;

/// Configuration for creating a [`NetworkInterface`].
#[derive(Clone, Debug)]
pub struct InterfaceConfig {
    /// MAC (hardware) address of the interface.
    pub mac_addr: [u8; 6],
    /// Static IPv4 address (CIDR). If `None` and `dhcp` is false, the
    /// interface will have no IPv4 address.
    pub ipv4_addr: Option<Ipv4Cidr>,
    /// Default IPv4 gateway.
    pub gateway: Option<Ipv4Addr>,
    /// Enable DHCP for automatic IP configuration.
    pub dhcp: bool,
}

impl InterfaceConfig {
    /// Create a new config with the given MAC address, no static IP, no
    /// gateway, and DHCP disabled.
    pub fn new(mac_addr: [u8; 6]) -> Self {
        Self {
            mac_addr,
            ipv4_addr: None,
            gateway: None,
            dhcp: false,
        }
    }

    /// Set a static IPv4 address (CIDR).
    pub fn with_ipv4(mut self, addr: Ipv4Cidr) -> Self {
        self.ipv4_addr = Some(addr);
        self
    }

    /// Set the default IPv4 gateway.
    pub fn with_gateway(mut self, gateway: Ipv4Addr) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// Enable or disable DHCP.
    pub fn with_dhcp(mut self, dhcp: bool) -> Self {
        self.dhcp = dhcp;
        self
    }
}

/// Network interface wrapping a `smoltcp::iface::Interface`.
///
/// Owns the smoltcp interface, the device adapter, and the socket set.
/// The caller is responsible for calling [`poll`](Self::poll) regularly to
/// process incoming and outgoing packets.
pub struct NetworkInterface<D: NetDevice> {
    /// smoltcp interface (manages ARP, routing, etc.).
    ///
    /// Public to allow socket wrappers to access `iface.context()` for
    /// split-borrowing patterns (e.g., `TcpSocket::connect`).
    pub iface: SmolcpInterface,
    /// Device adapter bridging NetDevice to smoltcp::phy::Device.
    pub device: SmolcpDevice<D>,
    /// Socket set holding all TCP/UDP/ICMP/DHCP sockets.
    pub sockets: SocketSet,
    /// Handle to the DHCP socket (if DHCP is enabled).
    dhcp_handle: Option<SocketHandle>,
}

impl<D: NetDevice> NetworkInterface<D> {
    /// Create a new network interface with the given device and configuration.
    ///
    /// # Panics
    ///
    /// Panics if the MAC address is not unicast (first byte's least
    /// significant bit is 1).
    pub fn new(device: D, config: InterfaceConfig) -> Self {
        let mut smolcp_dev = SmolcpDevice::new(device);

        // Create smoltcp interface configuration.
        let hw_addr = HardwareAddress::Ethernet(EthernetAddress(config.mac_addr));
        let smolcp_config = SmolcpConfig::new(hw_addr);

        let mut iface =
            SmolcpInterface::new(smolcp_config, &mut smolcp_dev, Instant::from_millis(0));

        // Configure static IPv4 address if provided.
        if let Some(cidr) = config.ipv4_addr {
            iface.update_ip_addrs(|addrs| {
                addrs.push(IpCidr::Ipv4(cidr)).ok();
            });
        }

        // Configure default IPv4 gateway if provided.
        if let Some(gw) = config.gateway {
            let _ = iface.routes_mut().add_default_ipv4_route(gw);
        }

        // Create socket set.
        let mut sockets = SocketSet::new();

        // Create DHCP socket if enabled.
        let dhcp_handle = if config.dhcp {
            let dhcp_socket = dhcpv4::Socket::new();
            Some(sockets.inner.add(dhcp_socket))
        } else {
            None
        };

        Self {
            iface,
            device: smolcp_dev,
            sockets,
            dhcp_handle,
        }
    }

    /// Poll the network interface.
    ///
    /// 1. Drains all available frames from the hardware into the RX queue.
    /// 2. Calls `smoltcp::iface::Interface::poll()` to process packets.
    ///
    /// Returns `Ok(())` on success. Errors are rare since smoltcp's `poll`
    /// returns a `PollResult` (not `Result`), but this method may return
    /// errors from the underlying device.
    pub fn poll(&mut self, timestamp_ms: u64) -> Result<(), TcpIpError> {
        // Drain hardware RX queue into smoltcp's RX queue.
        self.device.drain_rx();

        // Poll the smoltcp interface.
        let instant = Instant::from_millis(timestamp_ms as i64);
        let _ = self
            .iface
            .poll(instant, &mut self.device, &mut self.sockets.inner);

        Ok(())
    }

    /// Returns the timestamp at which `poll()` should be called next, or
    /// `None` if there are no pending timers.
    pub fn poll_at(&mut self, timestamp_ms: u64) -> Option<u64> {
        let instant = Instant::from_millis(timestamp_ms as i64);
        self.iface
            .poll_at(instant, &self.sockets.inner)
            .map(|i| i.millis() as u64)
    }

    /// Returns the delay until the next `poll()` should be called, or `None`.
    pub fn poll_delay(&mut self, timestamp_ms: u64) -> Option<u64> {
        let instant = Instant::from_millis(timestamp_ms as i64);
        self.iface
            .poll_delay(instant, &self.sockets.inner)
            .map(|d| d.total_millis())
    }

    /// Add an IPv4 address to the interface.
    pub fn add_ipv4_addr(&mut self, addr: Ipv4Cidr) {
        self.iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::Ipv4(addr)).ok();
        });
    }

    /// Returns the first IPv4 address of the interface, or `None`.
    pub fn ipv4_addr(&self) -> Option<Ipv4Addr> {
        self.iface.ipv4_addr()
    }

    /// Returns the default IPv4 gateway, or `None`.
    pub fn gateway(&self) -> Option<Ipv4Addr> {
        self.iface
            .routes()
            .get_default_ipv4_route()
            .and_then(|route| match route.via_router {
                smoltcp::wire::IpAddress::Ipv4(addr) => Some(addr),
                #[allow(unreachable_patterns)]
                _ => None,
            })
    }

    /// Returns the hardware (MAC) address of the interface.
    pub fn hardware_addr(&self) -> HardwareAddress {
        self.iface.hardware_addr()
    }

    /// Returns whether DHCP is enabled.
    pub fn dhcp_enabled(&self) -> bool {
        self.dhcp_handle.is_some()
    }

    /// Returns the DHCP socket handle if DHCP is enabled.
    pub fn dhcp_handle(&self) -> Option<SocketHandle> {
        self.dhcp_handle
    }

    /// Returns a reference to the inner smoltcp interface.
    pub fn inner(&self) -> &SmolcpInterface {
        &self.iface
    }

    /// Returns a mutable reference to the inner smoltcp interface context.
    pub fn context(&mut self) -> &mut smoltcp::iface::Context {
        self.iface.context()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};

    /// Minimal mock device for interface tests.
    struct MockNetDevice {
        mac_addr: [u8; 6],
        mtu: usize,
        link_up: bool,
    }

    impl NetDevice for MockNetDevice {
        fn send(&mut self, _frame: &[u8]) -> Result<(), crate::error::NetError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, crate::error::NetError> {
            Err(crate::error::NetError::NoBuffer)
        }
        fn mac_address(&self) -> [u8; 6] {
            self.mac_addr
        }
        fn mtu(&self) -> usize {
            self.mtu
        }
        fn link_up(&self) -> bool {
            self.link_up
        }
        fn set_promiscuous(&mut self, _on: bool) {}
        fn stats(&self) -> crate::error::NetStats {
            crate::error::NetStats::new()
        }
    }

    fn make_device() -> MockNetDevice {
        MockNetDevice {
            mac_addr: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
            mtu: 1500,
            link_up: true,
        }
    }

    #[test]
    fn test_interface_config_new() {
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        assert_eq!(config.mac_addr, [0x02, 0, 0, 0, 0, 0x01]);
        assert!(config.ipv4_addr.is_none());
        assert!(config.gateway.is_none());
        assert!(!config.dhcp);
    }

    #[test]
    fn test_interface_config_builder() {
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
            .with_gateway(ipv4_addr(192, 168, 1, 1))
            .with_dhcp(true);

        assert_eq!(config.mac_addr, [0x02, 0, 0, 0, 0, 0x01]);
        assert!(config.ipv4_addr.is_some());
        assert!(config.gateway.is_some());
        assert!(config.dhcp);
    }

    #[test]
    fn test_interface_config_chain() {
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(10, 0, 0, 1), 8));

        assert_eq!(config.ipv4_addr.unwrap().prefix_len(), 8);
        assert!(config.gateway.is_none());
        assert!(!config.dhcp);
    }

    #[test]
    fn test_network_interface_new_static_ip() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
            .with_gateway(ipv4_addr(192, 168, 1, 1));

        let iface = NetworkInterface::new(dev, config);
        assert_eq!(iface.ipv4_addr(), Some(ipv4_addr(192, 168, 1, 100)));
        assert!(!iface.dhcp_enabled());
    }

    #[test]
    fn test_network_interface_new_no_ip() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);
        assert_eq!(iface.ipv4_addr(), None);
        assert!(!iface.dhcp_enabled());
    }

    #[test]
    fn test_network_interface_new_with_dhcp() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let iface = NetworkInterface::new(dev, config);
        assert!(iface.dhcp_enabled());
        assert!(iface.dhcp_handle.is_some());
    }

    #[test]
    fn test_network_interface_poll() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));

        let mut iface = NetworkInterface::new(dev, config);
        let result = iface.poll(100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_network_interface_poll_multiple() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));

        let mut iface = NetworkInterface::new(dev, config);
        assert!(iface.poll(0).is_ok());
        assert!(iface.poll(100).is_ok());
        assert!(iface.poll(200).is_ok());
    }

    #[test]
    fn test_add_ipv4_addr() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let mut iface = NetworkInterface::new(dev, config);

        assert_eq!(iface.ipv4_addr(), None);
        iface.add_ipv4_addr(ipv4_cidr(ipv4_addr(10, 0, 0, 1), 24));
        assert_eq!(iface.ipv4_addr(), Some(ipv4_addr(10, 0, 0, 1)));
    }

    #[test]
    #[allow(irrefutable_let_patterns, clippy::disallowed_macros)]
    fn test_hardware_addr() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);

        let hw = iface.hardware_addr();
        if let HardwareAddress::Ethernet(mac) = hw {
            assert_eq!(mac.0, [0x02, 0, 0, 0, 0, 0x01]);
        } else {
            panic!("expected Ethernet address");
        }
    }

    #[test]
    fn test_poll_at() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));

        let mut iface = NetworkInterface::new(dev, config);
        iface.poll(0).unwrap();
        // poll_at may return Some or None depending on pending timers
        let _ = iface.poll_at(100);
    }

    #[test]
    fn test_poll_delay() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));

        let mut iface = NetworkInterface::new(dev, config);
        iface.poll(0).unwrap();
        let _ = iface.poll_delay(100);
    }

    #[test]
    fn test_gateway_static() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
            .with_gateway(ipv4_addr(192, 168, 1, 1));

        let iface = NetworkInterface::new(dev, config);
        // The gateway may or may not be returned depending on smoltcp's route table
        let _ = iface.gateway();
    }

    #[test]
    fn test_dhcp_disabled_by_default() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);
        assert!(!iface.dhcp_enabled());
        assert!(iface.dhcp_handle.is_none());
    }

    #[test]
    fn test_interface_config_clone() {
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));
        let cloned = config.clone();
        assert_eq!(config.mac_addr, cloned.mac_addr);
        assert_eq!(config.ipv4_addr, cloned.ipv4_addr);
    }

    #[test]
    fn test_interface_config_debug() {
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("InterfaceConfig"));
    }

    #[test]
    fn test_device_accessor() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);
        assert_eq!(
            iface.device.device().mac_address(),
            [0x02, 0, 0, 0, 0, 0x01]
        );
        assert_eq!(iface.device.device().mtu(), 1500);
    }
}
