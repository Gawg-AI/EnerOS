//! DHCP client wrapper for smoltcp's `dhcpv4::Socket`.
//!
//! Provides [`DhcpClient`] — a high-level wrapper around
//! `smoltcp::socket::dhcpv4::Socket` — and the [`DhcpState`] / [`DhcpLease`]
//! types for tracking DHCP status.
//!
//! # smoltcp 0.13 DHCP API
//!
//! In smoltcp 0.13, the `dhcpv4::Socket` uses an **event-driven** model:
//! - `socket.poll()` returns `Option<Event>` (`Deconfigured` or `Configured`)
//! - There is no public `state()` method to query the internal state machine
//! - The caller must track state by processing events
//!
//! [`DhcpClient`] wraps this by storing the current state and lease, updated
//! each time [`poll`](DhcpClient::poll) is called.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_net::{DhcpClient, InterfaceConfig, NetworkInterface};
//!
//! let mut iface = NetworkInterface::new(device, InterfaceConfig::new(mac).with_dhcp(true));
//! let mut dhcp = DhcpClient::new();
//! dhcp.start(&mut iface).unwrap();
//!
//! // In main loop:
//! iface.poll(timestamp).unwrap();
//! let state = dhcp.poll(&mut iface).unwrap();
//! if state.is_bound() {
//!     if let Some(lease) = dhcp.lease(&iface) {
//!         // Use lease.addr, lease.gateway, etc.
//!     }
//! }
//! ```

use alloc::vec::Vec;

use smoltcp::socket::dhcpv4;

use crate::mac::NetDevice;
use crate::tcpip::addr::{Ipv4Addr, Ipv4Cidr, SocketHandle};
use crate::tcpip::error::TcpIpError;
use crate::tcpip::interface::NetworkInterface;

// ---------------------------------------------------------------------------
// DhcpState
// ---------------------------------------------------------------------------

/// DHCP client state machine.
///
/// Tracks the state of the DHCP client. In smoltcp 0.13, the internal state
/// machine is not publicly accessible, so [`DhcpClient`] tracks state based
/// on events received from `socket.poll()`.
///
/// - [`Init`](Self::Init) — no lease acquired yet (covers smoltcp's
///   `Discovering` and `Requesting` states).
/// - [`Bound`](Self::Bound) — a lease has been acquired.
///
/// The `Selecting`, `Requesting`, `Renewing`, and `Rebinding` variants are
/// defined for API completeness but are not actively tracked with the current
/// smoltcp 0.13 API. They may be used in future versions or when the smoltcp
/// API exposes more state information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    /// Initial state — no DHCPDISCOVER sent yet.
    Init,
    /// Sent DHCPDISCOVER, waiting for DHCPOFFER.
    Selecting,
    /// Sent DHCPREQUEST, waiting for DHCPACK.
    Requesting,
    /// Received DHCPACK — have a valid lease.
    Bound,
    /// T1 timer expired — renewing the lease with the original server.
    Renewing,
    /// T2 timer expired — rebinding with any server.
    Rebinding,
}

impl DhcpState {
    /// Returns `true` if the client has a valid lease (Bound, Renewing, or
    /// Rebinding).
    pub fn is_bound(self) -> bool {
        matches!(
            self,
            DhcpState::Bound | DhcpState::Renewing | DhcpState::Rebinding
        )
    }

    /// Returns `true` if the client is in the process of acquiring a lease
    /// (Init, Selecting, or Requesting).
    pub fn is_acquiring(self) -> bool {
        matches!(
            self,
            DhcpState::Init | DhcpState::Selecting | DhcpState::Requesting
        )
    }

    /// Returns `true` if the client is in the initial state (no lease).
    pub fn is_init(self) -> bool {
        matches!(self, DhcpState::Init)
    }
}

impl core::fmt::Display for DhcpState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DhcpState::Init => write!(f, "Init"),
            DhcpState::Selecting => write!(f, "Selecting"),
            DhcpState::Requesting => write!(f, "Requesting"),
            DhcpState::Bound => write!(f, "Bound"),
            DhcpState::Renewing => write!(f, "Renewing"),
            DhcpState::Rebinding => write!(f, "Rebinding"),
        }
    }
}

// ---------------------------------------------------------------------------
// DhcpLease
// ---------------------------------------------------------------------------

/// DHCP lease information.
///
/// Contains the network configuration received from the DHCP server.
/// Created from a `smoltcp::socket::dhcpv4::Config` when a `Configured` event
/// is received.
#[derive(Debug, Clone)]
pub struct DhcpLease {
    /// Assigned IPv4 address (CIDR).
    pub addr: Ipv4Cidr,
    /// Default gateway (router).
    pub gateway: Option<Ipv4Addr>,
    /// DNS servers.
    pub dns_servers: Vec<Ipv4Addr>,
    /// Lease duration in milliseconds.
    ///
    /// **Note**: smoltcp 0.13's `Config` does not expose the lease duration.
    /// This field is set to 0 (unknown) until smoltcp provides this info.
    pub lease_duration: u64,
    /// DHCP server identifier.
    pub server_id: Ipv4Addr,
}

impl DhcpLease {
    /// Create a new `DhcpLease` from a smoltcp DHCP `Config`.
    ///
    /// Extracts the address, gateway, DNS servers, and server identifier.
    /// The `lease_duration` is set to 0 (not available in smoltcp 0.13's Config).
    pub fn from_config(config: &dhcpv4::Config<'_>) -> Self {
        let dns_servers: Vec<Ipv4Addr> = config.dns_servers.iter().copied().collect();
        Self {
            addr: config.address,
            gateway: config.router,
            dns_servers,
            lease_duration: 0,
            server_id: config.server.identifier,
        }
    }

    /// Returns `true` if the lease has a gateway configured.
    pub fn has_gateway(&self) -> bool {
        self.gateway.is_some()
    }

    /// Returns the number of DNS servers in the lease.
    pub fn dns_server_count(&self) -> usize {
        self.dns_servers.len()
    }
}

// ---------------------------------------------------------------------------
// DhcpClient
// ---------------------------------------------------------------------------

/// DHCP client wrapper.
///
/// Holds a [`SocketHandle`] that identifies the DHCP socket within a
/// [`NetworkInterface`]'s socket set. The socket is typically created by
/// [`NetworkInterface::new`] when DHCP is enabled, but can also be created
/// via [`DhcpClient::start`].
///
/// The client tracks the DHCP state and lease by processing events from
/// `socket.poll()` each time [`poll`](Self::poll) is called.
pub struct DhcpClient {
    /// Handle to the DHCP socket in the interface's socket set.
    handle: Option<SocketHandle>,
    /// Current DHCP state (tracked locally).
    state: DhcpState,
    /// Current DHCP lease (if bound).
    lease: Option<DhcpLease>,
}

impl DhcpClient {
    /// Create a new DHCP client without a socket.
    ///
    /// Call [`start`](Self::start) to associate the client with a DHCP socket
    /// on a [`NetworkInterface`].
    pub fn new() -> Self {
        Self {
            handle: None,
            state: DhcpState::Init,
            lease: None,
        }
    }

    /// Create a DHCP client with an explicit socket handle.
    pub fn with_handle(handle: SocketHandle) -> Self {
        Self {
            handle: Some(handle),
            state: DhcpState::Init,
            lease: None,
        }
    }

    /// Create a DHCP client from a [`NetworkInterface`] that has DHCP enabled.
    ///
    /// Returns `None` if DHCP is not enabled on the interface.
    pub fn from_iface<D: NetDevice>(iface: &NetworkInterface<D>) -> Option<Self> {
        iface.dhcp_handle().map(Self::with_handle)
    }

    /// Returns the socket handle, or `None` if not started.
    pub fn handle(&self) -> Option<SocketHandle> {
        self.handle
    }

    /// Start DHCP on the interface.
    ///
    /// If the interface already has a DHCP socket (created during
    /// `NetworkInterface::new` with `dhcp = true`), this method reuses it.
    /// Otherwise, a new DHCP socket is created and added to the interface's
    /// socket set.
    pub fn start<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
    ) -> Result<(), TcpIpError> {
        if self.handle.is_some() {
            return Ok(());
        }

        // Use the interface's existing DHCP socket if available.
        if let Some(handle) = iface.dhcp_handle() {
            self.handle = Some(handle);
            return Ok(());
        }

        // Otherwise, create a new DHCP socket.
        let dhcp_socket = dhcpv4::Socket::new();
        let handle = iface.sockets.inner.add(dhcp_socket);
        self.handle = Some(handle);
        Ok(())
    }

    /// Poll the DHCP client for state changes.
    ///
    /// Calls `socket.poll()` on the underlying smoltcp DHCP socket to retrieve
    /// configuration change events. Updates the tracked state and lease
    /// accordingly.
    ///
    /// **Note**: This method does NOT drive the DHCP state machine. The caller
    /// must call `NetworkInterface::poll()` regularly to process DHCP packets.
    /// This method only retrieves events that resulted from that processing.
    ///
    /// Returns the current DHCP state.
    pub fn poll<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
    ) -> Result<DhcpState, TcpIpError> {
        let handle = self.handle.ok_or(TcpIpError::InvalidArgument)?;
        let socket = iface.sockets.inner.get_mut::<dhcpv4::Socket>(handle);

        // Process all pending DHCP events.
        while let Some(event) = socket.poll() {
            match event {
                dhcpv4::Event::Deconfigured => {
                    self.state = DhcpState::Init;
                    self.lease = None;
                }
                dhcpv4::Event::Configured(config) => {
                    self.state = DhcpState::Bound;
                    self.lease = Some(DhcpLease::from_config(&config));
                }
            }
        }

        Ok(self.state)
    }

    /// Returns the current DHCP state (without polling for events).
    pub fn state(&self) -> DhcpState {
        self.state
    }

    /// Returns the current DHCP lease, or `None` if not bound.
    ///
    /// The `iface` parameter is accepted for API consistency but is not used
    /// — the lease is tracked internally and updated during [`poll`](Self::poll).
    pub fn lease<D: NetDevice>(&self, _iface: &NetworkInterface<D>) -> Option<DhcpLease> {
        self.lease.clone()
    }

    /// Returns a reference to the current DHCP lease, or `None` if not bound.
    pub fn lease_ref(&self) -> Option<&DhcpLease> {
        self.lease.as_ref()
    }

    /// Returns `true` if the client has been started.
    pub fn is_started(&self) -> bool {
        self.handle.is_some()
    }

    /// Returns `true` if the client has a valid lease.
    pub fn is_bound(&self) -> bool {
        self.state.is_bound()
    }

    /// Reset the DHCP client.
    ///
    /// Clears the state and lease. The socket handle is retained, so the
    /// client can continue polling after the interface is re-polled.
    pub fn reset(&mut self) {
        self.state = DhcpState::Init;
        self.lease = None;
    }
}

impl Default for DhcpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for DhcpClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DhcpClient")
            .field("handle", &self.handle)
            .field("state", &self.state)
            .field("has_lease", &self.lease.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};
    use crate::tcpip::interface::InterfaceConfig;

    /// Minimal mock device for DHCP tests.
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
            mac_addr: [0x02, 0, 0, 0, 0, 0x01],
            mtu: 1500,
            link_up: true,
        }
    }

    // --- DhcpState tests ---

    #[test]
    fn test_dhcp_state_is_bound() {
        assert!(DhcpState::Bound.is_bound());
        assert!(DhcpState::Renewing.is_bound());
        assert!(DhcpState::Rebinding.is_bound());
        assert!(!DhcpState::Init.is_bound());
        assert!(!DhcpState::Selecting.is_bound());
        assert!(!DhcpState::Requesting.is_bound());
    }

    #[test]
    fn test_dhcp_state_is_acquiring() {
        assert!(DhcpState::Init.is_acquiring());
        assert!(DhcpState::Selecting.is_acquiring());
        assert!(DhcpState::Requesting.is_acquiring());
        assert!(!DhcpState::Bound.is_acquiring());
        assert!(!DhcpState::Renewing.is_acquiring());
        assert!(!DhcpState::Rebinding.is_acquiring());
    }

    #[test]
    fn test_dhcp_state_is_init() {
        assert!(DhcpState::Init.is_init());
        assert!(!DhcpState::Bound.is_init());
        assert!(!DhcpState::Selecting.is_init());
    }

    #[test]
    fn test_dhcp_state_display() {
        assert_eq!(format!("{}", DhcpState::Init), "Init");
        assert_eq!(format!("{}", DhcpState::Selecting), "Selecting");
        assert_eq!(format!("{}", DhcpState::Requesting), "Requesting");
        assert_eq!(format!("{}", DhcpState::Bound), "Bound");
        assert_eq!(format!("{}", DhcpState::Renewing), "Renewing");
        assert_eq!(format!("{}", DhcpState::Rebinding), "Rebinding");
    }

    #[test]
    fn test_dhcp_state_all_variants() {
        let states = [
            DhcpState::Init,
            DhcpState::Selecting,
            DhcpState::Requesting,
            DhcpState::Bound,
            DhcpState::Renewing,
            DhcpState::Rebinding,
        ];
        assert_eq!(states.len(), 6);
    }

    #[test]
    fn test_dhcp_state_equality() {
        assert_eq!(DhcpState::Init, DhcpState::Init);
        assert_ne!(DhcpState::Init, DhcpState::Bound);
        assert_eq!(DhcpState::Bound, DhcpState::Bound);
    }

    // --- DhcpClient tests ---

    #[test]
    fn test_dhcp_client_new() {
        let client = DhcpClient::new();
        assert!(!client.is_started());
        assert!(!client.is_bound());
        assert_eq!(client.state(), DhcpState::Init);
        assert!(client.handle().is_none());
        assert!(client.lease_ref().is_none());
    }

    #[test]
    fn test_dhcp_client_default() {
        let client = DhcpClient::default();
        assert!(!client.is_started());
        assert_eq!(client.state(), DhcpState::Init);
    }

    #[test]
    fn test_dhcp_client_with_handle() {
        let handle = SocketHandle::default();
        let client = DhcpClient::with_handle(handle);
        assert!(client.is_started());
        assert_eq!(client.handle(), Some(handle));
        assert_eq!(client.state(), DhcpState::Init);
    }

    #[test]
    fn test_dhcp_client_from_iface_with_dhcp() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let iface = NetworkInterface::new(dev, config);

        let client = DhcpClient::from_iface(&iface);
        assert!(client.is_some());
        let client = client.unwrap();
        assert!(client.is_started());
    }

    #[test]
    fn test_dhcp_client_from_iface_without_dhcp() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);

        let client = DhcpClient::from_iface(&iface);
        assert!(client.is_none());
    }

    #[test]
    fn test_dhcp_client_start_with_existing_dhcp() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        assert!(!client.is_started());

        let result = client.start(&mut iface);
        assert!(result.is_ok());
        assert!(client.is_started());
        // Should reuse the existing DHCP socket handle
        assert_eq!(client.handle(), iface.dhcp_handle());
    }

    #[test]
    fn test_dhcp_client_start_without_existing_dhcp() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        assert!(!client.is_started());

        let result = client.start(&mut iface);
        assert!(result.is_ok());
        assert!(client.is_started());
        // A new DHCP socket should have been created
        assert!(client.handle().is_some());
    }

    #[test]
    fn test_dhcp_client_start_idempotent() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        client.start(&mut iface).unwrap();
        let handle1 = client.handle();

        // Starting again should be a no-op
        client.start(&mut iface).unwrap();
        let handle2 = client.handle();

        assert_eq!(handle1, handle2);
    }

    #[test]
    fn test_dhcp_client_poll_not_started() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        let result = client.poll(&mut iface);
        assert!(result.is_err());
    }

    #[test]
    fn test_dhcp_client_poll_started() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        client.start(&mut iface).unwrap();

        // Poll the interface first (processes packets)
        iface.poll(0).unwrap();

        // Poll the DHCP client (retrieves events)
        let result = client.poll(&mut iface);
        assert!(result.is_ok());
        // State should be Init (no DHCP server in mock environment)
        assert_eq!(result.unwrap(), DhcpState::Init);
    }

    #[test]
    fn test_dhcp_client_lease_not_started() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]);
        let iface = NetworkInterface::new(dev, config);

        let client = DhcpClient::new();
        assert!(client.lease(&iface).is_none());
        assert!(client.lease_ref().is_none());
    }

    #[test]
    fn test_dhcp_client_lease_started_no_binding() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        client.start(&mut iface).unwrap();
        iface.poll(0).unwrap();
        client.poll(&mut iface).unwrap();

        // No DHCP server in mock environment, so no lease
        assert!(client.lease(&iface).is_none());
    }

    #[test]
    fn test_dhcp_client_reset() {
        let dev = make_device();
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01]).with_dhcp(true);
        let mut iface = NetworkInterface::new(dev, config);

        let mut client = DhcpClient::new();
        client.start(&mut iface).unwrap();

        client.reset();
        assert_eq!(client.state(), DhcpState::Init);
        assert!(client.lease_ref().is_none());
        // Handle should be retained
        assert!(client.is_started());
    }

    #[test]
    fn test_dhcp_client_debug() {
        let client = DhcpClient::new();
        let debug_str = format!("{:?}", client);
        assert!(debug_str.contains("DhcpClient"));
        assert!(debug_str.contains("Init"));
    }

    #[test]
    fn test_dhcp_client_with_handle_debug() {
        let handle = SocketHandle::default();
        let client = DhcpClient::with_handle(handle);
        let debug_str = format!("{:?}", client);
        assert!(debug_str.contains("DhcpClient"));
    }

    #[test]
    fn test_dhcp_lease_has_gateway() {
        let lease = DhcpLease {
            addr: ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24),
            gateway: Some(ipv4_addr(192, 168, 1, 1)),
            dns_servers: Vec::new(),
            lease_duration: 3600,
            server_id: ipv4_addr(192, 168, 1, 1),
        };
        assert!(lease.has_gateway());

        let lease_no_gw = DhcpLease {
            addr: ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24),
            gateway: None,
            dns_servers: Vec::new(),
            lease_duration: 3600,
            server_id: ipv4_addr(192, 168, 1, 1),
        };
        assert!(!lease_no_gw.has_gateway());
    }

    #[test]
    fn test_dhcp_lease_dns_server_count() {
        let dns = vec![ipv4_addr(8, 8, 8, 8), ipv4_addr(8, 8, 4, 4)];

        let lease = DhcpLease {
            addr: ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24),
            gateway: Some(ipv4_addr(192, 168, 1, 1)),
            dns_servers: dns,
            lease_duration: 3600,
            server_id: ipv4_addr(192, 168, 1, 1),
        };
        assert_eq!(lease.dns_server_count(), 2);
    }

    #[test]
    fn test_dhcp_lease_debug() {
        let lease = DhcpLease {
            addr: ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24),
            gateway: Some(ipv4_addr(192, 168, 1, 1)),
            dns_servers: Vec::new(),
            lease_duration: 0,
            server_id: ipv4_addr(192, 168, 1, 1),
        };
        let debug_str = format!("{:?}", lease);
        assert!(debug_str.contains("DhcpLease"));
    }

    #[test]
    fn test_dhcp_lease_clone() {
        let lease = DhcpLease {
            addr: ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24),
            gateway: Some(ipv4_addr(192, 168, 1, 1)),
            dns_servers: Vec::new(),
            lease_duration: 3600,
            server_id: ipv4_addr(192, 168, 1, 1),
        };
        let cloned = lease.clone();
        assert_eq!(lease.addr, cloned.addr);
        assert_eq!(lease.gateway, cloned.gateway);
        assert_eq!(lease.lease_duration, cloned.lease_duration);
    }
}
