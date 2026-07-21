//! EnerOS File System Crate (v0.24.0)
//!
//! Log-structured file system based on littlefs2 integration.
//! Provides power-loss safe, wear-leveled file storage on top of
//! the v0.23.0 [`BlockDevice`] trait.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  Caller (Agent Runtime / TSDB / Config)       │
//! └─────────────┬────────────────────────────────┘
//!               │  FileSystem trait (11 methods)
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-fs::Lfs  (this crate)                 │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  File handle (value type)              │  │
//! │  │  FsError / FileMode / OpenFlags / ...  │  │
//! │  └────────────────────────────────────────┘  │
//! │  ┌────────────────────────────────────────┐  │
//! │  │  BlockDeviceStorage adapter            │  │
//! │  │  (impl littlefs2::driver::Storage)     │  │
//! │  └────────────────────────────────────────┘  │
//! └─────────────┬────────────────────────────────┘
//!               │  read_block / write_block / erase_block
//! ┌─────────────▼────────────────────────────────┐
//! │  eneros-storage::BlockDevice (v0.23.0)        │
//! │  MockBlockDevice / EmmcDriver / NvmeDriver    │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_fs::{Lfs, FileSystem, FileMode, OpenFlags};
//! use eneros_storage::{BlockDevice, MockBlockDevice};
//! use alloc::boxed::Box;
//!
//! let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
//! let mut fs = Lfs::format(dev).expect("format failed");
//!
//! // Create and write a file.
//! let mut file = fs.create("/hello.txt", FileMode::default_file())?;
//! file.write(&mut fs, b"hello world")?;
//!
//! // Read it back.
//! let stat = fs.stat("/hello.txt")?;
//! assert_eq!(stat.size, 11);
//! ```
//!
//! # Design Decisions
//!
//! - **littlefs2 integration** (per project rules §5.5): we use the mature
//!   `littlefs2` crate rather than self-implementing an LFS. This gives us
//!   copy-on-write power-loss safety, dynamic wear leveling, and bad block
//!   management for free.
//! - **Value-type `File` handle**: stores only path/offset/flags; each I/O
//!   operation re-opens the underlying file via littlefs2's closure-based
//!   API. This avoids lifetime issues from littlefs2's linked-list file
//!   handles.
//! - **Per-operation `mount_and_then`**: each `FileSystem` call mounts and
//!   unmounts the littlefs2 instance. State persists on disk; the overhead
//!   is small (a few cache reads per call).
//!
//! [`BlockDevice`]: eneros_storage::BlockDevice

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod error;
pub mod fs_trait;
pub mod lfs;
pub mod types;
pub mod wear;

mod file;

// Re-export the most commonly used types at the crate root.
pub use error::FsError;
pub use file::File;
pub use fs_trait::FileSystem;
pub use lfs::{BlockDeviceStorage, Lfs, LfsConfig};
pub use types::{DirEntry, DiskUsage, FileMode, FileStat, OpenFlags, SeekFrom};
// Re-export wear-leveling types.
pub use wear::{
    init_default, init_global, record_app_write, record_erase, record_flash_write,
    set_write_amp_limit, trigger_wear_leveling, wear_level_status, WearDistribution,
    WearLevelManager, WearLeveling, WearStatus, WriteAmplificationTracker,
};

/// The crate version string.
pub const VERSION: &str = "0.24.1";
