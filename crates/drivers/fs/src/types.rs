//! Filesystem type definitions.
//!
//! This module defines the value types used across the filesystem crate:
//! - [`FileMode`] — Unix-style file mode bits (file type + permissions).
//! - [`OpenFlags`] — flags controlling how a file is opened.
//! - [`SeekFrom`] — enumeration of seek origins.
//! - [`FileStat`] — metadata returned by `stat`.
//! - [`DirEntry`] — a directory entry returned by `readdir`.
//! - [`DiskUsage`] — disk space summary returned by `df`.

use alloc::string::String;
use core::fmt;

use bitflags::bitflags;

// ============================================================================
// FileMode
// ============================================================================

bitflags! {
    /// Unix-style file mode bits encoding the entry type and permissions.
    ///
    /// The high bits select the entry type (regular file or directory); the
    /// low bits encode read/write/execute permissions. This mirrors the
    /// `st_mode` field of POSIX `struct stat`.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct FileMode: u16 {
        /// Regular file type marker (0o100000).
        const FILE = 0o100_000;
        /// Directory type marker (0o040000).
        const DIR  = 0o040_000;
        /// Read permission (0o400).
        const READ = 0o400;
        /// Write permission (0o200).
        const WRITE = 0o200;
        /// Execute permission (0o100).
        const EXEC = 0o100;
    }
}

impl FileMode {
    /// Returns `true` if the mode marks a regular file.
    pub fn is_file(self) -> bool {
        self.contains(Self::FILE)
    }

    /// Returns `true` if the mode marks a directory.
    pub fn is_dir(self) -> bool {
        self.contains(Self::DIR)
    }

    /// Returns a default mode for a regular file: `FILE | READ | WRITE`.
    pub fn default_file() -> Self {
        Self::FILE | Self::READ | Self::WRITE
    }

    /// Returns a default mode for a directory: `DIR | READ | WRITE | EXEC`.
    pub fn default_dir() -> Self {
        Self::DIR | Self::READ | Self::WRITE | Self::EXEC
    }
}

impl fmt::Display for FileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0o{:o}", self.bits())
    }
}

// ============================================================================
// OpenFlags
// ============================================================================

bitflags! {
    /// Flags controlling how a file is opened.
    ///
    /// These are a simplified subset of POSIX `open(2)` flags, suitable for
    /// the littlefs2-backed implementation.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct OpenFlags: u8 {
        /// Open for reading.
        const READ = 1;
        /// Open for writing.
        const WRITE = 2;
        /// Create the file if it does not exist.
        const CREATE = 4;
        /// Create the file; fail if it already exists.
        const CREATE_NEW = 8;
        /// Truncate the file to zero length if it already exists.
        const TRUNCATE = 16;
        /// Append: writes go to the end of the file.
        const APPEND = 32;
    }
}

impl OpenFlags {
    /// Returns `true` if read access is requested.
    pub fn is_read(self) -> bool {
        self.contains(Self::READ)
    }

    /// Returns `true` if write access is requested.
    pub fn is_write(self) -> bool {
        self.contains(Self::WRITE)
    }

    /// Returns `true` if the file should be created.
    pub fn is_create(self) -> bool {
        self.contains(Self::CREATE) || self.contains(Self::CREATE_NEW)
    }

    /// Returns `true` if the file should be truncated.
    pub fn is_truncate(self) -> bool {
        self.contains(Self::TRUNCATE)
    }

    /// Returns `true` if writes should append to the end.
    pub fn is_append(self) -> bool {
        self.contains(Self::APPEND)
    }
}

impl fmt::Display for OpenFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0b{:b}", self.bits())
    }
}

// ============================================================================
// SeekFrom
// ============================================================================

/// Enumeration of possible seek origins.
///
/// Mirrors `std::io::SeekFrom` but uses `u64`/`i64` for the offset to support
/// large files on 64-bit targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekFrom {
    /// Seek from the start of the file.
    Start(u64),
    /// Seek from the end of the file (negative moves backwards).
    End(i64),
    /// Seek relative to the current position.
    Current(i64),
}

// ============================================================================
// FileStat
// ============================================================================

/// Metadata for a file or directory, returned by [`FileSystem::stat`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileStat {
    /// Inode number (littlefs assigns a unique id per entry).
    pub inode: u64,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// File mode (type + permissions).
    pub mode: u16,
    /// Creation timestamp (epoch seconds; 0 if unknown).
    pub created: u64,
    /// Last modification timestamp.
    pub modified: u64,
    /// Last access timestamp.
    pub accessed: u64,
    /// Number of blocks consumed (including metadata).
    pub block_count: u64,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

impl FileStat {
    /// Creates a `FileStat` for a regular file of the given size.
    pub fn file(size: u64) -> Self {
        Self {
            inode: 0,
            size,
            mode: FileMode::default_file().bits(),
            created: 0,
            modified: 0,
            accessed: 0,
            block_count: 0,
            is_dir: false,
        }
    }

    /// Creates a `FileStat` for a directory.
    pub fn dir() -> Self {
        Self {
            inode: 0,
            size: 0,
            mode: FileMode::default_dir().bits(),
            created: 0,
            modified: 0,
            accessed: 0,
            block_count: 0,
            is_dir: true,
        }
    }
}

// ============================================================================
// DirEntry
// ============================================================================

/// A directory entry, returned by [`FileSystem::readdir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Entry name (last path component, not the full path).
    pub name: String,
    /// Inode number.
    pub inode: u64,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

impl DirEntry {
    /// Creates a new `DirEntry` for a file.
    pub fn file(name: impl Into<String>, inode: u64) -> Self {
        Self {
            name: name.into(),
            inode,
            is_dir: false,
        }
    }

    /// Creates a new `DirEntry` for a directory.
    pub fn dir(name: impl Into<String>, inode: u64) -> Self {
        Self {
            name: name.into(),
            inode,
            is_dir: true,
        }
    }
}

// ============================================================================
// DiskUsage
// ============================================================================

/// Disk space usage summary, returned by [`FileSystem::df`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskUsage {
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Used bytes (data + metadata overhead).
    pub used_bytes: u64,
    /// Free bytes available for new writes.
    pub free_bytes: u64,
    /// Total number of blocks on the device.
    pub block_count: u64,
}

impl DiskUsage {
    /// Returns the usage ratio as a percentage (0–100).
    pub fn usage_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        let pct = (self.used_bytes * 100) / self.total_bytes;
        pct.min(100) as u8
    }
}

impl fmt::Display for DiskUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} used / {} total ({} free)",
            self.used_bytes, self.total_bytes, self.free_bytes
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    // ---- FileMode ----

    #[test]
    fn test_file_mode_file() {
        let m = FileMode::FILE;
        assert!(m.is_file());
        assert!(!m.is_dir());
    }

    #[test]
    fn test_file_mode_dir() {
        let m = FileMode::DIR;
        assert!(m.is_dir());
        assert!(!m.is_file());
    }

    #[test]
    fn test_file_mode_default_file() {
        let m = FileMode::default_file();
        assert!(m.is_file());
        assert!(m.contains(FileMode::READ));
        assert!(m.contains(FileMode::WRITE));
        assert!(!m.contains(FileMode::EXEC));
    }

    #[test]
    fn test_file_mode_default_dir() {
        let m = FileMode::default_dir();
        assert!(m.is_dir());
        assert!(m.contains(FileMode::READ));
        assert!(m.contains(FileMode::WRITE));
        assert!(m.contains(FileMode::EXEC));
    }

    #[test]
    fn test_file_mode_combine() {
        let m = FileMode::FILE | FileMode::READ | FileMode::WRITE;
        assert!(m.is_file());
        assert!(m.contains(FileMode::READ));
        assert!(m.contains(FileMode::WRITE));
    }

    #[test]
    fn test_file_mode_display() {
        let m = FileMode::default_file();
        assert_eq!(format!("{}", m), "0o100600");
    }

    #[test]
    fn test_file_mode_bits() {
        assert_eq!(FileMode::FILE.bits(), 0o100_000);
        assert_eq!(FileMode::DIR.bits(), 0o040_000);
        assert_eq!(FileMode::READ.bits(), 0o400);
        assert_eq!(FileMode::WRITE.bits(), 0o200);
        assert_eq!(FileMode::EXEC.bits(), 0o100);
    }

    // ---- OpenFlags ----

    #[test]
    fn test_open_flags_read() {
        let f = OpenFlags::READ;
        assert!(f.is_read());
        assert!(!f.is_write());
        assert!(!f.is_create());
    }

    #[test]
    fn test_open_flags_write() {
        let f = OpenFlags::WRITE;
        assert!(!f.is_read());
        assert!(f.is_write());
    }

    #[test]
    fn test_open_flags_create() {
        let f = OpenFlags::CREATE;
        assert!(f.is_create());
    }

    #[test]
    fn test_open_flags_create_new() {
        let f = OpenFlags::CREATE_NEW;
        assert!(f.is_create());
    }

    #[test]
    fn test_open_flags_truncate() {
        let f = OpenFlags::TRUNCATE;
        assert!(f.is_truncate());
    }

    #[test]
    fn test_open_flags_append() {
        let f = OpenFlags::APPEND;
        assert!(f.is_append());
    }

    #[test]
    fn test_open_flags_combine() {
        let f = OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
        assert!(f.is_read());
        assert!(f.is_write());
        assert!(f.is_create());
        assert!(f.is_truncate());
    }

    #[test]
    fn test_open_flags_display() {
        let f = OpenFlags::READ | OpenFlags::WRITE;
        assert_eq!(format!("{}", f), "0b11");
    }

    // ---- SeekFrom ----

    #[test]
    fn test_seek_from_start() {
        let s = SeekFrom::Start(1024);
        assert_eq!(s, SeekFrom::Start(1024));
    }

    #[test]
    fn test_seek_from_end() {
        let s = SeekFrom::End(-100);
        assert_eq!(s, SeekFrom::End(-100));
    }

    #[test]
    fn test_seek_from_current() {
        let s = SeekFrom::Current(50);
        assert_eq!(s, SeekFrom::Current(50));
    }

    #[test]
    fn test_seek_from_equality() {
        assert_eq!(SeekFrom::Start(0), SeekFrom::Start(0));
        assert_ne!(SeekFrom::Start(0), SeekFrom::Start(1));
        assert_ne!(SeekFrom::Start(0), SeekFrom::End(0));
        assert_ne!(SeekFrom::End(0), SeekFrom::Current(0));
    }

    // ---- FileStat ----

    #[test]
    fn test_file_stat_file() {
        let s = FileStat::file(4096);
        assert_eq!(s.size, 4096);
        assert!(!s.is_dir);
        assert!(FileMode::from_bits(s.mode).unwrap().is_file());
    }

    #[test]
    fn test_file_stat_dir() {
        let s = FileStat::dir();
        assert_eq!(s.size, 0);
        assert!(s.is_dir);
        assert!(FileMode::from_bits(s.mode).unwrap().is_dir());
    }

    #[test]
    fn test_file_stat_default() {
        let s = FileStat::default();
        assert_eq!(s.inode, 0);
        assert_eq!(s.size, 0);
        assert_eq!(s.mode, 0);
        assert!(!s.is_dir);
    }

    #[test]
    fn test_file_stat_with_inode() {
        let s = FileStat {
            inode: 42,
            size: 100,
            mode: FileMode::default_file().bits(),
            created: 1000,
            modified: 2000,
            accessed: 3000,
            block_count: 1,
            is_dir: false,
        };
        assert_eq!(s.inode, 42);
        assert_eq!(s.created, 1000);
        assert_eq!(s.modified, 2000);
        assert_eq!(s.accessed, 3000);
        assert_eq!(s.block_count, 1);
    }

    // ---- DirEntry ----

    #[test]
    fn test_dir_entry_file() {
        let e = DirEntry::file("test.txt", 1);
        assert_eq!(e.name, "test.txt");
        assert_eq!(e.inode, 1);
        assert!(!e.is_dir);
    }

    #[test]
    fn test_dir_entry_dir() {
        let e = DirEntry::dir("subdir", 2);
        assert_eq!(e.name, "subdir");
        assert_eq!(e.inode, 2);
        assert!(e.is_dir);
    }

    #[test]
    fn test_dir_entry_equality() {
        let a = DirEntry::file("a.txt", 1);
        let b = DirEntry::file("a.txt", 1);
        let c = DirEntry::file("a.txt", 2);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // ---- DiskUsage ----

    #[test]
    fn test_disk_usage_construction() {
        let du = DiskUsage {
            total_bytes: 1_000_000,
            used_bytes: 250_000,
            free_bytes: 750_000,
            block_count: 1000,
        };
        assert_eq!(du.total_bytes, 1_000_000);
        assert_eq!(du.used_bytes, 250_000);
        assert_eq!(du.free_bytes, 750_000);
        assert_eq!(du.block_count, 1000);
    }

    #[test]
    fn test_disk_usage_percent() {
        let du = DiskUsage {
            total_bytes: 1000,
            used_bytes: 250,
            free_bytes: 750,
            block_count: 10,
        };
        assert_eq!(du.usage_percent(), 25);
    }

    #[test]
    fn test_disk_usage_percent_full() {
        let du = DiskUsage {
            total_bytes: 1000,
            used_bytes: 1000,
            free_bytes: 0,
            block_count: 10,
        };
        assert_eq!(du.usage_percent(), 100);
    }

    #[test]
    fn test_disk_usage_percent_empty() {
        let du = DiskUsage {
            total_bytes: 1000,
            used_bytes: 0,
            free_bytes: 1000,
            block_count: 10,
        };
        assert_eq!(du.usage_percent(), 0);
    }

    #[test]
    fn test_disk_usage_percent_zero_total() {
        let du = DiskUsage::default();
        assert_eq!(du.usage_percent(), 0);
    }

    #[test]
    fn test_disk_usage_display() {
        let du = DiskUsage {
            total_bytes: 4096,
            used_bytes: 1024,
            free_bytes: 3072,
            block_count: 1,
        };
        let s = format!("{}", du);
        assert!(s.contains("1024"));
        assert!(s.contains("4096"));
        assert!(s.contains("3072"));
    }
}
