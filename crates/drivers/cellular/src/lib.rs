//! EnerOS Cellular Modem Driver & Dual-Network Redundancy (v0.30.0).
//!
//! Provides:
//! - **v0.30.1**: AT command encapsulation, PPP dial-up protocol, CellularModem driver
//! - **v0.30.2**: Heartbeat monitoring, failover management, dual-network redundancy
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  RedundancyManager (v0.30.2)                 │
//! │  ┌─────────────┐  ┌──────────────────────┐   │
//! │  │ LinkState   │  │ FailoverManager      │   │
//! │  │ (primary)   │  │ ┌──────────────────┐ │   │
//! │  └─────────────┘  │ │ HeartbeatMonitor │ │   │
//! │  ┌─────────────┐  │ └──────────────────┘ │   │
//! │  │ LinkState   │  │ ┌──────────────────┐ │   │
//! │  │ (backup)    │  │ │ HeartbeatMonitor │ │   │
//! │  └─────────────┘  │ └──────────────────┘ │   │
//! │                   └──────────────────────┘   │
//! └───────────────┬──────────────────────────────┘
//!                 │
//! ┌───────────────▼──────────────────────────────┐
//! │  CellularModem<S: HalSerial> (v0.30.1)       │
//! │  ┌──────────────┐  ┌─────────────────────┐   │
//! │  │ AtParser     │  │ PppStateMachine     │   │
//! │  │ (AT+CSQ etc) │  │ (LCP→Auth→IPCP→IP)  │   │
//! │  └──────────────┘  └─────────────────────┘   │
//! └───────────────┬──────────────────────────────┘
//!                 │  HalSerial trait
//! ┌───────────────▼──────────────────────────────┐
//! │  eneros-hal (UART driver)                    │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_cellular::{CellularModem, RetryConfig};
//! use eneros_hal::HalSerial;
//!
//! // Create modem with serial port
//! let modem = CellularModem::new(serial, "internet", RetryConfig::default());
//!
//! // Check signal and dial
//! let signal = modem.check_signal().unwrap();
//! let ip = modem.dial("internet").unwrap();
//!
//! // Dual-network redundancy
//! use eneros_cellular::RedundancyManager;
//! let mut redundancy = RedundancyManager::new();
//! redundancy.set_primary_status(false, now); // Trigger failover
//! ```
//!
//! # no_std Compliance
//! All code is `no_std` with `alloc`. Uses `BTreeMap` (not `HashMap`).
//!
//! # Deviation Notes
//! - PPP is a minimal implementation (state machine + basic HDLC frames).
//!   Full PPP stack (LCP/IPCP/PAP/CHAP complete packets) requires hardware verification.
//! - PppDevice smoltcp adapter interface is defined; actual data channel needs hardware.
//! - Integration tests (real modem, cable unplug) are deferred to hardware environment.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod at_command;
pub mod error;
pub mod failover;
pub mod heartbeat;
pub mod modem;
pub mod ppp;
pub mod redundancy;

/// Crate version string.
pub const VERSION: &str = "0.30.0";

// Re-export key types for convenience.
pub use at_command::{AtCommand, AtParser, AtResponse, NetworkType, SignalStrength};
pub use error::{CellularError, FailoverError};
pub use failover::{FailoverEvent, FailoverManager, FailoverState, LinkType};
pub use heartbeat::HeartbeatMonitor;
pub use modem::{CellularDriver, CellularModem, RetryConfig};
pub use ppp::{
    Ipv4Addr, PppFrame, PppState, PppStateMachine, HDLC_ESCAPE, HDLC_FLAG, PPP_CHAP, PPP_IP,
    PPP_IPCP, PPP_LCP, PPP_PAP,
};
pub use redundancy::{LinkState, RedundancyManager};
