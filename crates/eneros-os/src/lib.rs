//! EnerOS Power-Native Operating System layer
//!
//! This crate provides the OS-level functionality that makes EnerOS
//! a power-native operating system rather than an application:
//!
//! - `init`: PID 1 init system and service supervisor
//! - `rt`: Real-time runtime with SCHED_FIFO and CPU isolation
//! - `netcfg`: Network configuration (no NetworkManager dependency)
//! - `timesync`: Time synchronization (PTP/NTP)
//! - `syslog`: System logging with rotation
//! - `devmgr`: Device management and hotplug
//! - `update`: OTA update with A/B partition
//! - `hal`: Hardware abstraction layer
//! - `agentos`: AgentOS kernel — Agent process management, IPC, authority, quota, scheduler

pub mod init;
pub mod rt;
pub mod netcfg;
pub mod timesync;
pub mod syslog;
pub mod devmgr;
pub mod update;
pub mod hal;
pub mod agentos;
pub mod security;
pub mod ha;
