//! Socket wrappers for TCP, UDP, and ICMP.
//!
//! Provides [`SocketSet`] — a convenience wrapper around
//! `smoltcp::iface::SocketSet` — and handle-based wrappers for TCP, UDP, and
//! ICMP sockets. The wrappers hold a [`SocketHandle`] and operate on the
//! socket stored inside a [`NetworkInterface`]'s socket set.
//!
//! # Design
//!
//! In smoltcp, sockets are stored in a `SocketSet` and accessed via handles.
//! This module follows the same pattern: [`SocketSet`] provides convenience
//! methods for creating sockets, and [`TcpSocket`]/[`UdpSocket`]/[`IcmpSocket`]
//! are lightweight handle-based wrappers that delegate to the underlying
//! smoltcp socket via `&mut NetworkInterface`.
//!
//! # Usage
//!
//! ```ignore
//! use eneros_net::{NetworkInterface, InterfaceConfig, TcpSocket};
//!
//! let mut iface = NetworkInterface::new(device, config);
//! let handle = iface.sockets.add_tcp(1024, 1024);
//! let mut tcp = TcpSocket::new(handle);
//! tcp.listen(&mut iface, 80).expect("listen failed");
//! ```

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::SocketSet as SmolcpSocketSet;
use smoltcp::socket::{icmp, tcp, udp};
use smoltcp::storage::RingBuffer;
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::mac::NetDevice;
use crate::tcpip::addr::{Ipv4Addr, SocketAddr, SocketHandle};
use crate::tcpip::error::TcpIpError;
use crate::tcpip::interface::NetworkInterface;

// ---------------------------------------------------------------------------
// TcpState
// ---------------------------------------------------------------------------

/// TCP socket connection state (maps `smoltcp::socket::tcp::State`).
///
/// Represents the 11 states of the TCP state machine as defined in RFC 793.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    /// Closed — the socket is not in use.
    Closed,
    /// Listening for incoming connections.
    Listen,
    /// SYN sent — actively waiting for a connection to be established.
    SynSent,
    /// SYN received — received a SYN and sent a SYN-ACK.
    SynReceived,
    /// Established — the connection is ready for data transfer.
    Established,
    /// FIN-WAIT-1 — sent a FIN, waiting for an ACK or FIN.
    FinWait1,
    /// FIN-WAIT-2 — received an ACK for the FIN, waiting for a FIN.
    FinWait2,
    /// CLOSE-WAIT — received a FIN, waiting for the application to close.
    CloseWait,
    /// CLOSING — sent a FIN and received a FIN, waiting for an ACK.
    Closing,
    /// LAST-ACK — sent a FIN in response to a FIN, waiting for an ACK.
    LastAck,
    /// TIME-WAIT — waiting for 2*MSL to ensure the remote received the ACK.
    TimeWait,
}

impl From<tcp::State> for TcpState {
    fn from(state: tcp::State) -> Self {
        match state {
            tcp::State::Closed => TcpState::Closed,
            tcp::State::Listen => TcpState::Listen,
            tcp::State::SynSent => TcpState::SynSent,
            tcp::State::SynReceived => TcpState::SynReceived,
            tcp::State::Established => TcpState::Established,
            tcp::State::FinWait1 => TcpState::FinWait1,
            tcp::State::FinWait2 => TcpState::FinWait2,
            tcp::State::CloseWait => TcpState::CloseWait,
            tcp::State::Closing => TcpState::Closing,
            tcp::State::LastAck => TcpState::LastAck,
            tcp::State::TimeWait => TcpState::TimeWait,
        }
    }
}

impl TcpState {
    /// Returns `true` if the socket is in a state where data can be sent
    /// and received (i.e., `Established`).
    pub fn is_connected(self) -> bool {
        matches!(self, TcpState::Established)
    }

    /// Returns `true` if the socket is in a state where it can accept new
    /// connections (i.e., `Listen`).
    pub fn is_listening(self) -> bool {
        matches!(self, TcpState::Listen)
    }

    /// Returns `true` if the socket is closed or in the process of closing.
    pub fn is_closed(self) -> bool {
        matches!(
            self,
            TcpState::Closed | TcpState::TimeWait | TcpState::CloseWait | TcpState::LastAck
        )
    }
}

// ---------------------------------------------------------------------------
// SocketSet
// ---------------------------------------------------------------------------

/// A set of sockets (wraps `smoltcp::iface::SocketSet<'static>`).
///
/// Provides convenience methods for creating TCP, UDP, and ICMP sockets with
/// default buffer sizes. The sockets are owned by the set and accessed via
/// [`SocketHandle`]s.
///
/// In typical usage, a `SocketSet` is owned by a [`NetworkInterface`] and
/// accessed via `iface.sockets`. The `inner` field is public to allow
/// smoltcp to access the raw socket set during `poll()`.
pub struct SocketSet {
    /// The underlying smoltcp socket set.
    pub inner: SmolcpSocketSet<'static>,
}

impl SocketSet {
    /// Create a new empty socket set.
    pub fn new() -> Self {
        Self {
            inner: SmolcpSocketSet::new(Vec::new()),
        }
    }

    /// Create and add a TCP socket with the given RX and TX buffer sizes.
    ///
    /// Returns the handle for accessing the socket.
    pub fn add_tcp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle {
        let rx_buffer = RingBuffer::new(vec![0u8; rx_size]);
        let tx_buffer = RingBuffer::new(vec![0u8; tx_size]);
        let socket = tcp::Socket::new(rx_buffer, tx_buffer);
        self.inner.add(socket)
    }

    /// Create and add a UDP socket with the given RX and TX buffer sizes.
    ///
    /// Each buffer can hold up to 4 packets simultaneously.
    pub fn add_udp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle {
        let rx_meta: Vec<udp::PacketMetadata> = vec![udp::PacketMetadata::EMPTY; 4];
        let tx_meta: Vec<udp::PacketMetadata> = vec![udp::PacketMetadata::EMPTY; 4];
        let rx_buffer = udp::PacketBuffer::new(rx_meta, vec![0u8; rx_size]);
        let tx_buffer = udp::PacketBuffer::new(tx_meta, vec![0u8; tx_size]);
        let socket = udp::Socket::new(rx_buffer, tx_buffer);
        self.inner.add(socket)
    }

    /// Create and add an ICMP socket with the given RX and TX buffer sizes.
    ///
    /// ICMP sockets use separate RX and TX buffers, each holding up to 4
    /// packets.
    pub fn add_icmp(&mut self, rx_size: usize, tx_size: usize) -> SocketHandle {
        let rx_meta: Vec<icmp::PacketMetadata> = vec![icmp::PacketMetadata::EMPTY; 4];
        let tx_meta: Vec<icmp::PacketMetadata> = vec![icmp::PacketMetadata::EMPTY; 4];
        let rx_buffer = icmp::PacketBuffer::new(rx_meta, vec![0u8; rx_size]);
        let tx_buffer = icmp::PacketBuffer::new(tx_meta, vec![0u8; tx_size]);
        let socket = icmp::Socket::new(rx_buffer, tx_buffer);
        self.inner.add(socket)
    }

    /// Remove a socket from the set.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not belong to this socket set.
    pub fn remove(&mut self, handle: SocketHandle) {
        self.inner.remove(handle);
    }

    /// Returns the number of sockets in the set.
    pub fn len(&self) -> usize {
        self.inner.iter().count()
    }

    /// Returns `true` if the set contains no sockets.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SocketSet {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for SocketSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SocketSet")
            .field("len", &self.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// TcpSocket
// ---------------------------------------------------------------------------

/// TCP socket wrapper (handle-based).
///
/// Holds a [`SocketHandle`] that identifies the socket within a
/// [`NetworkInterface`]'s socket set. All operations require a `&mut
/// NetworkInterface` to access the underlying smoltcp socket.
///
/// # Design Deviation
///
/// The task spec defined `TcpSocket::new(rx_buffer: Vec<u8>, tx_buffer: Vec<u8>)`.
/// This was changed to `TcpSocket::new(handle: SocketHandle)` because smoltcp
/// sockets must be stored in a `SocketSet` (not owned by the wrapper). The
/// `SocketSet::add_tcp(rx_size, tx_size)` method creates the socket with the
/// specified buffer sizes and returns a handle.
pub struct TcpSocket {
    handle: SocketHandle,
}

impl TcpSocket {
    /// Create a new TCP socket wrapper for the given handle.
    pub fn new(handle: SocketHandle) -> Self {
        Self { handle }
    }

    /// Returns the socket handle.
    pub fn handle(&self) -> SocketHandle {
        self.handle
    }

    /// Listen for incoming connections on the given port.
    pub fn listen<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        port: u16,
    ) -> Result<(), TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        socket.listen(port)?;
        Ok(())
    }

    /// Connect to a remote endpoint.
    ///
    /// The local address is chosen automatically by the stack, but the local
    /// port must be specified by the caller (smoltcp 0.13 requires a non-zero
    /// local port).
    ///
    /// # Design Deviation
    ///
    /// The task spec defined `connect(iface, remote)` without a local port.
    /// This was changed to `connect(iface, remote, local_port)` because
    /// smoltcp 0.13's `tcp::Socket::connect()` rejects port 0 (unlike older
    /// versions where `None` meant "any port").
    pub fn connect<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        remote: SocketAddr,
        local_port: u16,
    ) -> Result<(), TcpIpError> {
        let cx = iface.iface.context();
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        socket.connect(cx, remote, local_port)?;
        Ok(())
    }

    /// Send data on the socket.
    ///
    /// Returns the number of bytes actually queued in the TX buffer (may be
    /// less than `data.len()` if the buffer is full).
    pub fn send<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        data: &[u8],
    ) -> Result<usize, TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        Ok(socket.send_slice(data)?)
    }

    /// Receive data from the socket.
    ///
    /// Returns the number of bytes actually written into `buf`.
    pub fn recv<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        buf: &mut [u8],
    ) -> Result<usize, TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        Ok(socket.recv_slice(buf)?)
    }

    /// Close the socket gracefully (sends FIN).
    pub fn close<D: NetDevice>(&mut self, iface: &mut NetworkInterface<D>) {
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        socket.close();
    }

    /// Abort the connection immediately (sends RST).
    pub fn abort<D: NetDevice>(&mut self, iface: &mut NetworkInterface<D>) {
        let socket = iface.sockets.inner.get_mut::<tcp::Socket>(self.handle);
        socket.abort();
    }

    /// Returns the current TCP state of the socket.
    pub fn state<D: NetDevice>(&self, iface: &NetworkInterface<D>) -> TcpState {
        let socket = iface.sockets.inner.get::<tcp::Socket>(self.handle);
        socket.state().into()
    }

    /// Returns the local endpoint, or `None` if not bound.
    pub fn local_endpoint<D: NetDevice>(&self, iface: &NetworkInterface<D>) -> Option<IpEndpoint> {
        let socket = iface.sockets.inner.get::<tcp::Socket>(self.handle);
        socket.local_endpoint()
    }

    /// Returns the remote endpoint, or `None` if not connected.
    pub fn remote_endpoint<D: NetDevice>(&self, iface: &NetworkInterface<D>) -> Option<IpEndpoint> {
        let socket = iface.sockets.inner.get::<tcp::Socket>(self.handle);
        socket.remote_endpoint()
    }
}

impl core::fmt::Debug for TcpSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TcpSocket")
            .field("handle", &self.handle)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// UdpSocket
// ---------------------------------------------------------------------------

/// UDP socket wrapper (handle-based).
///
/// Holds a [`SocketHandle`] that identifies the socket within a
/// [`NetworkInterface`]'s socket set.
pub struct UdpSocket {
    handle: SocketHandle,
}

impl UdpSocket {
    /// Create a new UDP socket wrapper for the given handle.
    pub fn new(handle: SocketHandle) -> Self {
        Self { handle }
    }

    /// Returns the socket handle.
    pub fn handle(&self) -> SocketHandle {
        self.handle
    }

    /// Bind the socket to a local port.
    pub fn bind<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        port: u16,
    ) -> Result<(), TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<udp::Socket>(self.handle);
        socket.bind(port).map_err(|_| TcpIpError::AddrInUse)?;
        Ok(())
    }

    /// Send data to the specified destination.
    pub fn send_to<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        data: &[u8],
        dst: SocketAddr,
    ) -> Result<usize, TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<udp::Socket>(self.handle);
        socket.send_slice(data, dst)?;
        Ok(data.len())
    }

    /// Receive data from the socket.
    ///
    /// Returns the number of bytes received and the source endpoint.
    pub fn recv_from<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        buf: &mut [u8],
    ) -> Result<(usize, SocketAddr), TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<udp::Socket>(self.handle);
        let (len, meta) = socket.recv_slice(buf)?;
        Ok((len, meta.endpoint))
    }

    /// Returns the local endpoint the socket is bound to.
    pub fn endpoint<D: NetDevice>(
        &self,
        iface: &NetworkInterface<D>,
    ) -> Option<smoltcp::wire::IpListenEndpoint> {
        let socket = iface.sockets.inner.get::<udp::Socket>(self.handle);
        Some(socket.endpoint())
    }
}

impl core::fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UdpSocket")
            .field("handle", &self.handle)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// IcmpSocket
// ---------------------------------------------------------------------------

/// ICMP socket wrapper (handle-based).
///
/// Used for sending ICMP Echo Request ("ping") packets and receiving Echo
/// Reply ("pong") packets.
pub struct IcmpSocket {
    handle: SocketHandle,
}

impl IcmpSocket {
    /// Create a new ICMP socket wrapper for the given handle.
    pub fn new(handle: SocketHandle) -> Self {
        Self { handle }
    }

    /// Returns the socket handle.
    pub fn handle(&self) -> SocketHandle {
        self.handle
    }

    /// Send an ICMP Echo Request ("ping") to the specified destination.
    ///
    /// Constructs a minimal ICMP Echo Request packet with the given sequence
    /// number and identifier=1.
    pub fn send_ping<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
        dst: Ipv4Addr,
        seq: u16,
    ) -> Result<(), TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<icmp::Socket>(self.handle);

        // Construct ICMP Echo Request packet (8 bytes, no payload data).
        let mut packet = [0u8; 8];
        packet[0] = 8; // Type: Echo Request
        packet[1] = 0; // Code: 0
                       // packet[2..4] = checksum (computed below)
        packet[4] = 0; // Identifier high byte
        packet[5] = 1; // Identifier low byte
        packet[6..8].copy_from_slice(&seq.to_be_bytes()); // Sequence Number

        // Compute ICMP checksum (one's complement of the one's complement sum).
        let cksum = icmp_checksum(&packet);
        packet[2] = (cksum >> 8) as u8;
        packet[3] = (cksum & 0xFF) as u8;

        socket.send_slice(&packet, IpAddress::Ipv4(dst))?;
        Ok(())
    }

    /// Receive an ICMP Echo Reply ("pong").
    ///
    /// Returns the source IPv4 address and the sequence number from the reply.
    pub fn recv_pong<D: NetDevice>(
        &mut self,
        iface: &mut NetworkInterface<D>,
    ) -> Result<(Ipv4Addr, u16), TcpIpError> {
        let socket = iface.sockets.inner.get_mut::<icmp::Socket>(self.handle);
        let (data, src) = socket.recv()?;

        if data.len() < 8 {
            return Err(TcpIpError::InvalidArgument);
        }

        // Check that it's an Echo Reply (type 0).
        if data[0] != 0 {
            return Err(TcpIpError::InvalidArgument);
        }

        let seq = u16::from_be_bytes([data[6], data[7]]);
        let src_ipv4 = match src {
            IpAddress::Ipv4(addr) => addr,
            #[allow(unreachable_patterns)]
            _ => return Err(TcpIpError::InvalidArgument),
        };

        Ok((src_ipv4, seq))
    }
}

impl core::fmt::Debug for IcmpSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IcmpSocket")
            .field("handle", &self.handle)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ICMP checksum
// ---------------------------------------------------------------------------

/// Compute the ICMP checksum (RFC 1071) for the given data.
///
/// The checksum is the one's complement of the one's complement sum of all
/// 16-bit words in the data. If the data has an odd length, the last byte
/// is padded with a zero byte for the computation.
fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        let word = ((data[i] as u32) << 8) | (data[i + 1] as u32);
        sum += word;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};
    use crate::tcpip::interface::InterfaceConfig;

    /// Minimal mock device for socket tests.
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

    fn make_iface() -> NetworkInterface<MockNetDevice> {
        let dev = MockNetDevice {
            mac_addr: [0x02, 0, 0, 0, 0, 0x01],
            mtu: 1500,
            link_up: true,
        };
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
            .with_gateway(ipv4_addr(192, 168, 1, 1));
        NetworkInterface::new(dev, config)
    }

    // --- TcpState tests ---

    #[test]
    fn test_tcp_state_from_smoltcp_closed() {
        let state = TcpState::from(tcp::State::Closed);
        assert_eq!(state, TcpState::Closed);
    }

    #[test]
    fn test_tcp_state_from_smoltcp_listen() {
        let state = TcpState::from(tcp::State::Listen);
        assert_eq!(state, TcpState::Listen);
    }

    #[test]
    fn test_tcp_state_from_smoltcp_established() {
        let state = TcpState::from(tcp::State::Established);
        assert_eq!(state, TcpState::Established);
    }

    #[test]
    fn test_tcp_state_is_connected() {
        assert!(TcpState::Established.is_connected());
        assert!(!TcpState::Closed.is_connected());
        assert!(!TcpState::Listen.is_connected());
    }

    #[test]
    fn test_tcp_state_is_listening() {
        assert!(TcpState::Listen.is_listening());
        assert!(!TcpState::Established.is_listening());
    }

    #[test]
    fn test_tcp_state_is_closed() {
        assert!(TcpState::Closed.is_closed());
        assert!(TcpState::TimeWait.is_closed());
        assert!(TcpState::CloseWait.is_closed());
        assert!(!TcpState::Established.is_closed());
        assert!(!TcpState::Listen.is_closed());
    }

    #[test]
    fn test_tcp_state_all_variants() {
        let states = [
            TcpState::Closed,
            TcpState::Listen,
            TcpState::SynSent,
            TcpState::SynReceived,
            TcpState::Established,
            TcpState::FinWait1,
            TcpState::FinWait2,
            TcpState::CloseWait,
            TcpState::Closing,
            TcpState::LastAck,
            TcpState::TimeWait,
        ];
        assert_eq!(states.len(), 11);
    }

    // --- SocketSet tests ---

    #[test]
    fn test_socket_set_new() {
        let set = SocketSet::new();
        assert_eq!(set.len(), 0);
        assert!(set.is_empty());
    }

    #[test]
    fn test_socket_set_default() {
        let set = SocketSet::default();
        assert!(set.is_empty());
    }

    #[test]
    fn test_socket_set_add_tcp() {
        let mut set = SocketSet::new();
        let handle = set.add_tcp(1024, 1024);
        assert_eq!(set.len(), 1);
        assert!(!set.is_empty());
        // The handle should be valid (0 for the first socket)
        let _ = handle;
    }

    #[test]
    fn test_socket_set_add_udp() {
        let mut set = SocketSet::new();
        let handle = set.add_udp(1024, 1024);
        assert_eq!(set.len(), 1);
        let _ = handle;
    }

    #[test]
    fn test_socket_set_add_icmp() {
        let mut set = SocketSet::new();
        let handle = set.add_icmp(1024, 1024);
        assert_eq!(set.len(), 1);
        let _ = handle;
    }

    #[test]
    fn test_socket_set_add_multiple() {
        let mut set = SocketSet::new();
        let _ = set.add_tcp(1024, 1024);
        let _ = set.add_udp(512, 512);
        let _ = set.add_icmp(256, 256);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn test_socket_set_remove() {
        let mut set = SocketSet::new();
        let handle = set.add_tcp(1024, 1024);
        assert_eq!(set.len(), 1);
        set.remove(handle);
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn test_socket_set_debug() {
        let set = SocketSet::new();
        let debug_str = format!("{:?}", set);
        assert!(debug_str.contains("SocketSet"));
    }

    // --- TcpSocket tests ---

    #[test]
    fn test_tcp_socket_new() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let tcp = TcpSocket::new(handle);
        assert_eq!(tcp.handle(), handle);
    }

    #[test]
    fn test_tcp_socket_state_closed() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let tcp = TcpSocket::new(handle);
        assert_eq!(tcp.state(&iface), TcpState::Closed);
    }

    #[test]
    fn test_tcp_socket_listen() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let mut tcp = TcpSocket::new(handle);
        let result = tcp.listen(&mut iface, 80);
        assert!(result.is_ok());
        assert_eq!(tcp.state(&iface), TcpState::Listen);
    }

    #[test]
    fn test_tcp_socket_close() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let mut tcp = TcpSocket::new(handle);
        tcp.listen(&mut iface, 80).unwrap();
        tcp.close(&mut iface);
    }

    #[test]
    fn test_tcp_socket_abort() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let mut tcp = TcpSocket::new(handle);
        tcp.abort(&mut iface);
        assert_eq!(tcp.state(&iface), TcpState::Closed);
    }

    #[test]
    fn test_tcp_socket_debug() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let tcp = TcpSocket::new(handle);
        let debug_str = format!("{:?}", tcp);
        assert!(debug_str.contains("TcpSocket"));
    }

    #[test]
    fn test_tcp_socket_local_endpoint_none() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let tcp = TcpSocket::new(handle);
        assert!(tcp.local_endpoint(&iface).is_none());
    }

    #[test]
    fn test_tcp_socket_remote_endpoint_none() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_tcp(1024, 1024);
        let tcp = TcpSocket::new(handle);
        assert!(tcp.remote_endpoint(&iface).is_none());
    }

    // --- UdpSocket tests ---

    #[test]
    fn test_udp_socket_new() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_udp(1024, 1024);
        let udp = UdpSocket::new(handle);
        assert_eq!(udp.handle(), handle);
    }

    #[test]
    fn test_udp_socket_bind() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_udp(1024, 1024);
        let mut udp = UdpSocket::new(handle);
        let result = udp.bind(&mut iface, 8080);
        assert!(result.is_ok());
    }

    #[test]
    fn test_udp_socket_debug() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_udp(1024, 1024);
        let udp = UdpSocket::new(handle);
        let debug_str = format!("{:?}", udp);
        assert!(debug_str.contains("UdpSocket"));
    }

    // --- IcmpSocket tests ---

    #[test]
    fn test_icmp_socket_new() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_icmp(1024, 1024);
        let icmp = IcmpSocket::new(handle);
        assert_eq!(icmp.handle(), handle);
    }

    #[test]
    fn test_icmp_socket_debug() {
        let mut iface = make_iface();
        let handle = iface.sockets.add_icmp(1024, 1024);
        let icmp = IcmpSocket::new(handle);
        let debug_str = format!("{:?}", icmp);
        assert!(debug_str.contains("IcmpSocket"));
    }

    // --- icmp_checksum tests ---

    #[test]
    fn test_icmp_checksum_empty() {
        let cksum = icmp_checksum(&[]);
        assert_eq!(cksum, 0xFFFF);
    }

    #[test]
    fn test_icmp_checksum_known_value() {
        // ICMP Echo Request with seq=1, ident=1
        let mut packet = [0u8; 8];
        packet[0] = 8; // Type
        packet[1] = 0; // Code
        packet[4] = 0;
        packet[5] = 1; // Identifier
        packet[6] = 0;
        packet[7] = 1; // Sequence
        let cksum = icmp_checksum(&packet);
        // Verify by checking that the checksum + data sums to 0xFFFF
        packet[2] = (cksum >> 8) as u8;
        packet[3] = (cksum & 0xFF) as u8;
        let verify = icmp_checksum(&packet);
        assert_eq!(verify, 0, "checksum verification failed");
    }

    #[test]
    fn test_icmp_checksum_odd_length() {
        let data = [0x01, 0x02, 0x03];
        let _cksum = icmp_checksum(&data);
        // Should not panic
    }
}
