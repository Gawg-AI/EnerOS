//! Address type aliases for the TCP/IP stack.
//!
//! Re-exports smoltcp's wire-layer address types under shorter names so that
//! callers do not need to depend on smoltcp directly. Provides helper
//! constructors [`ipv4_addr`] and [`ipv4_cidr`] for ergonomic address creation.

/// IPv4 address (wraps `smoltcp::wire::Ipv4Address`).
pub type Ipv4Addr = smoltcp::wire::Ipv4Address;

/// IPv4 CIDR (address + prefix length, wraps `smoltcp::wire::Ipv4Cidr`).
pub type Ipv4Cidr = smoltcp::wire::Ipv4Cidr;

/// Hardware (MAC) address (wraps `smoltcp::wire::HardwareAddress`).
pub type HardwareAddress = smoltcp::wire::HardwareAddress;

/// Socket address = IP + port (wraps `smoltcp::wire::IpEndpoint`).
pub type SocketAddr = smoltcp::wire::IpEndpoint;

/// Socket handle — identifies a socket within a [`tcpip::SocketSet`].
pub type SocketHandle = smoltcp::iface::SocketHandle;

/// Create an IPv4 address from four octets.
///
/// ```
/// # use eneros_net::tcpip::addr::ipv4_addr;
/// let addr = ipv4_addr(192, 168, 1, 1);
/// ```
pub fn ipv4_addr(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
    smoltcp::wire::Ipv4Address::new(a, b, c, d)
}

/// Create an IPv4 CIDR from an address and prefix length.
///
/// ```
/// # use eneros_net::tcpip::addr::{ipv4_addr, ipv4_cidr};
/// let cidr = ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24);
/// ```
pub fn ipv4_cidr(addr: Ipv4Addr, prefix_len: u8) -> Ipv4Cidr {
    smoltcp::wire::Ipv4Cidr::new(addr, prefix_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_addr_basic() {
        let addr = ipv4_addr(192, 168, 1, 1);
        assert_eq!(addr, smoltcp::wire::Ipv4Address::new(192, 168, 1, 1));
    }

    #[test]
    fn test_ipv4_addr_zeros() {
        let addr = ipv4_addr(0, 0, 0, 0);
        assert_eq!(addr, smoltcp::wire::Ipv4Address::UNSPECIFIED);
    }

    #[test]
    fn test_ipv4_addr_broadcast() {
        let addr = ipv4_addr(255, 255, 255, 255);
        assert_eq!(addr, smoltcp::wire::Ipv4Address::BROADCAST);
    }

    #[test]
    fn test_ipv4_cidr_basic() {
        let addr = ipv4_addr(10, 0, 0, 1);
        let cidr = ipv4_cidr(addr, 8);
        assert_eq!(cidr.address(), addr);
        assert_eq!(cidr.prefix_len(), 8);
    }

    #[test]
    fn test_ipv4_cidr_24() {
        let cidr = ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24);
        assert_eq!(cidr.prefix_len(), 24);
        assert_eq!(cidr.address(), ipv4_addr(192, 168, 1, 100));
    }

    #[test]
    fn test_ipv4_cidr_32() {
        let cidr = ipv4_cidr(ipv4_addr(172, 16, 0, 1), 32);
        assert_eq!(cidr.prefix_len(), 32);
    }

    #[test]
    fn test_type_alias_equivalence() {
        let addr: Ipv4Addr = smoltcp::wire::Ipv4Address::new(1, 2, 3, 4);
        assert_eq!(addr, ipv4_addr(1, 2, 3, 4));
    }

    #[test]
    #[allow(irrefutable_let_patterns, clippy::disallowed_macros)]
    fn test_hardware_address_ethernet() {
        let hw = HardwareAddress::Ethernet(smoltcp::wire::EthernetAddress([
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
        ]));
        if let HardwareAddress::Ethernet(mac) = hw {
            assert_eq!(mac.0, [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        } else {
            panic!("expected Ethernet hardware address");
        }
    }
}
