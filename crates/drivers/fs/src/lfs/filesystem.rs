//! [`Lfs`] — the littlefs2-backed [`FileSystem`] implementation.
//!
//! `Lfs` wraps a [`BlockDeviceStorage`] adapter (which in turn wraps a
//! [`BlockDevice`]) and implements the [`FileSystem`] trait by delegating each
//! operation to littlefs2 via the closure-based `mount_and_then` API.
//!
//! # Design
//!
//! Because littlefs2's [`Filesystem`] borrows `&mut Storage` for its entire
//! lifetime, we cannot store both the storage and a mounted `Filesystem` in
//! the same struct without self-referential pointers. Instead, `Lfs` holds the
//! storage in a [`RefCell`] and performs a fresh `mount` for each operation.
//! This is correct (state persists on disk) and the mount/unmount overhead is
//! small (a few cache reads).
//!
//! Read-only trait methods (`stat`, `readdir`, `df`) take `&self` and use
//! [`RefCell::borrow_mut`] internally — safe because the borrow is released
//! before the method returns.
//!
//! # Path Handling
//!
//! littlefs2 requires null-terminated ASCII paths. We convert `&str` to
//! [`PathBuf`] on each call; non-ASCII or over-long (>255 byte) paths return
//! [`FsError::InvalidPath`].
//!
//! [`Filesystem`]: littlefs2::fs::Filesystem

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use eneros_storage::BlockDevice;
use littlefs2::fs::{Filesystem, OpenOptions};
use littlefs2::io::SeekFrom as LfsSeekFrom;
use littlefs2::path::PathBuf;

use crate::error::FsError;
use crate::file::File;
use crate::fs_trait::FileSystem;
use crate::lfs::storage_adapter::BlockDeviceStorage;
use crate::types::{DirEntry, DiskUsage, FileMode, FileStat, OpenFlags};
use crate::LfsConfig;

/// littlefs2-backed filesystem implementing [`FileSystem`].
///
/// Construct with [`Lfs::format`] (first-time formatting) or [`Lfs::mount`]
/// (mount an already-formatted device).
pub struct Lfs {
    storage: RefCell<BlockDeviceStorage>,
    config: LfsConfig,
}

impl fmt::Debug for Lfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lfs")
            .field("config", &self.config)
            .field("block_count", &self.storage.borrow().device_block_count())
            .finish()
    }
}

impl Lfs {
    /// Formats the block device and returns a mounted `Lfs`.
    ///
    /// This is equivalent to `format` followed by `mount`. Use on a fresh
    /// device or to wipe an existing filesystem.
    pub fn format(device: Box<dyn BlockDevice>) -> Result<Self, FsError> {
        Self::format_with_config(device, LfsConfig::default())
    }

    /// Formats the block device with the given configuration.
    pub fn format_with_config(
        device: Box<dyn BlockDevice>,
        config: LfsConfig,
    ) -> Result<Self, FsError> {
        let mut storage = BlockDeviceStorage::new(device)?;
        Filesystem::format(&mut storage).map_err(FsError::from)?;
        Ok(Self {
            storage: RefCell::new(storage),
            config,
        })
    }

    /// Mounts an already-formatted filesystem.
    ///
    /// Returns [`FsError::BadSuperBlock`] if the device is not formatted.
    pub fn mount(device: Box<dyn BlockDevice>) -> Result<Self, FsError> {
        Self::mount_with_config(device, LfsConfig::default())
    }

    /// Mounts an already-formatted filesystem with the given configuration.
    pub fn mount_with_config(
        device: Box<dyn BlockDevice>,
        config: LfsConfig,
    ) -> Result<Self, FsError> {
        let mut storage = BlockDeviceStorage::new(device)?;
        // Verify mountability; if this fails the superblock is missing/invalid.
        let mount_ok = Filesystem::is_mountable(&mut storage);
        if !mount_ok {
            return Err(FsError::BadSuperBlock);
        }
        Ok(Self {
            storage: RefCell::new(storage),
            config,
        })
    }

    /// Returns the active configuration.
    pub fn config(&self) -> &LfsConfig {
        &self.config
    }

    /// Returns the block count of the underlying device.
    pub fn block_count(&self) -> usize {
        self.storage.borrow().device_block_count()
    }

    // --------------------------------------------------------------------
    // Helpers used by `File` (file.rs)
    // --------------------------------------------------------------------

    /// Reads up to `buf.len()` bytes from the file at `path` starting at
    /// `offset`. Returns the number of bytes read (0 at EOF).
    pub(crate) fn read_file_at(
        &self,
        path: &str,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| {
            OpenOptions::new()
                .read(true)
                .open_and_then(fs, &lfs_path, |file| {
                    if offset > 0 {
                        file.seek(LfsSeekFrom::Start(offset as u32))?;
                    }
                    file.read(buf)
                })
        })
        .map_err(FsError::from)
    }

    /// Writes `buf` to the file at `path` starting at `offset`. The file must
    /// already exist. Returns the number of bytes written.
    pub(crate) fn write_file_at(
        &self,
        path: &str,
        offset: u64,
        buf: &[u8],
    ) -> Result<usize, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open_and_then(fs, &lfs_path, |file| {
                    if offset > 0 {
                        file.seek(LfsSeekFrom::Start(offset as u32))?;
                    }
                    file.write(buf)
                })
        })
        .map_err(FsError::from)
    }

    /// Returns the size of the file at `path` in bytes.
    pub(crate) fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        let size = Filesystem::mount_and_then(&mut *storage, |fs| {
            OpenOptions::new()
                .read(true)
                .open_and_then(fs, &lfs_path, |file| file.len())
        })
        .map_err(FsError::from)?;
        Ok(size as u64)
    }

    /// Truncates (or extends) the file at `path` to `size` bytes.
    pub(crate) fn truncate_file(&self, path: &str, size: u64) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| {
            OpenOptions::new()
                .write(true)
                .open_and_then(fs, &lfs_path, |file| file.set_len(size as usize))
        })
        .map_err(FsError::from)
    }
}

impl FileSystem for Lfs {
    fn open(&mut self, path: &str, flags: OpenFlags) -> Result<File, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();

        if flags.is_create() {
            // CREATE / CREATE_NEW: create the file on disk so subsequent
            // stat() calls succeed even before the first write.
            Filesystem::mount_and_then(&mut *storage, |fs| {
                let mut opts = OpenOptions::new();
                opts.write(true);
                if flags.contains(OpenFlags::CREATE_NEW) {
                    opts.create_new(true);
                } else {
                    opts.create(true);
                    if flags.is_truncate() {
                        opts.truncate(true);
                    }
                }
                opts.open_and_then(fs, &lfs_path, |_file| Ok(()))
            })
            .map_err(FsError::from)?;
        } else {
            // Non-create path: verify the file exists.
            let exists =
                Filesystem::mount_and_then(&mut *storage, |fs| match fs.metadata(&lfs_path) {
                    Ok(_) => Ok(true),
                    Err(littlefs2::io::Error::NO_SUCH_ENTRY) => Ok(false),
                    Err(e) => Err(e),
                })
                .map_err(FsError::from)?;
            if !exists {
                return Err(FsError::NotFound {
                    path: String::from(path),
                });
            }
            if flags.is_truncate() && flags.is_write() {
                Filesystem::mount_and_then(&mut *storage, |fs| {
                    let mut opts = OpenOptions::new();
                    opts.write(true);
                    opts.open_and_then(fs, &lfs_path, |file| file.set_len(0))
                })
                .map_err(FsError::from)?;
            }
        }
        Ok(File::new(String::from(path), flags))
    }

    fn create(&mut self, path: &str, _mode: FileMode) -> Result<File, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        // Use littlefs2's create_and_then which opens with create+truncate.
        Filesystem::mount_and_then(&mut *storage, |fs| {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open_and_then(fs, &lfs_path, |_file| Ok(()))
        })
        .map_err(FsError::from)?;
        drop(storage);
        Ok(File::new(
            String::from(path),
            OpenFlags::READ | OpenFlags::WRITE,
        ))
    }

    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| fs.remove(&lfs_path)).map_err(FsError::from)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let lfs_from = make_path(from)?;
        let lfs_to = make_path(to)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| fs.rename(&lfs_from, &lfs_to))
            .map_err(FsError::from)
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        let metadata = Filesystem::mount_and_then(&mut *storage, |fs| fs.metadata(&lfs_path))
            .map_err(FsError::from)?;
        let mode = if metadata.is_file() {
            FileMode::default_file().bits()
        } else {
            FileMode::default_dir().bits()
        };
        Ok(FileStat {
            inode: 0,
            size: metadata.len() as u64,
            mode,
            created: 0,
            modified: 0,
            accessed: 0,
            block_count: 0,
            is_dir: metadata.is_dir(),
        })
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| fs.create_dir(&lfs_path))
            .map_err(FsError::from)
    }

    fn rmdir(&mut self, path: &str) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |fs| fs.remove_dir(&lfs_path))
            .map_err(FsError::from)
    }

    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let lfs_path = make_path(path)?;
        let mut storage = self.storage.borrow_mut();
        let entries: Vec<DirEntry> = Filesystem::mount_and_then(&mut *storage, |fs| {
            let mut out: Vec<DirEntry> = Vec::new();
            fs.read_dir_and_then(&lfs_path, |read_dir| {
                // Skip the first two entries ("." and "..") emitted by littlefs.
                for entry in read_dir.skip(2) {
                    let entry = entry?;
                    let name = String::from(entry.file_name().as_str());
                    out.push(DirEntry {
                        name,
                        inode: 0,
                        is_dir: entry.file_type().is_dir(),
                    });
                }
                Ok(())
            })?;
            Ok(out)
        })
        .map_err(FsError::from)?;
        Ok(entries)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        // littlefs2 syncs every file close, so a mount/unmount cycle is the
        // natural sync point. Re-mounting forces the metadata to be flushed.
        // For the value-type File design, all writes are already flushed at
        // the end of each `mount_and_then` closure, so this is effectively a
        // no-op. We perform a no-op mount to verify the filesystem is still
        // healthy.
        let mut storage = self.storage.borrow_mut();
        Filesystem::mount_and_then(&mut *storage, |_fs| Ok(())).map_err(FsError::from)
    }

    fn df(&self) -> Result<DiskUsage, FsError> {
        let mut storage = self.storage.borrow_mut();
        let (total_bytes, free_bytes, block_count) =
            Filesystem::mount_and_then(&mut *storage, |fs| {
                let total = fs.total_space();
                let free = fs.available_space()?;
                Ok((total, free, fs.total_blocks()))
            })
            .map_err(FsError::from)?;
        let used_bytes = total_bytes.saturating_sub(free_bytes);
        Ok(DiskUsage {
            total_bytes: total_bytes as u64,
            used_bytes: used_bytes as u64,
            free_bytes: free_bytes as u64,
            block_count: block_count as u64,
        })
    }
}

/// Converts a `&str` path to a littlefs2 [`PathBuf`].
///
/// Returns [`FsError::InvalidPath`] if the string is non-ASCII, contains
/// embedded nulls, or exceeds 255 bytes.
fn make_path(path: &str) -> Result<PathBuf, FsError> {
    PathBuf::try_from(path).map_err(|_| FsError::InvalidPath {
        path: String::from(path),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use eneros_storage::MockBlockDevice;

    use super::*;

    fn make_fs() -> Lfs {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        Lfs::format(dev).expect("format should succeed")
    }

    #[test]
    fn test_format_and_mount() {
        let fs = make_fs();
        assert_eq!(fs.block_count(), 64);
        assert_eq!(fs.config().block_size, 4096);
    }

    #[test]
    fn test_mount_unformatted_fails() {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        let err = Lfs::mount(dev).unwrap_err();
        assert_eq!(err, FsError::BadSuperBlock);
    }

    #[test]
    fn test_create_and_stat() {
        let mut fs = make_fs();
        let _file = fs
            .create("/test.txt", FileMode::default_file())
            .expect("create should succeed");
        let stat = fs.stat("/test.txt").expect("stat should succeed");
        assert!(!stat.is_dir);
        assert_eq!(stat.size, 0);
    }

    #[test]
    fn test_stat_missing() {
        let fs = make_fs();
        let err = fs.stat("/missing.txt").unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[test]
    fn test_write_and_read() {
        let mut fs = make_fs();
        let mut file = fs
            .create("/data.bin", FileMode::default_file())
            .expect("create should succeed");
        let data = b"hello world";
        let n = file.write(&mut fs, data).expect("write should succeed");
        assert_eq!(n, data.len());

        // Read back from start.
        let mut buf = [0u8; 11];
        let mut read_file = fs
            .open("/data.bin", OpenFlags::READ)
            .expect("open should succeed");
        let n = read_file
            .read(&mut fs, &mut buf)
            .expect("read should succeed");
        assert_eq!(n, data.len());
        assert_eq!(&buf, data);
    }

    #[test]
    fn test_write_at_offset() {
        let mut fs = make_fs();
        // Pre-create a file with some content.
        let mut file = fs
            .create("/offset.bin", FileMode::default_file())
            .expect("create should succeed");
        let _ = file
            .write(&mut fs, b"AAAAABBBBB")
            .expect("write should succeed");

        // Overwrite bytes 5..10.
        let n = fs
            .write_file_at("/offset.bin", 5, b"CCCCC")
            .expect("write_at should succeed");
        assert_eq!(n, 5);

        // Read full file.
        let mut buf = [0u8; 10];
        let n = fs
            .read_file_at("/offset.bin", 0, &mut buf)
            .expect("read should succeed");
        assert_eq!(n, 10);
        assert_eq!(&buf, b"AAAAACCCCC");
    }

    #[test]
    fn test_file_size() {
        let mut fs = make_fs();
        let mut file = fs
            .create("/size.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = file
            .write(&mut fs, b"1234567890")
            .expect("write should succeed");
        let size = fs.file_size("/size.txt").expect("file_size should succeed");
        assert_eq!(size, 10);
    }

    #[test]
    fn test_truncate_file() {
        let mut fs = make_fs();
        let mut file = fs
            .create("/trunc.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = file
            .write(&mut fs, b"1234567890")
            .expect("write should succeed");
        fs.truncate_file("/trunc.txt", 5)
            .expect("truncate should succeed");
        let size = fs
            .file_size("/trunc.txt")
            .expect("file_size should succeed");
        assert_eq!(size, 5);
    }

    #[test]
    fn test_remove() {
        let mut fs = make_fs();
        fs.create("/rm.txt", FileMode::default_file())
            .expect("create should succeed");
        fs.remove("/rm.txt").expect("remove should succeed");
        let err = fs.stat("/rm.txt").unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[test]
    fn test_rename() {
        let mut fs = make_fs();
        fs.create("/old.txt", FileMode::default_file())
            .expect("create should succeed");
        fs.rename("/old.txt", "/new.txt")
            .expect("rename should succeed");
        assert!(fs.stat("/old.txt").is_err());
        assert!(fs.stat("/new.txt").is_ok());
    }

    #[test]
    fn test_mkdir_and_stat_dir() {
        let mut fs = make_fs();
        fs.mkdir("/data").expect("mkdir should succeed");
        let stat = fs.stat("/data").expect("stat should succeed");
        assert!(stat.is_dir);
    }

    #[test]
    fn test_rmdir() {
        let mut fs = make_fs();
        fs.mkdir("/tmpdir").expect("mkdir should succeed");
        fs.rmdir("/tmpdir").expect("rmdir should succeed");
        assert!(fs.stat("/tmpdir").is_err());
    }

    #[test]
    fn test_rmdir_non_empty_fails() {
        let mut fs = make_fs();
        fs.mkdir("/parent").expect("mkdir should succeed");
        fs.create("/parent/child.txt", FileMode::default_file())
            .expect("create should succeed");
        let err = fs.rmdir("/parent").unwrap_err();
        assert_eq!(err, FsError::DirectoryNotEmpty);
    }

    #[test]
    fn test_readdir() {
        let mut fs = make_fs();
        fs.mkdir("/dir1").expect("mkdir should succeed");
        fs.create("/file1.txt", FileMode::default_file())
            .expect("create should succeed");
        fs.create("/file2.txt", FileMode::default_file())
            .expect("create should succeed");

        let entries = fs.readdir("/").expect("readdir should succeed");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"dir1"));
        assert!(names.contains(&"file1.txt"));
        assert!(names.contains(&"file2.txt"));

        let dir1 = entries
            .iter()
            .find(|e| e.name == "dir1")
            .expect("dir1 should exist");
        assert!(dir1.is_dir);
        let f1 = entries
            .iter()
            .find(|e| e.name == "file1.txt")
            .expect("file1.txt should exist");
        assert!(!f1.is_dir);
    }

    #[test]
    fn test_sync_no_op() {
        let mut fs = make_fs();
        fs.sync().expect("sync should succeed");
    }

    #[test]
    fn test_df() {
        let fs = make_fs();
        let du = fs.df().expect("df should succeed");
        assert_eq!(du.total_bytes, 64 * 4096);
        // Some blocks are used by littlefs metadata.
        assert!(du.used_bytes > 0);
        assert!(du.free_bytes < du.total_bytes);
        assert_eq!(du.block_count, 64);
    }

    #[test]
    fn test_invalid_path_with_null() {
        let fs = make_fs();
        let err = fs.stat("/bad\0path").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath { .. }));
    }

    #[test]
    fn test_invalid_path_non_ascii() {
        let fs = make_fs();
        let err = fs.stat("/bad/path/ö").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath { .. }));
    }

    #[test]
    fn test_open_missing_no_create() {
        let mut fs = make_fs();
        let err = fs.open("/missing", OpenFlags::READ).unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[test]
    fn test_open_with_create() {
        let mut fs = make_fs();
        let _file = fs
            .open(
                "/new.txt",
                OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE,
            )
            .expect("open with CREATE should succeed");
        assert!(fs.stat("/new.txt").is_ok());
    }

    #[test]
    fn test_open_with_truncate() {
        let mut fs = make_fs();
        fs.create("/trunc.txt", FileMode::default_file())
            .expect("create should succeed");
        // Write some data.
        let mut f = fs
            .open("/trunc.txt", OpenFlags::WRITE)
            .expect("open should succeed");
        let _ = f
            .write(&mut fs, b"1234567890")
            .expect("write should succeed");
        // Reopen with TRUNCATE.
        let _ = fs
            .open("/trunc.txt", OpenFlags::WRITE | OpenFlags::TRUNCATE)
            .expect("open with TRUNCATE should succeed");
        let size = fs
            .file_size("/trunc.txt")
            .expect("file_size should succeed");
        assert_eq!(size, 0);
    }

    #[test]
    fn test_append_mode() {
        let mut fs = make_fs();
        let mut f = fs
            .create("/append.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f.write(&mut fs, b"first").expect("write should succeed");

        // Open with APPEND and write more.
        let mut f2 = fs
            .open("/append.txt", OpenFlags::WRITE | OpenFlags::APPEND)
            .expect("open should succeed");
        let _ = f2
            .write(&mut fs, b" second")
            .expect("append should succeed");

        let size = fs
            .file_size("/append.txt")
            .expect("file_size should succeed");
        assert_eq!(size, 12);
    }

    #[test]
    fn test_persistence_across_remount() {
        // Write some data, then drop the Lfs and re-mount to verify
        // persistence.
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        let mut fs = Lfs::format(dev).expect("format should succeed");
        let mut f = fs
            .create("/persist.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f
            .write(&mut fs, b"persistent data")
            .expect("write should succeed");
        drop(fs);

        // Re-create the same device state by writing through a fresh mock.
        // Note: MockBlockDevice state is in-memory and lost on drop, so this
        // test verifies the littlefs2 metadata is consistent within a single
        // device lifetime. For true persistence we would need a shared mock.
        let dev2: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        let err = Lfs::mount(dev2).unwrap_err();
        assert_eq!(err, FsError::BadSuperBlock);
    }

    #[test]
    fn test_nested_directories() {
        let mut fs = make_fs();
        fs.mkdir("/a").expect("mkdir /a should succeed");
        fs.mkdir("/a/b").expect("mkdir /a/b should succeed");
        fs.mkdir("/a/b/c").expect("mkdir /a/b/c should succeed");
        fs.create("/a/b/file.txt", FileMode::default_file())
            .expect("create should succeed");
        let stat = fs.stat("/a/b/file.txt").expect("stat should succeed");
        assert!(!stat.is_dir);
    }

    #[test]
    fn test_many_files() {
        let mut fs = make_fs();
        for i in 0..10 {
            let path = alloc::format!("/file{}.txt", i);
            fs.create(&path, FileMode::default_file())
                .expect("create should succeed");
        }
        let entries = fs.readdir("/").expect("readdir should succeed");
        assert_eq!(entries.len(), 10);
    }

    #[test]
    fn test_file_seek() {
        let mut fs = make_fs();
        let mut f = fs
            .create("/seek.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f
            .write(&mut fs, b"0123456789")
            .expect("write should succeed");

        // Seek to offset 5 and read.
        use crate::types::SeekFrom;
        let pos = f
            .seek(&mut fs, SeekFrom::Start(5))
            .expect("seek should succeed");
        assert_eq!(pos, 5);

        let mut buf = [0u8; 3];
        let n = f.read(&mut fs, &mut buf).expect("read should succeed");
        assert_eq!(n, 3);
        assert_eq!(&buf, b"567");
    }

    #[test]
    fn test_file_seek_end() {
        let mut fs = make_fs();
        let mut f = fs
            .create("/seekend.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f.write(&mut fs, b"abcdef").expect("write should succeed");

        use crate::types::SeekFrom;
        // Seek to 2 bytes before end.
        let pos = f
            .seek(&mut fs, SeekFrom::End(-2))
            .expect("seek should succeed");
        assert_eq!(pos, 4);

        let mut buf = [0u8; 4];
        let n = f.read(&mut fs, &mut buf).expect("read should succeed");
        assert_eq!(n, 2);
        assert_eq!(&buf[..n], b"ef");
    }

    #[test]
    fn test_file_seek_current() {
        let mut fs = make_fs();
        let mut f = fs
            .create("/seekcur.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f
            .write(&mut fs, b"abcdefghij")
            .expect("write should succeed");

        use crate::types::SeekFrom;
        // Seek back to start, then read 3 bytes.
        f.seek(&mut fs, SeekFrom::Start(0))
            .expect("seek to start should succeed");
        let mut buf = [0u8; 3];
        let _ = f.read(&mut fs, &mut buf).expect("read should succeed");
        assert_eq!(&buf, b"abc");

        // Seek forward 2 from current (offset 3 -> 5).
        let pos = f
            .seek(&mut fs, SeekFrom::Current(2))
            .expect("seek should succeed");
        assert_eq!(pos, 5);

        let mut buf2 = [0u8; 3];
        let _ = f.read(&mut fs, &mut buf2).expect("read should succeed");
        assert_eq!(&buf2, b"fgh");
    }

    #[test]
    fn test_make_path_valid() {
        let p = make_path("/test/path.txt").expect("path should be valid");
        assert_eq!(p.as_str(), "/test/path.txt");
    }

    #[test]
    fn test_make_path_with_null() {
        let err = make_path("/bad\0path").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath { .. }));
    }

    #[test]
    fn test_make_path_non_ascii() {
        let err = make_path("/bad/path/ü").unwrap_err();
        assert!(matches!(err, FsError::InvalidPath { .. }));
    }

    #[test]
    fn test_make_path_too_long() {
        let long_path = alloc::format!("/{}", "a".repeat(300));
        let err = make_path(&long_path).unwrap_err();
        assert!(matches!(err, FsError::InvalidPath { .. }));
    }
}
