//! EnerOS Configuration Management Crate (workspace v0.192.0)
//!
//! Provides TOML/JSON configuration loading/saving, hot reload notification,
//! version management with rollback, and default value mechanism.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  Caller (Agent Runtime / Drivers / Kernel)    │
//! └─────────────┬────────────────────────────────┘
//!               │  ConfigManager API
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-config (this crate)                   │
//! │  ┌────────────┐ ┌──────────┐ ┌────────────┐  │
//! │  │ ConfigValue│ │ Loaders  │ │  Version   │  │
//! │  │  Schema    │ │ TOML/JSON│ │  History   │  │
//! │  └────────────┘ └──────────┘ └────────────┘  │
//! │  ┌────────────┐ ┌──────────────────────┐    │
//! │  │  Watcher   │ │  ConfigManager       │    │
//! │  │ Registry   │ │  (main entry point)  │    │
//! │  └────────────┘ └──────────────────────┘    │
//! └─────────────┬────────────────────────────────┘
//!               │  FileSystem trait (eneros-fs)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-fs::Lfs (workspace v0.192.0)                     │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```text
//! use eneros_config::{ConfigManager, ConfigValue};
//! use eneros_fs::Lfs;
//!
//! let fs = Lfs::format(dev).expect("format failed");
//! fn time_fn() -> u64 { 0 }
//! let mut mgr = ConfigManager::new(fs, "/config", time_fn)
//!     .expect("open config failed");
//!
//! // Get a configuration value.
//! if let Some(port) = mgr.get("device.port") {
//!     if let Some(port_num) = port.as_int() {
//!         // use port_num
//!     }
//! }
//!
//! // Set and persist a value.
//! mgr.set("device.port", ConfigValue::Int(9090))
//!     .expect("set failed");
//! ```
//!
//! # Design Decisions
//!
//! - **Holds `Lfs` concrete type**: `File::read/write` require `&mut Lfs`
//!   (concrete type, not trait object), so `ConfigManager` holds `Lfs` directly
//!   rather than `Box<dyn FileSystem>`. Same pattern as v0.25.0 TSDB.
//! - **Time source injection**: Uses `fn() -> u64` function pointer instead of
//!   hardcoding `crate::time::now()`, avoiding tight coupling with the time crate.
//! - **Official `toml` crate**: v1.0+ supports no_std with `default-features = false`
//!   + `parse`/`display` features. Chosen over `boml` (limited) and `tomling`
//!     (abandoned).
//! - **BTreeMap over HashMap**: All maps use `alloc::collections::BTreeMap` for
//!   no_std compatibility and deterministic iteration order.
//! - **Manual hot reload**: no_std RTOS has no inotify; "hot reload" is a manual
//!   `reload(name)` call that re-reads the file, validates schema, and notifies
//!   watchers.
//! - **Version history with CRC32**: Each version stores serialized data + CRC32
//!   checksum; rollback verifies integrity before restoring.

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod audit;
pub mod error;
pub mod loader;
pub mod manager;
pub mod schema;
pub mod store;
pub mod transaction;
pub mod version;
pub mod watcher;

// Re-export key types at crate root.
pub use audit::{AuditEntry, AuditLog, AuditOp};
pub use error::ConfigError;
pub use loader::{ConfigFormat, ConfigLoader, JsonLoader, TomlLoader};
pub use manager::ConfigManager;
pub use schema::{ConfigField, ConfigSchema, ConfigType, ConfigValue};
pub use transaction::{CasOutcome, CommitOutcome, Transaction};
pub use version::{ConfigVersion, VersionHistory};
pub use watcher::{ConfigWatcher, WatcherRegistry};

/// The crate version string.
pub const VERSION: &str = "0.26.0";
