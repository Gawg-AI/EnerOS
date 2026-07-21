//! File handle — a value-type wrapper around a filesystem path.
//!
//! [`File`] stores the path, current offset, open flags, and an `is_open`
//! flag. It does **not** hold a littlefs2 `File` handle; instead, each I/O
//! operation re-opens the underlying file via the [`Lfs`](crate::Lfs)
//! implementation, performs the transfer, and closes it.
//!
//! # Rationale
//!
//! littlefs2's `File` requires a live `&Filesystem` reference and must be
//! explicitly closed before going out of scope (UB otherwise). Storing such a
//! handle inside a value-type `File` returned from `FileSystem::open` would
//! create lifetime and ownership problems. The value-type design trades a
//! small per-operation overhead (re-opening) for safe, ergonomic code.
//!
//! # Usage
//!
//! ```ignore
//! let mut file = fs.open("/data.txt", OpenFlags::READ)?;
//! let mut buf = [0u8; 64];
//! let n = file.read(&mut fs, &mut buf)?;
//! ```

use alloc::string::String;

use crate::error::FsError;
use crate::types::{OpenFlags, SeekFrom};

/// A file handle storing path, offset, and flags.
///
/// Created by [`FileSystem::open`](crate::FileSystem::open) or
/// [`FileSystem::create`](crate::FileSystem::create). Use
/// [`read`](File::read)/[`write`](File::write) with a `&mut Lfs` reference
/// to perform I/O.
#[derive(Debug, Clone)]
pub struct File {
    /// Path to the file in the filesystem.
    path: String,
    /// Current read/write offset (byte position from start).
    offset: u64,
    /// Flags controlling access mode.
    flags: OpenFlags,
    /// Whether the file is logically open.
    is_open: bool,
}

impl File {
    /// Creates a new `File` handle (called by `FileSystem::open`/`create`).
    pub(crate) fn new(path: String, flags: OpenFlags) -> Self {
        Self {
            path,
            offset: 0,
            flags,
            is_open: true,
        }
    }

    /// Returns the file path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the current offset.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the open flags.
    pub fn flags(&self) -> OpenFlags {
        self.flags
    }

    /// Returns `true` if the file is still open.
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Reads up to `buf.len()` bytes from the file at the current offset,
    /// delegating the actual I/O to the [`Lfs`](crate::Lfs) implementation.
    ///
    /// The offset is advanced by the number of bytes read. Returns 0 when
    /// the end of the file is reached.
    pub fn read(&mut self, fs: &mut crate::Lfs, buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.is_open {
            return Err(FsError::InvalidArgument);
        }
        if !self.flags.is_read() {
            return Err(FsError::InvalidArgument);
        }
        let n = fs.read_file_at(&self.path, self.offset, buf)?;
        self.offset = self.offset.saturating_add(n as u64);
        Ok(n)
    }

    /// Writes `buf` to the file at the current offset, delegating the actual
    /// I/O to the [`Lfs`](crate::Lfs) implementation.
    ///
    /// The offset is advanced by the number of bytes written. If
    /// [`OpenFlags::APPEND`] is set, the write goes to the end of the file
    /// regardless of the current offset.
    pub fn write(&mut self, fs: &mut crate::Lfs, buf: &[u8]) -> Result<usize, FsError> {
        if !self.is_open {
            return Err(FsError::InvalidArgument);
        }
        if !self.flags.is_write() {
            return Err(FsError::ReadOnly);
        }
        let write_offset = if self.flags.is_append() {
            // For append mode, seek to end first.
            let size = fs.file_size(&self.path)?;
            self.offset = size;
            size
        } else {
            self.offset
        };
        let n = fs.write_file_at(&self.path, write_offset, buf)?;
        self.offset = write_offset.saturating_add(n as u64);
        Ok(n)
    }

    /// Seeks to the given position and returns the new offset.
    ///
    /// For [`SeekFrom::End`], the file size is queried first.
    pub fn seek(&mut self, fs: &mut crate::Lfs, pos: SeekFrom) -> Result<u64, FsError> {
        if !self.is_open {
            return Err(FsError::InvalidArgument);
        }
        let new_offset = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(delta) => {
                let size = fs.file_size(&self.path)?;
                if delta >= 0 {
                    size.saturating_add(delta as u64)
                } else {
                    size.saturating_sub((-delta) as u64)
                }
            }
            SeekFrom::Current(delta) => {
                if delta >= 0 {
                    self.offset.saturating_add(delta as u64)
                } else {
                    self.offset.saturating_sub((-delta) as u64)
                }
            }
        };
        self.offset = new_offset;
        Ok(new_offset)
    }

    /// Truncates (or extends) the file to the given `size`.
    pub fn truncate(&mut self, fs: &mut crate::Lfs, size: u64) -> Result<(), FsError> {
        if !self.is_open {
            return Err(FsError::InvalidArgument);
        }
        if !self.flags.is_write() {
            return Err(FsError::ReadOnly);
        }
        fs.truncate_file(&self.path, size)
    }

    /// Closes the file handle.
    ///
    /// In the value-type design this simply marks the handle as closed; no
    /// littlefs2 resources need to be released (each operation opened and
    /// closed the underlying file already).
    pub fn close(mut self) -> Result<(), FsError> {
        self.is_open = false;
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_file_new() {
        let f = File::new(
            String::from("/test.txt"),
            OpenFlags::READ | OpenFlags::WRITE,
        );
        assert_eq!(f.path(), "/test.txt");
        assert_eq!(f.offset(), 0);
        assert!(f.is_open());
        assert!(f.flags().is_read());
        assert!(f.flags().is_write());
    }

    #[test]
    fn test_file_close() {
        let f = File::new(String::from("/test.txt"), OpenFlags::READ);
        assert!(f.is_open());
        f.close().expect("close should succeed");
        // After close, is_open is false (but f is consumed).
    }

    #[test]
    fn test_file_clone() {
        let f = File::new(String::from("/test.txt"), OpenFlags::READ);
        let cloned = f.clone();
        assert_eq!(f.path(), cloned.path());
        assert_eq!(f.offset(), cloned.offset());
        assert_eq!(f.flags(), cloned.flags());
        assert_eq!(f.is_open(), cloned.is_open());
    }

    #[test]
    fn test_file_flags_accessors() {
        let f = File::new(
            String::from("/a"),
            OpenFlags::READ | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        );
        assert!(f.flags().is_read());
        assert!(!f.flags().is_write());
        assert!(f.flags().is_create());
        assert!(f.flags().is_truncate());
        assert!(!f.flags().is_append());
    }
}
