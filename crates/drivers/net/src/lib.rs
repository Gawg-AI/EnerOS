//! EnerOS Ethernet Network Driver (v0.27.0)
//!
//! Raw Ethernet MAC driver for the EnerOS Edge Box. Provides frame TX/RX
//! via DMA descriptor rings, PHY autonegotiation via MII management, and
//! a [`NetDevice`] trait abstraction for the v0.28.0 TCP/IP protocol stack.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  Caller (v0.28.0 TCP/IP stack)               │
//! └─────────────┬────────────────────────────────┘
//!               │  NetDevice trait (send/recv/mac_address/mtu/link_up)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-net::MacController (this crate)      │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  DMA Ring (TX + RX descriptor rings)   │  │
//! │  │  PHY Driver (GenericPhy via MII)       │  │
//! │  │  Frame Buffers (Vec<Vec<u8>>)          │  │
//! │  └────────────────────────────────────────┘  │
//! └─────────────┬────────────────────────────────┘
//!               │  MacRegs trait (read/write register offsets)
//! ┌─────────────▼────────────────────────────────┐
//! │  MmioMacRegs (real hardware, aarch64)        │
//! │  MockMacRegs (testing, BTreeMap-backed)      │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_net::{MacController, NetDevice, MmioMacRegs};
//!
//! // Real hardware (aarch64 only)
//! let regs = MmioMacRegs::new(0x0901_0000);
//! let mut mac = MacController::new(regs, [0x02,0,0,0,0,0x01], 1500, 16, 32, 0);
//! mac.init().expect("PHY autoneg failed");
//!
//! // Send a frame
//! let frame = [0xFF; 64];
//! mac.send(&frame).expect("send failed");
//!
//! // Receive a frame
//! let mut buf = [0u8; 1500];
//! let len = mac.recv(&mut buf).expect("recv failed");
//! ```
//!
//! # Design Decisions
//!
//! - **MacRegs trait**: Register access is abstracted via a trait, enabling
//!   mock testing without hardware. Real hardware uses [`MmioMacRegs`]
//!   (volatile MMIO); tests use [`MockMacRegs`](mock::MockMacRegs)
//!   (BTreeMap-backed).
//! - **GenericPhy**: Does not own the register set. Methods accept
//!   `&mut R: MacRegs`, allowing [`MacController`] to share its registers.
//! - **No VLAN/FCS**: `EthFrame` omits VLAN tags and FCS/CRC — the hardware
//!   MAC strips FCS, and VLAN is deferred to the TCP/IP stack.
//! - **no_std**: `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`.
//!   No `std` dependencies; all collections use `alloc`.

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod dma_ring;
pub mod error;
pub mod eth_frame;
pub mod mac;
#[cfg(test)]
mod mock;
pub mod perf;
pub mod phy;
pub mod security;
pub mod socket;
pub mod tcpip;

/// Crate version string.
pub const VERSION: &str = "0.30.0";

// Re-export key types for convenience.
pub use dma_ring::{DmaDescriptor, DmaRing, DESC_FS, DESC_IOC, DESC_LS, DESC_OWN};
pub use error::{NetError, NetStats};
pub use eth_frame::{EthFrame, EtherType};
#[cfg(target_arch = "aarch64")]
pub use mac::MmioMacRegs;
pub use mac::{MacController, MacRegs, NetDevice};
pub use phy::{
    GenericPhy, PhyDriver, PhyDuplex, PhySpeed, PhyState, BMCR_AUTONEG, BMCR_RESET, BMCR_RESTART,
    BMSR_ANEG_COMPLETE, BMSR_LINK, MII_ANAR, MII_ANLPAR, MII_BMCR, MII_BMSR, MII_PHYID1,
    MII_PHYID2,
};
// Re-export socket module types (v0.29.0 Socket abstraction layer).
// Note: socket::UdpSocket (SocketId newtype) is NOT re-exported here to avoid
// name collision with tcpip::UdpSocket (v0.28.0 handle-based wrapper). Access
// the v0.29.0 UdpSocket via `eneros_net::socket::UdpSocket`.
pub use socket::{
    Event, Interest, Poll, Readiness, Socket, SocketError, SocketId, SocketKind, SocketManager,
    TcpListener, TcpStream,
};
// Re-export tcpip module types (v0.28.0 TCP/IP protocol stack).
pub use tcpip::{
    DhcpClient, DhcpLease, DhcpState, HardwareAddress, IcmpSocket, InterfaceConfig, Ipv4Addr,
    Ipv4Cidr, NetworkInterface, SmolcpDevice, SocketAddr, SocketHandle, SocketSet, TcpIpError,
    TcpSocket, TcpState, UdpSocket,
};
