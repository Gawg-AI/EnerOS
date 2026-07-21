//! Network security subsystem (v0.30.0).
//!
//! Provides firewall rule engine, connection tracking, rate limiting,
//! and DDoS protection for the network stack.

pub mod ddos;
pub mod firewall;
pub mod rate_limit;

// Re-export key types for convenience.
pub use ddos::{DdosProtector, SecurityError, SynInfo};
pub use firewall::{Firewall, FirewallAction, FirewallPolicy, FirewallRule, IpProtocol};
pub use rate_limit::{ConnInfo, ConnectionTracker, RateLimit};
