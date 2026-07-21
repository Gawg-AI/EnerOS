//! TCP/IP protocol stack integration (v0.28.0).
//!
//! This module integrates [smoltcp] v0.13 — a `no_std` TCP/IP stack — as the
//! default network protocol layer for EnerOS, per Blueprint §5.5 (Default
//! Integration List) and §3644 ("固定 smoltcp，禁止自研").
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  Caller (Agent Runtime / application)        │
//! └─────────────┬────────────────────────────────┘
//!               │  TcpSocket / UdpSocket / IcmpSocket / DhcpClient
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-net::tcpip (this module)             │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  NetworkInterface (wraps smoltcp::iface)│  │
//! │  │  SocketSet (wraps smoltcp::iface)       │  │
//! │  │  SmolcpDevice (adapts NetDevice → phy)  │  │
//! │  └────────────────────────────────────────┘  │
//! └─────────────┬────────────────────────────────┘
//!               │  smoltcp::phy::Device trait (RxToken / TxToken)
//! ┌─────────────▼────────────────────────────────┐
//! │  smoltcp v0.13 (protocol stack, 0BSD)        │
//! └─────────────┬────────────────────────────────┘
//!               │  NetDevice trait (send / recv)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-net::MacController (v0.27.0 driver)  │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_net::{InterfaceConfig, NetworkInterface, TcpSocket};
//!
//! // 1. Create a network interface with static IP
//! let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
//!     .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
//!     .with_gateway(ipv4_addr(192, 168, 1, 1));
//! let mut iface = NetworkInterface::new(device, config);
//!
//! // 2. Create a TCP socket
//! let tcp_handle = iface.sockets.add_tcp(65535, 65535);
//! let mut tcp = TcpSocket::new(tcp_handle);
//! tcp.listen(&mut iface, 80).expect("listen failed");
//!
//! // 3. Poll the interface
//! iface.poll(0).expect("poll failed");
//! ```
//!
//! # Design Decisions
//!
//! - **smoltcp integration**: We wrap smoltcp types rather than reimplementing
//!   protocol logic. This follows the "Simplicity First" principle and
//!   Blueprint §5.5 (禁止自研 TCP/IP 栈).
//! - **SmolcpDevice adapter**: Bridges the copy-based [`crate::NetDevice`]
//!   trait (send/recv byte slices) to smoltcp's zero-copy token-based
//!   [`smoltcp::phy::Device`] trait via an internal `VecDeque<Vec<u8>>` RX
//!   queue.
//! - **TcpIpError**: A separate error enum (not extending v0.27.0's `NetError`)
//!   to avoid modifying existing source files (Surgical Changes principle).
//! - **no_std**: All modules are `no_std` with `alloc`. smoltcp itself is
//!   `#![no_std]` with the `alloc` feature enabled.

pub mod addr;
pub mod device;
pub mod dhcp;
pub mod error;
pub mod interface;
pub mod socket;

// Re-export key types for convenience.
pub use addr::{
    ipv4_addr, ipv4_cidr, HardwareAddress, Ipv4Addr, Ipv4Cidr, SocketAddr, SocketHandle,
};
pub use device::SmolcpDevice;
pub use dhcp::{DhcpClient, DhcpLease, DhcpState};
pub use error::TcpIpError;
pub use interface::{InterfaceConfig, NetworkInterface};
pub use socket::{IcmpSocket, SocketSet, TcpSocket, TcpState, UdpSocket};
