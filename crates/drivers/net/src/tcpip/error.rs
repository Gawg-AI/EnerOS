//! Error types for the TCP/IP protocol stack.
//!
//! Defines [`TcpIpError`] — a separate error enum from v0.27.0's `NetError`,
//! created to avoid modifying existing source files (Surgical Changes principle).
//!
//! # smoltcp 0.13 Error Model
//!
//! In smoltcp 0.13, the unified `smoltcp::Error` enum was removed. Errors are
//! now specific to each socket operation:
//! - `smoltcp::socket::tcp::{ListenError, ConnectError, SendError, RecvError}`
//! - `smoltcp::socket::udp::{SendError, RecvError}`
//! - `smoltcp::socket::icmp::{SendError, RecvError}`
//!
//! [`TcpIpError`] implements `From` for each of these specific types, providing
//! a single unified error for the wrapper layer.

use core::fmt;

use crate::error::NetError;

/// TCP/IP protocol stack error type (15 variants).
///
/// Covers all failure modes of the TCP/IP stack wrapper layer, including
/// device errors, routing failures, connection state errors, socket
/// operations, and DHCP failures.
#[derive(Debug, Clone, PartialEq)]
pub enum TcpIpError {
    /// DMA or hardware-level error from the underlying NetDevice.
    DmaError,
    /// No route to the destination (gateway unreachable or no matching route).
    NoRoute,
    /// ARP resolution failed (could not resolve the hardware address for the
    /// next-hop IP within the timeout period).
    ArpResolutionFailed,
    /// Connection was refused by the remote endpoint (RST received during
    /// the handshake).
    ConnectionRefused,
    /// Connection was reset by the remote endpoint (RST received after
    /// establishment).
    ConnectionReset,
    /// Socket is not connected (attempted send/recv on a non-established socket).
    NotConnected,
    /// Operation would block (non-blocking socket has no data ready).
    WouldBlock,
    /// Operation timed out (e.g. TCP connect timeout, DHCP lease timeout).
    TimedOut,
    /// Address is already in use (bind to a port that is already bound).
    AddrInUse,
    /// Address is not available (bind to an address the interface doesn't have).
    AddrNotAvailable,
    /// Invalid argument passed to a socket or interface method.
    InvalidArgument,
    /// DHCP lease acquisition or renewal failed.
    DhcpFailed,
    /// Socket handle not found in the SocketSet.
    SocketNotFound,
    /// Destination is unreachable (ICMP destination unreachable received or
    /// no matching route).
    Unreachable,
    /// Packet exceeds the maximum transmission unit.
    PacketTooLarge { size: usize, max: usize },
}

impl fmt::Display for TcpIpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpIpError::DmaError => write!(f, "DMA/hardware error"),
            TcpIpError::NoRoute => write!(f, "no route to destination"),
            TcpIpError::ArpResolutionFailed => write!(f, "ARP resolution failed"),
            TcpIpError::ConnectionRefused => write!(f, "connection refused"),
            TcpIpError::ConnectionReset => write!(f, "connection reset by peer"),
            TcpIpError::NotConnected => write!(f, "socket not connected"),
            TcpIpError::WouldBlock => write!(f, "operation would block"),
            TcpIpError::TimedOut => write!(f, "operation timed out"),
            TcpIpError::AddrInUse => write!(f, "address already in use"),
            TcpIpError::AddrNotAvailable => write!(f, "address not available"),
            TcpIpError::InvalidArgument => write!(f, "invalid argument"),
            TcpIpError::DhcpFailed => write!(f, "DHCP failed"),
            TcpIpError::SocketNotFound => write!(f, "socket not found"),
            TcpIpError::Unreachable => write!(f, "destination unreachable"),
            TcpIpError::PacketTooLarge { size, max } => {
                write!(f, "packet too large: {} bytes (max {})", size, max)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// From<smoltcp error types> for TcpIpError
// ---------------------------------------------------------------------------

// In smoltcp 0.13, the unified `smoltcp::Error` enum was removed. Errors are
// now specific to each socket operation. We implement From for each.

/// Convert `smoltcp::socket::tcp::ListenError` to `TcpIpError`.
impl From<smoltcp::socket::tcp::ListenError> for TcpIpError {
    fn from(e: smoltcp::socket::tcp::ListenError) -> Self {
        match e {
            smoltcp::socket::tcp::ListenError::InvalidState => TcpIpError::InvalidArgument,
            smoltcp::socket::tcp::ListenError::Unaddressable => TcpIpError::AddrNotAvailable,
        }
    }
}

/// Convert `smoltcp::socket::tcp::ConnectError` to `TcpIpError`.
impl From<smoltcp::socket::tcp::ConnectError> for TcpIpError {
    fn from(e: smoltcp::socket::tcp::ConnectError) -> Self {
        match e {
            smoltcp::socket::tcp::ConnectError::InvalidState => TcpIpError::InvalidArgument,
            smoltcp::socket::tcp::ConnectError::Unaddressable => TcpIpError::AddrNotAvailable,
        }
    }
}

/// Convert `smoltcp::socket::tcp::SendError` to `TcpIpError`.
impl From<smoltcp::socket::tcp::SendError> for TcpIpError {
    fn from(e: smoltcp::socket::tcp::SendError) -> Self {
        match e {
            smoltcp::socket::tcp::SendError::InvalidState => TcpIpError::NotConnected,
        }
    }
}

/// Convert `smoltcp::socket::tcp::RecvError` to `TcpIpError`.
impl From<smoltcp::socket::tcp::RecvError> for TcpIpError {
    fn from(e: smoltcp::socket::tcp::RecvError) -> Self {
        match e {
            smoltcp::socket::tcp::RecvError::InvalidState => TcpIpError::NotConnected,
            smoltcp::socket::tcp::RecvError::Finished => TcpIpError::NotConnected,
        }
    }
}

/// Convert `smoltcp::socket::udp::SendError` to `TcpIpError`.
impl From<smoltcp::socket::udp::SendError> for TcpIpError {
    fn from(_e: smoltcp::socket::udp::SendError) -> Self {
        TcpIpError::Unreachable
    }
}

/// Convert `smoltcp::socket::udp::RecvError` to `TcpIpError`.
impl From<smoltcp::socket::udp::RecvError> for TcpIpError {
    fn from(_e: smoltcp::socket::udp::RecvError) -> Self {
        TcpIpError::WouldBlock
    }
}

/// Convert `smoltcp::socket::icmp::SendError` to `TcpIpError`.
impl From<smoltcp::socket::icmp::SendError> for TcpIpError {
    fn from(_e: smoltcp::socket::icmp::SendError) -> Self {
        TcpIpError::Unreachable
    }
}

/// Convert `smoltcp::socket::icmp::RecvError` to `TcpIpError`.
impl From<smoltcp::socket::icmp::RecvError> for TcpIpError {
    fn from(_e: smoltcp::socket::icmp::RecvError) -> Self {
        TcpIpError::WouldBlock
    }
}

/// Convert `crate::error::NetError` to `TcpIpError`.
impl From<NetError> for TcpIpError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::NotInitialized => TcpIpError::InvalidArgument,
            NetError::LinkDown => TcpIpError::NoRoute,
            NetError::NoBuffer => TcpIpError::WouldBlock,
            NetError::DmaError(_) => TcpIpError::DmaError,
            NetError::FrameTooLarge { size, max } => TcpIpError::PacketTooLarge { size, max },
            NetError::FrameTooSmall => TcpIpError::InvalidArgument,
            NetError::CrcError => TcpIpError::DmaError,
            NetError::PhyError => TcpIpError::DmaError,
            NetError::Timeout => TcpIpError::TimedOut,
        }
    }
}

// ---------------------------------------------------------------------------
// From<TcpIpError> for NetError (unified error conversion)
// ---------------------------------------------------------------------------

/// Convert `TcpIpError` back to `NetError` for unified error handling.
///
/// This allows callers that work with `NetError` to also handle TCP/IP errors
/// without changing their error type. The mapping is lossy — some TCP/IP error
/// variants do not have a direct `NetError` equivalent and are mapped to the
/// closest match.
impl From<TcpIpError> for NetError {
    fn from(e: TcpIpError) -> Self {
        match e {
            TcpIpError::DmaError => NetError::DmaError(0),
            TcpIpError::NoRoute => NetError::NoBuffer,
            TcpIpError::ArpResolutionFailed => NetError::Timeout,
            TcpIpError::ConnectionRefused => NetError::LinkDown,
            TcpIpError::ConnectionReset => NetError::LinkDown,
            TcpIpError::NotConnected => NetError::LinkDown,
            TcpIpError::WouldBlock => NetError::NoBuffer,
            TcpIpError::TimedOut => NetError::Timeout,
            TcpIpError::AddrInUse => NetError::NoBuffer,
            TcpIpError::AddrNotAvailable => NetError::NoBuffer,
            TcpIpError::InvalidArgument => NetError::FrameTooSmall,
            TcpIpError::DhcpFailed => NetError::Timeout,
            TcpIpError::SocketNotFound => NetError::NoBuffer,
            TcpIpError::Unreachable => NetError::NoBuffer,
            TcpIpError::PacketTooLarge { size, max } => NetError::FrameTooLarge { size, max },
        }
    }
}

// ---------------------------------------------------------------------------
// is_retriable()
// ---------------------------------------------------------------------------

impl TcpIpError {
    /// Returns `true` if the error is transient and the operation can be retried.
    ///
    /// Retriable errors:
    /// - [`TcpIpError::WouldBlock`] — non-blocking socket has no data ready yet.
    /// - [`TcpIpError::TimedOut`] — operation timed out, may succeed on retry.
    /// - [`TcpIpError::ArpResolutionFailed`] — ARP cache may be populated on retry.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            TcpIpError::WouldBlock | TcpIpError::TimedOut | TcpIpError::ArpResolutionFailed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Display tests ---

    #[test]
    fn test_dma_error_display() {
        assert_eq!(format!("{}", TcpIpError::DmaError), "DMA/hardware error");
    }

    #[test]
    fn test_no_route_display() {
        assert_eq!(
            format!("{}", TcpIpError::NoRoute),
            "no route to destination"
        );
    }

    #[test]
    fn test_arp_resolution_failed_display() {
        assert_eq!(
            format!("{}", TcpIpError::ArpResolutionFailed),
            "ARP resolution failed"
        );
    }

    #[test]
    fn test_connection_refused_display() {
        assert_eq!(
            format!("{}", TcpIpError::ConnectionRefused),
            "connection refused"
        );
    }

    #[test]
    fn test_would_block_display() {
        assert_eq!(
            format!("{}", TcpIpError::WouldBlock),
            "operation would block"
        );
    }

    #[test]
    fn test_timed_out_display() {
        assert_eq!(format!("{}", TcpIpError::TimedOut), "operation timed out");
    }

    #[test]
    fn test_packet_too_large_display() {
        let err = TcpIpError::PacketTooLarge {
            size: 2000,
            max: 1500,
        };
        assert_eq!(
            format!("{}", err),
            "packet too large: 2000 bytes (max 1500)"
        );
    }

    #[test]
    fn test_dhcp_failed_display() {
        assert_eq!(format!("{}", TcpIpError::DhcpFailed), "DHCP failed");
    }

    // --- is_retriable tests ---

    #[test]
    fn test_would_block_is_retriable() {
        assert!(TcpIpError::WouldBlock.is_retriable());
    }

    #[test]
    fn test_timed_out_is_retriable() {
        assert!(TcpIpError::TimedOut.is_retriable());
    }

    #[test]
    fn test_arp_failed_is_retriable() {
        assert!(TcpIpError::ArpResolutionFailed.is_retriable());
    }

    #[test]
    fn test_connection_refused_not_retriable() {
        assert!(!TcpIpError::ConnectionRefused.is_retriable());
    }

    #[test]
    fn test_invalid_argument_not_retriable() {
        assert!(!TcpIpError::InvalidArgument.is_retriable());
    }

    // --- From<smoltcp error types> tests ---

    #[test]
    fn test_from_tcp_listen_error_invalid_state() {
        let err = TcpIpError::from(smoltcp::socket::tcp::ListenError::InvalidState);
        assert_eq!(err, TcpIpError::InvalidArgument);
    }

    #[test]
    fn test_from_tcp_listen_error_unaddressable() {
        let err = TcpIpError::from(smoltcp::socket::tcp::ListenError::Unaddressable);
        assert_eq!(err, TcpIpError::AddrNotAvailable);
    }

    #[test]
    fn test_from_tcp_send_error() {
        let err = TcpIpError::from(smoltcp::socket::tcp::SendError::InvalidState);
        assert_eq!(err, TcpIpError::NotConnected);
    }

    #[test]
    fn test_from_tcp_recv_error_invalid_state() {
        let err = TcpIpError::from(smoltcp::socket::tcp::RecvError::InvalidState);
        assert_eq!(err, TcpIpError::NotConnected);
    }

    #[test]
    fn test_from_tcp_recv_error_finished() {
        let err = TcpIpError::from(smoltcp::socket::tcp::RecvError::Finished);
        assert_eq!(err, TcpIpError::NotConnected);
    }

    // --- From<NetError> tests ---

    #[test]
    fn test_from_net_error_link_down() {
        let err = TcpIpError::from(NetError::LinkDown);
        assert_eq!(err, TcpIpError::NoRoute);
    }

    #[test]
    fn test_from_net_error_timeout() {
        let err = TcpIpError::from(NetError::Timeout);
        assert_eq!(err, TcpIpError::TimedOut);
    }

    #[test]
    fn test_from_net_error_no_buffer() {
        let err = TcpIpError::from(NetError::NoBuffer);
        assert_eq!(err, TcpIpError::WouldBlock);
    }

    #[test]
    fn test_from_net_error_frame_too_large() {
        let err = TcpIpError::from(NetError::FrameTooLarge {
            size: 2000,
            max: 1500,
        });
        assert_eq!(
            err,
            TcpIpError::PacketTooLarge {
                size: 2000,
                max: 1500
            }
        );
    }

    // --- From<TcpIpError> for NetError tests ---

    #[test]
    fn test_to_net_error_dma() {
        let net_err = NetError::from(TcpIpError::DmaError);
        assert_eq!(net_err, NetError::DmaError(0));
    }

    #[test]
    fn test_to_net_error_timeout() {
        let net_err = NetError::from(TcpIpError::TimedOut);
        assert_eq!(net_err, NetError::Timeout);
    }

    #[test]
    fn test_to_net_error_would_block() {
        let net_err = NetError::from(TcpIpError::WouldBlock);
        assert_eq!(net_err, NetError::NoBuffer);
    }

    #[test]
    fn test_to_net_error_packet_too_large() {
        let net_err = NetError::from(TcpIpError::PacketTooLarge {
            size: 2000,
            max: 1500,
        });
        assert_eq!(
            net_err,
            NetError::FrameTooLarge {
                size: 2000,
                max: 1500
            }
        );
    }

    #[test]
    fn test_to_net_error_connection_refused() {
        let net_err = NetError::from(TcpIpError::ConnectionRefused);
        assert_eq!(net_err, NetError::LinkDown);
    }

    // --- Error equality ---

    #[test]
    fn test_error_equality() {
        assert_eq!(TcpIpError::TimedOut, TcpIpError::TimedOut);
        assert_ne!(TcpIpError::TimedOut, TcpIpError::WouldBlock);
    }

    #[test]
    fn test_error_clone() {
        let err = TcpIpError::PacketTooLarge { size: 100, max: 50 };
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_all_variants_display() {
        // Ensure all variants have a Display implementation
        let variants = [
            TcpIpError::DmaError,
            TcpIpError::NoRoute,
            TcpIpError::ArpResolutionFailed,
            TcpIpError::ConnectionRefused,
            TcpIpError::ConnectionReset,
            TcpIpError::NotConnected,
            TcpIpError::WouldBlock,
            TcpIpError::TimedOut,
            TcpIpError::AddrInUse,
            TcpIpError::AddrNotAvailable,
            TcpIpError::InvalidArgument,
            TcpIpError::DhcpFailed,
            TcpIpError::SocketNotFound,
            TcpIpError::Unreachable,
            TcpIpError::PacketTooLarge { size: 1, max: 0 },
        ];
        for v in &variants {
            let s = format!("{}", v);
            assert!(!s.is_empty());
        }
        assert_eq!(variants.len(), 15);
    }
}
