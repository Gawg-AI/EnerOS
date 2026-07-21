//! Configuration for [`Lfs`](crate::Lfs) filesystem instances.
//!
//! [`LfsConfig`] holds tunable parameters that govern the littlefs2-backed
//! filesystem. Most fields mirror the underlying littlefs2 compile-time
//! constants exposed by [`BlockDeviceStorage`](crate::lfs::BlockDeviceStorage),
//! but stored at runtime so a single binary can mount filesystems of different
//! geometries (e.g. test vs. production flash).
//!
//! # Defaults
//!
//! The [`Default`] implementation matches the geometry compiled into
//! [`BlockDeviceStorage`](crate::lfs::BlockDeviceStorage):
//!
//! | Field           | Default | Note                                  |
//! |-----------------|---------|---------------------------------------|
//! | `block_size`    | 4096    | Must match `Storage::BLOCK_SIZE`      |
//! | `segment_size`  | 64      | Blocks per segment (also `BLOCK_COUNT`) |
//! | `cache_size`    | 4096    | One-block cache                       |
//! | `lookahead`     | 8       | 8 × 8 = 64-byte lookahead bitmap      |
//! | `max_open_files`| 16      | Soft limit enforced by the upper layer |

/// Configuration for an [`Lfs`](crate::Lfs) instance.
///
/// Construct manually or via [`LfsConfig::default`] for the standard
/// geometry baked into [`BlockDeviceStorage`](crate::lfs::BlockDeviceStorage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LfsConfig {
    /// Block size in bytes. Must match `BlockDeviceStorage::BLOCK_SIZE` (4096).
    pub block_size: usize,

    /// Number of blocks in one segment. The default of 64 matches
    /// `BlockDeviceStorage::BLOCK_COUNT` (256 KB total) and is suitable for
    /// host tests. Production deployments on larger flash parts should set
    /// this to the actual segment size of the device.
    pub segment_size: u32,

    /// Filesystem cache size in bytes. Must be a multiple of `block_size`
    /// and match `BlockDeviceStorage::CACHE_SIZE` (4096).
    pub cache_size: usize,

    /// Lookahead window in blocks (8 × 8 = 64-byte bitmap). Must match
    /// `BlockDeviceStorage::LOOKAHEAD_SIZE` (8).
    pub lookahead: u32,

    /// Maximum number of simultaneously open files. This is a soft limit
    /// enforced by the upper layer (the value-type `File` design does not
    /// hold littlefs2 handles, so this is informational only).
    pub max_open_files: usize,
}

impl Default for LfsConfig {
    fn default() -> Self {
        Self {
            block_size: 4096,
            segment_size: 64,
            cache_size: 4096,
            lookahead: 8,
            max_open_files: 16,
        }
    }
}

impl LfsConfig {
    /// Creates a new `LfsConfig` with the given block size and segment size,
    /// using defaults for the remaining fields.
    pub const fn new(block_size: usize, segment_size: u32) -> Self {
        Self {
            block_size,
            segment_size,
            cache_size: block_size,
            lookahead: 8,
            max_open_files: 16,
        }
    }

    /// Returns the total filesystem capacity in bytes (`block_size *
    /// segment_size`).
    pub fn total_bytes(&self) -> u64 {
        self.block_size as u64 * self.segment_size as u64
    }

    /// Returns `true` if the configuration is internally consistent with the
    /// compile-time geometry of [`BlockDeviceStorage`](crate::lfs::BlockDeviceStorage).
    pub fn is_compatible_with_storage(&self) -> bool {
        use littlefs2::driver::Storage;

        use crate::lfs::storage_adapter::BlockDeviceStorage;
        self.block_size == BlockDeviceStorage::BLOCK_SIZE
            && self.cache_size == BlockDeviceStorage::READ_SIZE
            && self.segment_size as usize <= BlockDeviceStorage::BLOCK_COUNT
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
    fn test_default_values() {
        let c = LfsConfig::default();
        assert_eq!(c.block_size, 4096);
        assert_eq!(c.segment_size, 64);
        assert_eq!(c.cache_size, 4096);
        assert_eq!(c.lookahead, 8);
        assert_eq!(c.max_open_files, 16);
    }

    #[test]
    fn test_new_constructor() {
        let c = LfsConfig::new(4096, 128);
        assert_eq!(c.block_size, 4096);
        assert_eq!(c.segment_size, 128);
        assert_eq!(c.cache_size, 4096);
        assert_eq!(c.lookahead, 8);
        assert_eq!(c.max_open_files, 16);
    }

    #[test]
    fn test_total_bytes() {
        let c = LfsConfig::default();
        assert_eq!(c.total_bytes(), 4096 * 64);
        assert_eq!(c.total_bytes(), 262_144);
    }

    #[test]
    fn test_total_bytes_large() {
        let c = LfsConfig::new(4096, 65536);
        assert_eq!(c.total_bytes(), 4096 * 65536);
        assert_eq!(c.total_bytes(), 268_435_456); // 256 MB
    }

    #[test]
    fn test_is_compatible_with_storage_default() {
        let c = LfsConfig::default();
        assert!(c.is_compatible_with_storage());
    }

    #[test]
    fn test_is_compatible_with_storage_wrong_block_size() {
        let c = LfsConfig {
            block_size: 512,
            ..LfsConfig::default()
        };
        assert!(!c.is_compatible_with_storage());
    }

    #[test]
    fn test_is_compatible_with_storage_too_large_segment() {
        let c = LfsConfig {
            segment_size: 1024, // exceeds BLOCK_COUNT=64
            ..LfsConfig::default()
        };
        assert!(!c.is_compatible_with_storage());
    }

    #[test]
    fn test_is_compatible_with_storage_wrong_cache_size() {
        let c = LfsConfig {
            cache_size: 2048,
            ..LfsConfig::default()
        };
        assert!(!c.is_compatible_with_storage());
    }

    #[test]
    fn test_clone_copy() {
        let c = LfsConfig::default();
        let c2 = c; // Copy
        assert_eq!(c, c2);

        let c3 = c;
        assert_eq!(c2, c3);
    }

    #[test]
    fn test_equality() {
        let a = LfsConfig::default();
        let b = LfsConfig::default();
        assert_eq!(a, b);

        let c = LfsConfig::new(4096, 128);
        assert_ne!(a, c);
    }

    #[test]
    fn test_debug_format() {
        let c = LfsConfig::default();
        let s = format!("{:?}", c);
        assert!(s.contains("LfsConfig"));
        assert!(s.contains("block_size"));
        assert!(s.contains("4096"));
    }
}
