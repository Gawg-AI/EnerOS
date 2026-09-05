//! [`Lfs`] — the littlefs2-backed [`FileSystem`] implementation.
//!
//! `Lfs` wraps a littlefs2 [`Filesystem`] held in a **persistent mount**
//! (蓝图 68 卷 68.3 WP-68.3.2)：构造（format/mount）时挂载一次，此后
//! 全部操作直接走已挂载实例——不再每操作 `mount_and_then` 重挂载。
//!
//! # Design（持久 mount 的所有权形态）
//!
//! littlefs2 0.8 的 `Filesystem<'a, Storage>` 持有 `&'a mut Allocation`
//! 与 `&'a mut Storage`，且 mount 后 C 侧指针指入两者（`set_alloc_config`
//! 写 cfg.context→storage、read/prog/lookahead→alloc.cache；lfs_t.cfg→
//! &alloc.config）——**mount 之后两者不可再移动**。本实现的时序：
//! 1. `BlockDeviceStorage` / `Allocation` 各自装箱后 `Box::leak` 落定
//!    最终堆地址（构造期指针全 null，装箱移动安全）；
//! 2. 在最终地址上 `Filesystem::mount`，得 `Filesystem<'static>`；
//! 3. `Filesystem<'static>` 作为普通字段存入 Lfs——Lfs 本体保持可移动。
//!
//! 泄漏有界性（内存审计，执行卡 WP-68.3.0）：每个 Lfs 实例泄漏
//! sizeof(Allocation)+sizeof(BlockDeviceStorage)（≈8.5KB，cache 4096×2
//! 为主）；服务生命周期内常驻实例=1（构造一次）；唯一例外 =
//! mount-or-format 双 face（如 telemetry_history open_disk_history）——
//! mount 探测失败的首次泄漏一对（≈8.5KB，每卷一次性，之后恒 mount 成功），
//! host 测试按进程回收。littlefs2 0.8 无 unmount/Drop API（旧
//! mount_and_then 闭包结束同样不 unmount）——耐久语义 = 逐文件 close
//! sync，持久形态逐操作等价。
//!
//! Read-only trait methods (`stat`, `readdir`, `df`) take `&self`;
//! littlefs2 操作全部取 `&Filesystem`，无需内部可变性。
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
use core::fmt;

use eneros_storage::ScopedBlockDevice;
use littlefs2::fs::{Allocation, Filesystem, OpenOptions};
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
/// (mount an already-formatted device). 持久 mount：挂载发生在构造期，
/// 之后操作零重挂载（WP-68.3.2）。
pub struct Lfs {
    /// 持久挂载的 littlefs2 实例（'static 借用指向泄漏的 Allocation/
    /// Storage；见模块文档的时序与泄漏纪律）。
    fs: Filesystem<'static, BlockDeviceStorage>,
    config: LfsConfig,
    /// 底层设备实际块数（≤ `BlockDeviceStorage::BLOCK_COUNT`；构造期
    /// 捕获——storage 已归 Filesystem 独占借用，常规路径不可再达）。
    block_count: usize,
}

impl fmt::Debug for Lfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lfs")
            .field("config", &self.config)
            .field("block_count", &self.block_count)
            .finish()
    }
}

/// 落定 storage 到最终堆地址并预检（68.3 持久 mount 时序第一步）。
///
/// `probe`：mount 前置校验（None = format 路径免检；Some(f) = 先跑
/// 校验、失败返回其错误）。
fn leak_storage(
    device: Box<dyn ScopedBlockDevice>,
    probe: Option<fn(&mut BlockDeviceStorage) -> bool>,
) -> Result<(&'static mut BlockDeviceStorage, usize), FsError> {
    let storage = BlockDeviceStorage::new(device)?;
    let blocks = storage.device_block_count();
    let storage: &'static mut BlockDeviceStorage = Box::leak(Box::new(storage));
    if let Some(check) = probe {
        if !check(storage) {
            return Err(FsError::BadSuperBlock);
        }
    }
    Ok((storage, blocks))
}

/// 在已落定地址上完成持久 mount（第二步；泄漏纪律见模块文档）。
fn mount_at(
    storage: &'static mut BlockDeviceStorage,
    block_count: usize,
    config: LfsConfig,
) -> Result<Lfs, FsError> {
    let alloc: &'static mut Allocation<BlockDeviceStorage> = Box::leak(Box::new(Allocation::new()));
    let fs = Filesystem::mount(alloc, storage).map_err(FsError::from)?;
    Ok(Lfs {
        fs,
        config,
        block_count,
    })
}

impl Lfs {
    /// Formats the block device and returns a mounted `Lfs`.
    ///
    /// This is equivalent to `format` followed by `mount`. Use on a fresh
    /// device or to wipe an existing filesystem.
    ///
    /// 39 收尾复验（Exit Gate #2）：只接受角色分区作用域；格式化
    /// 能力受作用域来源约束（生产路径经 ProvisioningGate 授权后
    /// 的分区句柄）。
    pub fn format(device: Box<dyn ScopedBlockDevice>) -> Result<Self, FsError> {
        Self::format_with_config(device, LfsConfig::default())
    }

    /// Formats the block device with the given configuration.
    pub fn format_with_config(
        device: Box<dyn ScopedBlockDevice>,
        config: LfsConfig,
    ) -> Result<Self, FsError> {
        let (storage, blocks) = leak_storage(device, None)?;
        Filesystem::format(storage).map_err(FsError::from)?;
        mount_at(storage, blocks, config)
    }

    /// Mounts an already-formatted filesystem.
    ///
    /// Returns [`FsError::BadSuperBlock`] if the device is not formatted.
    ///
    /// 39 收尾复验（Exit Gate #2）：只接受角色分区作用域。
    pub fn mount(device: Box<dyn ScopedBlockDevice>) -> Result<Self, FsError> {
        Self::mount_with_config(device, LfsConfig::default())
    }

    /// Mounts an already-formatted filesystem with the given configuration.
    pub fn mount_with_config(
        device: Box<dyn ScopedBlockDevice>,
        config: LfsConfig,
    ) -> Result<Self, FsError> {
        // Verify mountability; if this fails the superblock is missing/invalid.
        let (storage, blocks) = leak_storage(device, Some(Filesystem::is_mountable))?;
        mount_at(storage, blocks, config)
    }

    /// Returns the active configuration.
    pub fn config(&self) -> &LfsConfig {
        &self.config
    }

    /// Returns the block count of the underlying device.
    pub fn block_count(&self) -> usize {
        self.block_count
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
        OpenOptions::new()
            .read(true)
            .open_and_then(&self.fs, &lfs_path, |file| {
                if offset > 0 {
                    file.seek(LfsSeekFrom::Start(offset as u32))?;
                }
                file.read(buf)
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
        OpenOptions::new()
            .read(true)
            .write(true)
            .open_and_then(&self.fs, &lfs_path, |file| {
                if offset > 0 {
                    file.seek(LfsSeekFrom::Start(offset as u32))?;
                }
                file.write(buf)
            })
            .map_err(FsError::from)
    }

    /// Returns the size of the file at `path` in bytes.
    pub(crate) fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let lfs_path = make_path(path)?;
        let size = OpenOptions::new()
            .read(true)
            .open_and_then(&self.fs, &lfs_path, |file| file.len())
            .map_err(FsError::from)?;
        Ok(size as u64)
    }

    /// Truncates (or extends) the file at `path` to `size` bytes.
    pub(crate) fn truncate_file(&self, path: &str, size: u64) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        OpenOptions::new()
            .write(true)
            .open_and_then(&self.fs, &lfs_path, |file| file.set_len(size as usize))
            .map_err(FsError::from)
    }
}

impl FileSystem for Lfs {
    fn open(&mut self, path: &str, flags: OpenFlags) -> Result<File, FsError> {
        let lfs_path = make_path(path)?;

        if flags.is_create() {
            // CREATE / CREATE_NEW: create the file on disk so subsequent
            // stat() calls succeed even before the first write.
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
            opts.open_and_then(&self.fs, &lfs_path, |_file| Ok(()))
                .map_err(FsError::from)?;
        } else {
            // Non-create path: verify the file exists.
            let exists = match self.fs.metadata(&lfs_path) {
                Ok(_) => true,
                Err(littlefs2::io::Error::NO_SUCH_ENTRY) => false,
                Err(e) => return Err(FsError::from(e)),
            };
            if !exists {
                return Err(FsError::NotFound {
                    path: String::from(path),
                });
            }
            if flags.is_truncate() && flags.is_write() {
                OpenOptions::new()
                    .write(true)
                    .open_and_then(&self.fs, &lfs_path, |file| file.set_len(0))
                    .map_err(FsError::from)?;
            }
        }
        Ok(File::new(String::from(path), flags))
    }

    fn create(&mut self, path: &str, _mode: FileMode) -> Result<File, FsError> {
        let lfs_path = make_path(path)?;
        // Use littlefs2's create semantics: open with create+truncate.
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open_and_then(&self.fs, &lfs_path, |_file| Ok(()))
            .map_err(FsError::from)?;
        Ok(File::new(
            String::from(path),
            OpenFlags::READ | OpenFlags::WRITE,
        ))
    }

    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        self.fs.remove(&lfs_path).map_err(FsError::from)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let lfs_from = make_path(from)?;
        let lfs_to = make_path(to)?;
        self.fs.rename(&lfs_from, &lfs_to).map_err(FsError::from)
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        let lfs_path = make_path(path)?;
        let metadata = self.fs.metadata(&lfs_path).map_err(FsError::from)?;
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
        self.fs.create_dir(&lfs_path).map_err(FsError::from)
    }

    fn rmdir(&mut self, path: &str) -> Result<(), FsError> {
        let lfs_path = make_path(path)?;
        self.fs.remove_dir(&lfs_path).map_err(FsError::from)
    }

    fn readdir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let lfs_path = make_path(path)?;
        let mut out: Vec<DirEntry> = Vec::new();
        self.fs
            .read_dir_and_then(&lfs_path, |read_dir| {
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
            })
            .map_err(FsError::from)?;
        Ok(out)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        // 68.3 持久 mount 语义订正：littlefs2 的落盘点是逐文件 close
        // （每个值类型 File 操作在 open_and_then 闭包内 close+sync）——
        // 本调用无独立脏状态可冲刷。旧实现注释声称 "mount/unmount cycle
        // is the natural sync point" 不实（littlefs2 0.8 无 unmount，
        // mount_and_then 闭包结束只是丢弃实例）；持久形态下耐久保证与
        // 旧形态逐操作等价（WP-68.3.2）。
        Ok(())
    }

    fn df(&self) -> Result<DiskUsage, FsError> {
        let total_bytes = self.fs.total_space();
        let free_bytes = self.fs.available_space().map_err(FsError::from)?;
        let block_count = self.fs.total_blocks();
        let used_bytes = total_bytes.saturating_sub(free_bytes);
        Ok(DiskUsage {
            total_bytes: total_bytes as u64,
            used_bytes: used_bytes as u64,
            free_bytes: free_bytes as u64,
            block_count: block_count as u64,
        })
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, FsError> {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        let mut offset = 0u64;
        loop {
            let n = self.read_file_at(path, offset, &mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            offset += n as u64;
        }
        Ok(out)
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsError> {
        // 29 DR-01（方案 B）：tmp+rename 原子写——先写 `<path>.tmp`
        // （create=true，空文件），全量写入后再 rename 到目标；掉电不丢
        // 旧数据（旧文件在 rename 前保持完整）。
        // 原实现 `remove(path)` 后 `write_file_at`（open 无 create）→ 新
        // 路径首写必失败（NO_SUCH_ENTRY）；测试盲区根因：fs 泛型测试全走
        // MockFileSystem（内存版正常），真实 littlefs2 路径从未覆盖。
        let tmp_path = alloc::format!("{path}.tmp");
        // 清理上次崩溃残留的 tmp。
        let _ = self.remove(&tmp_path);
        self.create(&tmp_path, FileMode::default_file())?;
        self.write_file_at(&tmp_path, 0, data)?;
        // 原子替换目标（littlefs2 rename 覆盖语义；先清目标防 rename 语义
        // 差异——崩溃时 tmp 保留，旧数据在 remove 前仍完整）。
        let _ = self.remove(path);
        self.rename(&tmp_path, path)
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

    use eneros_storage::partition::{boxed_mock_scope, BlockRole};
    use eneros_storage::MockBlockDevice;

    use super::*;

    fn data_scope(blocks: u64) -> Box<dyn ScopedBlockDevice> {
        boxed_mock_scope(MockBlockDevice::new(blocks, 4096), BlockRole::Data)
    }

    fn make_fs() -> Lfs {
        let dev = data_scope(64);
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
        let dev = data_scope(64);
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

    /// 29 DR-01 判据：真实 littlefs2 + RAM 后端，空文件系统新路径首写
    /// 必须成功且可读回——修复前 remove 后 write_file_at（open 无 create）
    /// 必 NO_SUCH_ENTRY 失败（MockFileSystem 内存版正常，测试盲区根因）。
    #[test]
    fn test_write_file_new_path_first_write_ok() {
        let mut fs = make_fs();
        fs.write_file("/a.txt", b"x")
            .expect("新路径首写必须成功（29 DR-01 判据）");
        let back = fs.read_file("/a.txt").expect("read back");
        assert_eq!(back, b"x");
    }

    /// 29 DR-01：覆盖写（目标已存在）保持旧语义——全量替换 + 可读回；
    /// tmp 文件不得残留。
    #[test]
    fn test_write_file_overwrite_no_tmp_left() {
        let mut fs = make_fs();
        fs.write_file("/b.txt", b"old-content")
            .expect("first write");
        fs.write_file("/b.txt", b"new").expect("overwrite");
        let back = fs.read_file("/b.txt").expect("read back");
        assert_eq!(back, b"new");
        assert!(fs.stat("/b.txt.tmp").is_err(), "tmp 文件必须清理");
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
        let dev = data_scope(64);
        let mut fs = Lfs::format(dev).expect("format should succeed");
        let mut f = fs
            .create("/persist.txt", FileMode::default_file())
            .expect("create should succeed");
        let _ = f
            .write(&mut fs, b"persistent data")
            .expect("write should succeed");
        // fs 就此弃置（Lfs 无 Drop；mock 介质随进程，语义同旧 drop(fs)）。

        // Re-create the same device state by writing through a fresh mock.
        // Note: MockBlockDevice state is in-memory and lost on drop, so this
        // test verifies the littlefs2 metadata is consistent within a single
        // device lifetime. For true persistence we would need a shared mock.
        let dev2 = data_scope(64);
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

    // ========================================================================
    // 68.3 WP-68.3.2：持久 mount 形态判据
    // ========================================================================

    use std::cell::Cell as StdCell;
    use std::rc::Rc as StdRc;

    /// 共享介质块设备（测试）：纯字节面 + block-0 读计数（mount 扫描的
    /// 观测口——littlefs2 mount 必读超级块所在块 0）。Clone 共享介质与
    /// 计数器（掉电重挂载 = 同介质新实例）。
    #[derive(Clone)]
    struct SharedMedia {
        data: StdRc<std::cell::RefCell<std::vec::Vec<u8>>>,
        block0_reads: StdRc<StdCell<u64>>,
        reads: StdRc<StdCell<u64>>,
        blocks: u64,
    }

    impl SharedMedia {
        fn new(blocks: u64) -> Self {
            Self {
                data: StdRc::new(std::cell::RefCell::new(vec![0u8; (blocks * 4096) as usize])),
                block0_reads: StdRc::new(StdCell::new(0)),
                reads: StdRc::new(StdCell::new(0)),
                blocks,
            }
        }
        fn reads(&self) -> u64 {
            self.reads.get()
        }
    }

    impl eneros_storage::BlockDevice for SharedMedia {
        fn read_block(
            &self,
            block_idx: u64,
            buf: &mut [u8],
        ) -> Result<(), eneros_storage::StorageError> {
            if block_idx >= self.blocks {
                return Err(eneros_storage::StorageError::OutOfRange {
                    block_idx,
                    max: self.blocks,
                });
            }
            self.reads.set(self.reads.get() + 1);
            if block_idx == 0 {
                self.block0_reads.set(self.block0_reads.get() + 1);
            }
            let off = (block_idx * 4096) as usize;
            let d = self.data.borrow();
            buf.copy_from_slice(&d[off..off + buf.len()]);
            Ok(())
        }
        fn write_block(
            &mut self,
            block_idx: u64,
            buf: &[u8],
        ) -> Result<(), eneros_storage::StorageError> {
            if block_idx >= self.blocks {
                return Err(eneros_storage::StorageError::OutOfRange {
                    block_idx,
                    max: self.blocks,
                });
            }
            let off = (block_idx * 4096) as usize;
            let mut d = self.data.borrow_mut();
            d[off..off + buf.len()].copy_from_slice(buf);
            Ok(())
        }
        fn erase_block(&mut self, block_idx: u64) -> Result<(), eneros_storage::StorageError> {
            if block_idx >= self.blocks {
                return Err(eneros_storage::StorageError::OutOfRange {
                    block_idx,
                    max: self.blocks,
                });
            }
            let off = (block_idx * 4096) as usize;
            let mut d = self.data.borrow_mut();
            d[off..off + 4096].fill(0);
            Ok(())
        }
        fn block_count(&self) -> u64 {
            self.blocks
        }
        fn block_size(&self) -> usize {
            4096
        }
        fn flush(&mut self) -> Result<(), eneros_storage::StorageError> {
            Ok(())
        }
        fn health_status(&self) -> eneros_storage::driver::DeviceHealth {
            eneros_storage::driver::DeviceHealth {
                total_blocks: self.blocks,
                bad_blocks: 0,
                wear_level: 0,
                temperature: 0,
                remaining_life: 100,
            }
        }
    }

    /// 操作成本有界（mount 消除的观测口）：持久形态下读操作只剩
    /// littlefs 自身的元数据对扫描（read_file=4、stat=2 块读——根目录
    /// 对即超级块对 0/1，逐操作触碰属正常）；每操作重挂载的旧形态会
    /// 叠加 mount 探测 + 挂载扫描（构造期实测 ≥3 块读/次）。以
    /// "每操作块读 ≤ 6" 锁持久形态（WP-68.3.2 性能计数判据；littlefs
    /// 内部缓存策略变化时按新事实重标）。
    #[test]
    fn persistent_form_op_costs_are_bounded() {
        let media = SharedMedia::new(64);
        let mut fs =
            Lfs::format(boxed_mock_scope(media.clone(), BlockRole::Data)).expect("format+mount");
        assert!(
            media.reads() >= 1,
            "构造期 mount 必须读超级块（观测口自检）"
        );
        fs.write_file("/a.txt", b"hello").expect("write");
        // 读操作逐次计数。
        for _ in 0..3 {
            let before = media.reads();
            assert_eq!(fs.read_file("/a.txt").expect("read"), b"hello");
            let per_op = media.reads() - before;
            assert!(per_op <= 6, "read_file 每操作块读必须有界（实得 {per_op}）");
        }
        let before = media.reads();
        let _ = fs.stat("/a.txt").expect("stat");
        assert!(media.reads() - before <= 4, "stat 每操作块读必须有界");
    }

    /// 持久形态掉电语义：mount 一次 × N 操作 × 弃实例（掉电）× 同介质
    /// 重挂载（走 mount 非 format）——数据完整（fs 层 c3 同款）。
    #[test]
    fn persistent_form_power_loss_remount_preserves_data() {
        let media = SharedMedia::new(64);
        {
            let mut fs = Lfs::format(boxed_mock_scope(media.clone(), BlockRole::Data))
                .expect("format+mount");
            for i in 0..5u32 {
                let path = alloc::format!("/f{i}.txt");
                fs.write_file(&path, alloc::format!("payload-{i}").as_bytes())
                    .expect("write");
            }
            // —— 掉电：drop Lfs（介质留存）——
        }
        // —— 上电：mount（非 format）读回全部数据 ——
        let mut fs2 = Lfs::mount(boxed_mock_scope(media, BlockRole::Data))
            .expect("remount must mount (not reformat)");
        for i in 0..5u32 {
            let path = alloc::format!("/f{i}.txt");
            assert_eq!(
                fs2.read_file(&path).expect("read back"),
                alloc::format!("payload-{i}").as_bytes(),
                "掉电后文件 {i} 必须完整"
            );
        }
    }
}
