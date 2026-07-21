//! Socket API types — `SocketId`, `SocketKind`, `SocketError`, `Socket` trait,
//! and zero-cost handle newtypes (`TcpStream` / `TcpListener` / `UdpSocket`).
//!
//! # Design
//!
//! The handle types are newtypes around [`SocketId`] (`usize`). They carry no
//! data beyond the integer id; all socket state lives inside
//! [`crate::socket::manager::SocketManager`]. This follows the "Simplicity
//! First" principle: no `Rc`/`RefCell`/`Box` is needed, and handles are `Copy`.
//!
//! # `Socket` trait deviation
//!
//! The [`Socket`] trait is defined for documentation and future extensions
//! (e.g. mock sockets in tests, alternative backends). It is **not** implemented
//! for the smoltcp backend because doing so would require shared ownership of
//! `NetworkInterface` (`Rc<RefCell<>>` or unsafe globals), violating
//! "Simplicity First". The [`SocketManager`](crate::socket::manager::SocketManager)
//! method API provides equivalent functionality.

use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::error::NetError;
use crate::tcpip::addr::SocketAddr;
use crate::tcpip::error::TcpIpError;

/// Opaque identifier for a socket managed by `SocketManager`.
///
/// Internally a `usize`; the `SocketManager` assigns monotonically increasing
/// ids starting at 0.
pub type SocketId = usize;

/// Kind of socket stored in a `SocketEntry`.
///
/// Distinguishes TCP streams (connected), TCP listeners (accepting), and UDP
/// sockets so that `SocketManager` can dispatch IO operations to the correct
/// smoltcp socket type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    /// Connected TCP socket (can read/write).
    TcpStream,
    /// Listening TCP socket (can accept).
    TcpListener,
    /// Bound UDP socket (can send_to/recv_from).
    Udp,
}

/// Unified socket-layer error (11 variants).
///
/// Maps errors from the v0.27.0 `NetError` and v0.28.0 `TcpIpError` enums into
/// a single type suitable for the socket abstraction layer. The `IoError`
/// variant carries a descriptive string for errors that do not have a direct
/// semantic match (e.g. DMA errors, ARP failures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketError {
    /// Socket is not connected (attempted read/write on a non-established socket).
    NotConnected,
    /// Connection was refused by the remote endpoint (RST during handshake).
    ConnectionRefused,
    /// Connection was reset by the remote endpoint (RST after establishment).
    ConnectionReset,
    /// Operation would block (non-blocking socket has no data ready).
    WouldBlock,
    /// Operation timed out (e.g. connect timeout, DHCP lease timeout).
    TimedOut,
    /// Broken pipe (write to a closed/shutdown socket).
    BrokenPipe,
    /// Address is already in use (bind to a port that is already bound).
    AddrInUse,
    /// Address is not available (bind to an address the interface doesn't have).
    AddrNotAvailable,
    /// Invalid argument passed to a socket method.
    InvalidArgument,
    /// Socket has been closed or does not exist.
    Closed,
    /// Generic I/O error with a descriptive message.
    IoError(String),
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocketError::NotConnected => write!(f, "socket not connected"),
            SocketError::ConnectionRefused => write!(f, "connection refused"),
            SocketError::ConnectionReset => write!(f, "connection reset by peer"),
            SocketError::WouldBlock => write!(f, "operation would block"),
            SocketError::TimedOut => write!(f, "operation timed out"),
            SocketError::BrokenPipe => write!(f, "broken pipe"),
            SocketError::AddrInUse => write!(f, "address already in use"),
            SocketError::AddrNotAvailable => write!(f, "address not available"),
            SocketError::InvalidArgument => write!(f, "invalid argument"),
            SocketError::Closed => write!(f, "socket closed"),
            SocketError::IoError(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

/// Convert `TcpIpError` (v0.28.0) to `SocketError`.
///
/// Direct semantic matches are preserved (e.g. `WouldBlock` → `WouldBlock`).
/// Errors without a direct match (DMA, ARP, route, DHCP, unreachable,
/// packet-too-large) are folded into [`SocketError::IoError`] with a
/// descriptive message.
impl From<TcpIpError> for SocketError {
    fn from(e: TcpIpError) -> Self {
        match e {
            TcpIpError::ConnectionRefused => SocketError::ConnectionRefused,
            TcpIpError::ConnectionReset => SocketError::ConnectionReset,
            TcpIpError::NotConnected => SocketError::NotConnected,
            TcpIpError::WouldBlock => SocketError::WouldBlock,
            TcpIpError::TimedOut => SocketError::TimedOut,
            TcpIpError::AddrInUse => SocketError::AddrInUse,
            TcpIpError::AddrNotAvailable => SocketError::AddrNotAvailable,
            TcpIpError::InvalidArgument => SocketError::InvalidArgument,
            TcpIpError::SocketNotFound => SocketError::Closed,
            TcpIpError::DmaError => SocketError::IoError(String::from("DMA/hardware error")),
            TcpIpError::NoRoute => SocketError::IoError(String::from("no route to destination")),
            TcpIpError::ArpResolutionFailed => {
                SocketError::IoError(String::from("ARP resolution failed"))
            }
            TcpIpError::DhcpFailed => SocketError::IoError(String::from("DHCP failed")),
            TcpIpError::Unreachable => {
                SocketError::IoError(String::from("destination unreachable"))
            }
            TcpIpError::PacketTooLarge { size, max } => {
                SocketError::IoError(format!("packet too large: {} bytes (max {})", size, max))
            }
        }
    }
}

/// Convert `NetError` (v0.27.0) to `SocketError`.
///
/// Maps hardware-level errors into socket-layer semantics. `NoBuffer` becomes
/// `WouldBlock` (RX ring empty / TX ring full), `Timeout` becomes `TimedOut`,
/// and the rest are folded into [`SocketError::IoError`] with context.
impl From<NetError> for SocketError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::NoBuffer => SocketError::WouldBlock,
            NetError::Timeout => SocketError::TimedOut,
            NetError::FrameTooSmall => SocketError::InvalidArgument,
            NetError::NotInitialized => {
                SocketError::IoError(String::from("network device not initialized"))
            }
            NetError::LinkDown => SocketError::IoError(String::from("link is down")),
            NetError::DmaError(status) => {
                SocketError::IoError(format!("DMA error (status=0x{:08x})", status))
            }
            NetError::FrameTooLarge { size, max } => {
                SocketError::IoError(format!("frame too large: {} bytes (max {})", size, max))
            }
            NetError::CrcError => SocketError::IoError(String::from("CRC check failed")),
            NetError::PhyError => SocketError::IoError(String::from("PHY error")),
        }
    }
}

/// Unified socket interface (trait definition only).
///
/// Defines the standard read/write/close/non-blocking/query API. **Not
/// implemented for the smoltcp backend** — see the [module-level deviation
/// note](../index.html#偏差声明). The
/// [`SocketManager`](crate::socket::manager::SocketManager) methods provide
/// equivalent functionality without requiring shared ownership of
/// `NetworkInterface`.
///
/// The trait is retained for:
/// - Mock testing (test-only `MockSocket` can implement `Socket`).
/// - Future backends (e.g. a non-smoltcp socket implementation).
pub trait Socket {
    /// Read data into `buf`, returning the number of bytes read.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SocketError>;
    /// Write data from `buf`, returning the number of bytes written.
    fn write(&mut self, buf: &[u8]) -> Result<usize, SocketError>;
    /// Close the socket gracefully.
    fn close(&mut self) -> Result<(), SocketError>;
    /// Enable or disable non-blocking mode.
    fn set_nonblocking(&mut self, nonblocking: bool);
    /// Returns `true` if the socket has data ready to read.
    fn is_readable(&self) -> bool;
    /// Returns `true` if the socket can accept data for writing.
    fn is_writable(&self) -> bool;
    /// Returns the local socket address.
    fn local_addr(&self) -> Result<SocketAddr, SocketError>;
    /// Returns the remote socket address.
    fn remote_addr(&self) -> Result<SocketAddr, SocketError>;
}

// ---------------------------------------------------------------------------
// Handle newtypes
// ---------------------------------------------------------------------------

/// Handle to a connected TCP stream.
///
/// A zero-cost newtype around [`SocketId`]. The actual socket state lives in
/// [`SocketManager`](crate::socket::manager::SocketManager); pass `stream.id()`
/// to `SocketManager` methods to perform IO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpStream(SocketId);

impl TcpStream {
    /// Create a new handle for the given socket id (crate-internal).
    pub(crate) fn new(id: SocketId) -> Self {
        Self(id)
    }
    /// Returns the underlying socket id.
    pub fn id(&self) -> SocketId {
        self.0
    }
}

/// Handle to a listening TCP socket.
///
/// A zero-cost newtype around [`SocketId`]. Use
/// [`SocketManager::tcp_accept`](crate::socket::manager::SocketManager::tcp_accept)
/// to accept incoming connections on a listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpListener(SocketId);

impl TcpListener {
    /// Create a new handle for the given socket id (crate-internal).
    pub(crate) fn new(id: SocketId) -> Self {
        Self(id)
    }
    /// Returns the underlying socket id.
    pub fn id(&self) -> SocketId {
        self.0
    }
}

/// Handle to a bound UDP socket.
///
/// A zero-cost newtype around [`SocketId`]. Use
/// [`SocketManager::send_to`](crate::socket::manager::SocketManager::send_to)
/// and
/// [`SocketManager::recv_from`](crate::socket::manager::SocketManager::recv_from)
/// for UDP IO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpSocket(SocketId);

impl UdpSocket {
    /// Create a new handle for the given socket id (crate-internal).
    pub(crate) fn new(id: SocketId) -> Self {
        Self(id)
    }
    /// Returns the underlying socket id.
    pub fn id(&self) -> SocketId {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SocketKind tests ---

    #[test]
    fn test_socket_kind_variants() {
        let kinds = [
            SocketKind::TcpStream,
            SocketKind::TcpListener,
            SocketKind::Udp,
        ];
        assert_eq!(kinds.len(), 3);
        assert_ne!(SocketKind::TcpStream, SocketKind::TcpListener);
        assert_ne!(SocketKind::TcpListener, SocketKind::Udp);
        assert_ne!(SocketKind::TcpStream, SocketKind::Udp);
    }

    #[test]
    fn test_socket_kind_copy_eq() {
        let k = SocketKind::TcpStream;
        let k2 = k; // Copy
        assert_eq!(k, k2);
    }

    // --- SocketError tests ---

    #[test]
    fn test_socket_error_equality() {
        assert_eq!(SocketError::WouldBlock, SocketError::WouldBlock);
        assert_ne!(SocketError::WouldBlock, SocketError::TimedOut);
        assert_eq!(
            SocketError::IoError(String::from("x")),
            SocketError::IoError(String::from("x"))
        );
        assert_ne!(
            SocketError::IoError(String::from("x")),
            SocketError::IoError(String::from("y"))
        );
    }

    #[test]
    fn test_socket_error_clone() {
        let err = SocketError::IoError(String::from("test"));
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_socket_error_display() {
        assert_eq!(
            format!("{}", SocketError::NotConnected),
            "socket not connected"
        );
        assert_eq!(
            format!("{}", SocketError::ConnectionRefused),
            "connection refused"
        );
        assert_eq!(
            format!("{}", SocketError::WouldBlock),
            "operation would block"
        );
        assert_eq!(format!("{}", SocketError::Closed), "socket closed");
        assert_eq!(
            format!("{}", SocketError::IoError(String::from("dma failed"))),
            "I/O error: dma failed"
        );
    }

    #[test]
    fn test_socket_error_all_variants_display() {
        let variants = [
            SocketError::NotConnected,
            SocketError::ConnectionRefused,
            SocketError::ConnectionReset,
            SocketError::WouldBlock,
            SocketError::TimedOut,
            SocketError::BrokenPipe,
            SocketError::AddrInUse,
            SocketError::AddrNotAvailable,
            SocketError::InvalidArgument,
            SocketError::Closed,
            SocketError::IoError(String::from("misc")),
        ];
        for v in &variants {
            let s = format!("{}", v);
            assert!(!s.is_empty());
        }
        assert_eq!(variants.len(), 11);
    }

    // --- From<TcpIpError> tests ---

    #[test]
    fn test_from_tcpip_error_would_block() {
        assert_eq!(
            SocketError::from(TcpIpError::WouldBlock),
            SocketError::WouldBlock
        );
    }

    #[test]
    fn test_from_tcpip_error_connection_refused() {
        assert_eq!(
            SocketError::from(TcpIpError::ConnectionRefused),
            SocketError::ConnectionRefused
        );
    }

    #[test]
    fn test_from_tcpip_error_timed_out() {
        assert_eq!(
            SocketError::from(TcpIpError::TimedOut),
            SocketError::TimedOut
        );
    }

    #[test]
    fn test_from_tcpip_error_addr_in_use() {
        assert_eq!(
            SocketError::from(TcpIpError::AddrInUse),
            SocketError::AddrInUse
        );
    }

    #[test]
    fn test_from_tcpip_error_socket_not_found() {
        assert_eq!(
            SocketError::from(TcpIpError::SocketNotFound),
            SocketError::Closed
        );
    }

    #[test]
    #[allow(clippy::disallowed_macros)]
    fn test_from_tcpip_error_dma_to_ioerror() {
        let err = SocketError::from(TcpIpError::DmaError);
        match err {
            SocketError::IoError(msg) => assert!(msg.contains("DMA")),
            _ => panic!("expected IoError for DmaError"),
        }
    }

    #[test]
    #[allow(clippy::disallowed_macros)]
    fn test_from_tcpip_error_packet_too_large() {
        let err = SocketError::from(TcpIpError::PacketTooLarge {
            size: 2000,
            max: 1500,
        });
        match err {
            SocketError::IoError(msg) => {
                assert!(msg.contains("2000"));
                assert!(msg.contains("1500"));
            }
            _ => panic!("expected IoError for PacketTooLarge"),
        }
    }

    // --- From<NetError> tests ---

    #[test]
    fn test_from_net_error_no_buffer() {
        assert_eq!(
            SocketError::from(NetError::NoBuffer),
            SocketError::WouldBlock
        );
    }

    #[test]
    fn test_from_net_error_timeout() {
        assert_eq!(SocketError::from(NetError::Timeout), SocketError::TimedOut);
    }

    #[test]
    fn test_from_net_error_frame_too_small() {
        assert_eq!(
            SocketError::from(NetError::FrameTooSmall),
            SocketError::InvalidArgument
        );
    }

    #[test]
    #[allow(clippy::disallowed_macros)]
    fn test_from_net_error_link_down_to_ioerror() {
        let err = SocketError::from(NetError::LinkDown);
        match err {
            SocketError::IoError(msg) => assert!(msg.contains("link")),
            _ => panic!("expected IoError for LinkDown"),
        }
    }

    #[test]
    #[allow(clippy::disallowed_macros)]
    fn test_from_net_error_dma_to_ioerror() {
        let err = SocketError::from(NetError::DmaError(0x42));
        match err {
            SocketError::IoError(msg) => assert!(msg.contains("0x00000042")),
            _ => panic!("expected IoError for DmaError"),
        }
    }

    // --- Handle newtype tests ---

    #[test]
    fn test_tcp_stream_new_and_id() {
        let s = TcpStream::new(7);
        assert_eq!(s.id(), 7);
    }

    #[test]
    fn test_tcp_listener_new_and_id() {
        let l = TcpListener::new(3);
        assert_eq!(l.id(), 3);
    }

    #[test]
    fn test_udp_socket_new_and_id() {
        let u = UdpSocket::new(11);
        assert_eq!(u.id(), 11);
    }

    #[test]
    fn test_handle_copy_and_eq() {
        let s1 = TcpStream::new(5);
        let s2 = s1; // Copy
        assert_eq!(s1, s2);
        let s3 = TcpStream::new(5);
        assert_eq!(s1, s3);
        let s4 = TcpStream::new(6);
        assert_ne!(s1, s4);
    }

    #[test]
    fn test_handle_zero_id() {
        let s = TcpStream::new(0);
        assert_eq!(s.id(), 0);
    }
}
