//! The [`FileSystem`] trait — a backend-agnostic filesystem interface.
//!
//! This trait abstracts over the concrete littlefs2 implementation so that
//! upper layers (Agent Runtime, protocol stacks) can be tested against a mock
//! filesystem or swapped to a different backend without changing call sites.
//!
//! # Design
//!
//! - [`open`](FileSystem::open) and [`create`](FileSystem::create) return a
//!   [`File`](crate::File) value type that stores only the path, offset, and
//!   flags. The file is not "held open" in the littlefs sense; each I/O
//!   operation re-opens the underlying file, performs the transfer, and closes
//!   it. This avoids the lifetime complications of littlefs2's closure-based
//!   file API while remaining correct (state is persisted on disk).
//! - Read-only operations (`stat`, `readdir`, `df`) take `&self` and rely on
//!   interior mutability inside the implementation.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::FsError;
use crate::file::File;
use crate::types::{DirEntry, DiskUsage, FileMode, FileStat, OpenFlags};

/// Backend-agnostic filesystem interface.
///
/// All paths are null-terminated ASCII strings (littlefs requirement). Paths
/// must not exceed 255 bytes. Leading `/` is optional but recommended.
pub trait FileSystem {
    /// Opens an existing file at `path` with the given `flags`.
    ///
    /// Returns [`FsError::NotFound`] if the path does not exist and
    /// `CREATE`/`CREATE_NEW` is not set.
    fn open(&mut self, path: &str, flags: OpenFlags) -> Result<File, FsError>;

    /// Creates a new file at `path` with the given `mode`.
    ///
    /// If the file already exists, it is truncated to zero length.
    fn create(&mut self, path: &str, mode: FileMode) -> Result<File, FsError>;

    /// Removes a file or empty directory at `path`.
    fn remove(&mut self, path: &str) -> Result<(), FsError>;

    /// Renames or moves a file/directory from `from` to `to`.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;

    /// Returns metadata for the entry at `path`.
    fn stat(&self, path: &str) -> Result<FileStat, FsError>;

    /// Creates a directory at `path`.
    fn mkdir(&mut self, path: &str) -> Result<(), FsError>;

    /// Removes an empty directory at `path`.
    fn rmdir(&mut self, path: &str) -> Result<(), FsError>;

    /// Lists the entries in the directory at `path`.
    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>, FsError>;

    /// Flushes all pending writes to persistent storage.
    fn sync(&mut self) -> Result<(), FsError>;

    /// Returns the current disk usage.
    fn df(&self) -> Result<DiskUsage, FsError>;

    /// 读取文件全部内容（v0.221.0，QoS 持久化需要）。
    ///
    /// 路径级原子读取；文件不存在返回 `Ok(空)`。
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, FsError>;

    /// 写入文件（截断重写）（v0.221.0，QoS 持久化需要）。
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError>;
}

/// A simple in-memory mock filesystem for trait-level testing.
pub struct MockFileSystem {
    files: BTreeMap<String, Vec<u8>>,
    dirs: Vec<String>,
}

impl Default for MockFileSystem {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: vec![String::from("/")],
        }
    }
}

impl MockFileSystem {
    /// 创建空内存文件系统。
    pub fn new() -> Self {
        Self::default()
    }
}

impl FileSystem for MockFileSystem {
    fn open(&mut self, path: &str, flags: OpenFlags) -> Result<File, FsError> {
        if !self.files.contains_key(path) {
            if flags.is_create() {
                self.files.insert(String::from(path), Vec::new());
            } else {
                return Err(FsError::NotFound {
                    path: String::from(path),
                });
            }
        }
        Ok(File::new(String::from(path), flags))
    }

    fn create(&mut self, path: &str, _mode: FileMode) -> Result<File, FsError> {
        self.files.insert(String::from(path), Vec::new());
        Ok(File::new(
            String::from(path),
            OpenFlags::READ | OpenFlags::WRITE,
        ))
    }

    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        if self.files.remove(path).is_some() {
            Ok(())
        } else {
            Err(FsError::NotFound {
                path: String::from(path),
            })
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let data = self.files.remove(from).ok_or_else(|| FsError::NotFound {
            path: String::from(from),
        })?;
        self.files.insert(String::from(to), data);
        Ok(())
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        if let Some(data) = self.files.get(path) {
            Ok(FileStat::file(data.len() as u64))
        } else if self.dirs.iter().any(|d| d == path) {
            Ok(FileStat::dir())
        } else {
            Err(FsError::NotFound {
                path: String::from(path),
            })
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        self.dirs.push(String::from(path));
        Ok(())
    }

    fn rmdir(&mut self, path: &str) -> Result<(), FsError> {
        let idx = self.dirs.iter().position(|d| d == path);
        match idx {
            Some(i) => {
                self.dirs.remove(i);
                Ok(())
            }
            None => Err(FsError::NotFound {
                path: String::from(path),
            }),
        }
    }

    fn readdir(&self, _path: &str) -> Result<Vec<DirEntry>, FsError> {
        let mut entries = Vec::new();
        for (name, data) in &self.files {
            entries.push(DirEntry::file(name.clone(), 0));
            let _ = data;
        }
        for name in self.dirs.iter().skip(1) {
            entries.push(DirEntry::dir(name.clone(), 0));
        }
        Ok(entries)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        match self.files.get(path) {
            Some(data) => Ok(data.clone()),
            None => Ok(Vec::new()),
        }
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        self.files.insert(String::from(path), data.to_vec());
        Ok(())
    }

    fn df(&self) -> Result<DiskUsage, FsError> {
        let used: usize = self.files.values().map(|v| v.len()).sum();
        Ok(DiskUsage {
            total_bytes: 1_048_576,
            used_bytes: used as u64,
            free_bytes: 1_048_576 - used as u64,
            block_count: 256,
        })
    }
}


// ============================================================================
// Tests — mock implementation verifies trait usability
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;


    #[test]
    fn test_mock_create_and_stat() {
        let mut fs = MockFileSystem::new();
        let _file = fs
            .create("/test.txt", FileMode::default_file())
            .expect("create should succeed");
        let stat = fs.stat("/test.txt").expect("stat should succeed");
        assert!(!stat.is_dir);
        assert_eq!(stat.size, 0);
    }

    #[test]
    fn test_mock_open_not_found() {
        let mut fs = MockFileSystem::new();
        let err = fs.open("/missing", OpenFlags::READ).unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[test]
    fn test_mock_open_create() {
        let mut fs = MockFileSystem::new();
        let _file = fs
            .open("/new.txt", OpenFlags::READ | OpenFlags::CREATE)
            .expect("open with CREATE should succeed");
        assert!(fs.stat("/new.txt").is_ok());
    }

    #[test]
    fn test_mock_remove() {
        let mut fs = MockFileSystem::new();
        fs.create("/del.txt", FileMode::default_file()).unwrap();
        fs.remove("/del.txt").expect("remove should succeed");
        assert!(fs.stat("/del.txt").is_err());
    }

    #[test]
    fn test_mock_rename() {
        let mut fs = MockFileSystem::new();
        fs.create("/old.txt", FileMode::default_file()).unwrap();
        fs.rename("/old.txt", "/new.txt")
            .expect("rename should succeed");
        assert!(fs.stat("/old.txt").is_err());
        assert!(fs.stat("/new.txt").is_ok());
    }

    #[test]
    fn test_mock_mkdir_rmdir() {
        let mut fs = MockFileSystem::new();
        fs.mkdir("/subdir").expect("mkdir should succeed");
        let stat = fs.stat("/subdir").expect("stat dir should succeed");
        assert!(stat.is_dir);
        fs.rmdir("/subdir").expect("rmdir should succeed");
        assert!(fs.stat("/subdir").is_err());
    }

    #[test]
    fn test_mock_readdir() {
        let mut fs = MockFileSystem::new();
        fs.create("/a.txt", FileMode::default_file()).unwrap();
        fs.create("/b.txt", FileMode::default_file()).unwrap();
        fs.mkdir("/subdir").unwrap();
        let entries = fs.readdir("/").expect("readdir should succeed");
        assert!(entries.len() >= 3);
    }

    #[test]
    fn test_mock_df() {
        let fs = MockFileSystem::new();
        let du = fs.df().expect("df should succeed");
        assert_eq!(du.total_bytes, 1_048_576);
        assert_eq!(du.free_bytes, 1_048_576);
    }

    #[test]
    fn test_mock_sync() {
        let mut fs = MockFileSystem::new();
        assert!(fs.sync().is_ok());
    }
}
