//! littlefs2 integration layer.
//!
//! This module groups together the components that bridge EnerOS block
//! devices to the littlefs2 filesystem:
//!
//! - [`storage_adapter`] — adapts [`BlockDevice`] to littlefs2's [`Storage`]
//!   trait.
//! - [`config`] — runtime configuration ([`LfsConfig`]).
//! - [`filesystem`] — the [`Lfs`] struct that implements [`FileSystem`].
//!
//! [`Storage`]: littlefs2::driver::Storage
//! [`BlockDevice`]: eneros_storage::BlockDevice
//! [`FileSystem`]: crate::FileSystem

pub mod config;
pub mod filesystem;
pub mod storage_adapter;

pub use config::LfsConfig;
pub use filesystem::Lfs;
pub use storage_adapter::BlockDeviceStorage;
