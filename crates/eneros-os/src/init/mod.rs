//! PID 1 init system and service supervisor

pub mod service;
pub mod graph;
pub mod supervisor;
pub mod manager;
pub mod config;
pub mod signal;
pub mod netcfg;
pub mod firewall;
pub mod devmgr;
pub mod timesync;
pub mod syslog;
pub mod audit;
pub mod serial_mgr;
pub mod usb_mgr;

pub use service::{Service, ServiceConfig, ServiceStatus, RestartPolicy};
pub use graph::ServiceGraph;
pub use supervisor::Supervisor;
pub use manager::ServiceManager;
pub use config::{AgentServiceConfig, InitConfig};
pub use signal::SignalHandler;
pub use netcfg::{NetworkConfig, NetworkInterface, BondConfig, BondStatus, NetworkError};
pub use firewall::{FirewallManager, FirewallConfig, FirewallRule};
pub use devmgr::{
    DeviceManager, DeviceType, DeviceStatus, DeviceInfo, DeviceConfig, DeviceRule,
    HotplugEvent, HotplugAction, DeviceError,
};
pub use timesync::{
    TimeSyncManager, TimeSyncConfig, TimeSyncStatus, ClockSource, TimeSyncError,
    PtpConfig, NtpConfig, PhcInfo, request_daemon_shutdown,
};
pub use syslog::{SyslogManager, SyslogConfig, LogEntry, LogLevel, LogCategory, SyslogError};
pub use audit::{
    AuditLogger, AuditConfig, AuditEntry, AuditAction, AuditResult, AuditError,
    IntegrityViolation, ViolationType,
};
pub use serial_mgr::{
    SerialAccessControl, SerialConfigData, SerialHealth, SerialMgrError, SerialMonitor,
    SerialPreset,
};
pub use usb_mgr::{UsbMgrError, UsbSerialAdapter, UsbWhitelist, UsbWhitelistRule};
