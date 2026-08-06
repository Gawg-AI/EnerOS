//! Error types for the cellular subsystem.

/// Errors from cellular modem operations (workspace v0.192.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellularError {
    /// No SIM card detected.
    NoSimCard,
    /// No cellular signal.
    NoSignal,
    /// PPP dial-up failed.
    DialFailed,
    /// AT command timed out.
    AtCommandTimeout,
    /// PPP negotiation failed.
    PppNegotiationFailed,
}

/// Errors from failover management (workspace v0.192.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverError {
    /// No backup link available.
    NoBackupAvailable,
    /// A switch is already in progress.
    SwitchInProgress,
    /// Heartbeat timeout exceeded.
    HeartbeatTimeout,
    /// Invalid state for the requested operation.
    InvalidState,
}
