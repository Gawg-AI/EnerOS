//! Configuration version management with CRC32 integrity checking.
//!
//! Each save/set operation records a [`ConfigVersion`] containing the
//! serialized data, a CRC32 checksum, and a timestamp. [`VersionHistory`]
//! stores up to `MAX_VERSIONS` entries and supports rollback.

use alloc::vec::Vec;

use crc32fast::Hasher as Crc32Hasher;

/// Maximum number of versions retained per config file.
pub const MAX_VERSIONS: usize = 10;

/// A single configuration version snapshot.
#[derive(Debug, Clone)]
pub struct ConfigVersion {
    /// Monotonically increasing version number.
    pub version: u64,
    /// Timestamp (epoch seconds, from injected time source).
    pub timestamp: u64,
    /// Serialized configuration data.
    pub data: Vec<u8>,
    /// CRC32 checksum of `data` for integrity verification.
    pub crc32: u32,
}

impl ConfigVersion {
    /// Creates a new version snapshot, computing the CRC32 of `data`.
    pub fn new(version: u64, timestamp: u64, data: Vec<u8>) -> Self {
        let crc32 = compute_crc32(&data);
        Self {
            version,
            timestamp,
            data,
            crc32,
        }
    }

    /// Verifies that the stored CRC32 matches the data.
    ///
    /// Returns `true` if the data is intact, `false` if corrupted.
    pub fn verify(&self) -> bool {
        compute_crc32(&self.data) == self.crc32
    }
}

/// Computes the CRC32 checksum of `data`.
pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Version history for a single configuration file.
///
/// Stores up to [`MAX_VERSIONS`] entries. When full, the oldest entry is
/// evicted.
#[derive(Debug, Default)]
pub struct VersionHistory {
    /// Version entries, ordered by version number ascending.
    versions: Vec<ConfigVersion>,
    /// The next version number to assign.
    next_version: u64,
}

impl VersionHistory {
    /// Creates a new empty history.
    pub fn new() -> Self {
        Self {
            versions: Vec::new(),
            next_version: 1,
        }
    }

    /// Records a new version snapshot and returns the assigned version number.
    ///
    /// If the history is full, the oldest entry is evicted.
    pub fn record(&mut self, timestamp: u64, data: Vec<u8>) -> u64 {
        let version = self.next_version;
        self.next_version = self.next_version.saturating_add(1);
        let entry = ConfigVersion::new(version, timestamp, data);
        if self.versions.len() >= MAX_VERSIONS {
            self.versions.remove(0);
        }
        self.versions.push(entry);
        version
    }

    /// Returns the version snapshot with the given version number.
    pub fn get(&self, version: u64) -> Option<&ConfigVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// Returns the current (latest) version number, or `None` if empty.
    pub fn current_version(&self) -> Option<u64> {
        self.versions.last().map(|v| v.version)
    }

    /// Lists all version numbers in ascending order.
    pub fn list_versions(&self) -> Vec<u64> {
        self.versions.iter().map(|v| v.version).collect()
    }

    /// Returns the number of stored versions.
    pub fn len(&self) -> usize {
        self.versions.len()
    }

    /// Returns `true` if the history is empty.
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    // ---- compute_crc32 ----

    #[test]
    fn test_compute_crc32_empty() {
        // CRC32 of empty input is a well-known constant.
        assert_eq!(compute_crc32(b""), 0);
    }

    #[test]
    fn test_compute_crc32_known_value() {
        // CRC32 of "123456789" is 0xCBF43926 (a well-known test vector).
        assert_eq!(compute_crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn test_compute_crc32_different_inputs_differ() {
        let a = compute_crc32(b"hello");
        let b = compute_crc32(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn test_compute_crc32_deterministic() {
        let a = compute_crc32(b"some config data");
        let b = compute_crc32(b"some config data");
        assert_eq!(a, b);
    }

    // ---- ConfigVersion::new ----

    #[test]
    fn test_config_version_new_sets_fields() {
        let data = vec![1u8, 2, 3, 4];
        let v = ConfigVersion::new(7, 1000, data.clone());
        assert_eq!(v.version, 7);
        assert_eq!(v.timestamp, 1000);
        assert_eq!(v.data, data);
        // CRC32 should be the checksum of the data.
        assert_eq!(v.crc32, compute_crc32(&data));
    }

    #[test]
    fn test_config_version_new_empty_data() {
        let v = ConfigVersion::new(1, 0, Vec::new());
        assert_eq!(v.version, 1);
        assert!(v.data.is_empty());
        assert_eq!(v.crc32, 0);
    }

    #[test]
    fn test_config_version_new_computes_crc() {
        // Same data should produce the same CRC.
        let data = b"port = 8080\n".to_vec();
        let v1 = ConfigVersion::new(1, 100, data.clone());
        let v2 = ConfigVersion::new(2, 200, data);
        assert_eq!(v1.crc32, v2.crc32);
    }

    // ---- ConfigVersion::verify ----

    #[test]
    fn test_config_version_verify_intact() {
        let data = b"hello world".to_vec();
        let v = ConfigVersion::new(1, 0, data);
        assert!(v.verify());
    }

    #[test]
    fn test_config_version_verify_corrupted_data() {
        let data = b"hello world".to_vec();
        let mut v = ConfigVersion::new(1, 0, data);
        // Corrupt the data without updating the checksum.
        v.data[0] = b'H';
        assert!(!v.verify());
    }

    #[test]
    fn test_config_version_verify_corrupted_checksum() {
        let data = b"hello world".to_vec();
        let mut v = ConfigVersion::new(1, 0, data);
        // Corrupt the checksum.
        v.crc32 = v.crc32.wrapping_add(1);
        assert!(!v.verify());
    }

    #[test]
    fn test_config_version_verify_empty_data() {
        let v = ConfigVersion::new(1, 0, Vec::new());
        assert!(v.verify());
    }

    // ---- ConfigVersion clone & debug ----

    #[test]
    fn test_config_version_clone() {
        let v = ConfigVersion::new(5, 1000, vec![1, 2, 3]);
        let v2 = v.clone();
        assert_eq!(v.version, v2.version);
        assert_eq!(v.timestamp, v2.timestamp);
        assert_eq!(v.data, v2.data);
        assert_eq!(v.crc32, v2.crc32);
    }

    #[test]
    fn test_config_version_debug_format() {
        let v = ConfigVersion::new(1, 0, vec![1, 2]);
        let s = format!("{:?}", v);
        assert!(s.contains("ConfigVersion"));
        assert!(s.contains("version: 1"));
    }

    // ---- VersionHistory::new ----

    #[test]
    fn test_version_history_new_empty() {
        let h = VersionHistory::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert_eq!(h.current_version(), None);
        assert!(h.list_versions().is_empty());
    }

    #[test]
    fn test_version_history_default_is_empty() {
        let h = VersionHistory::default();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
    }

    // ---- VersionHistory::record ----

    #[test]
    fn test_record_returns_version_numbers_ascending() {
        let mut h = VersionHistory::new();
        let v1 = h.record(100, vec![1]);
        let v2 = h.record(200, vec![2]);
        let v3 = h.record(300, vec![3]);
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(v3, 3);
    }

    #[test]
    fn test_record_increments_length() {
        let mut h = VersionHistory::new();
        assert_eq!(h.len(), 0);
        h.record(100, vec![1]);
        assert_eq!(h.len(), 1);
        h.record(200, vec![2]);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_record_updates_current_version() {
        let mut h = VersionHistory::new();
        assert_eq!(h.current_version(), None);
        let v1 = h.record(100, vec![1]);
        assert_eq!(h.current_version(), Some(v1));
        let v2 = h.record(200, vec![2]);
        assert_eq!(h.current_version(), Some(v2));
    }

    #[test]
    fn test_record_data_stored_correctly() {
        let mut h = VersionHistory::new();
        let data = b"port = 8080".to_vec();
        let version = h.record(1000, data.clone());
        let entry = h.get(version).expect("version should exist");
        assert_eq!(entry.version, version);
        assert_eq!(entry.timestamp, 1000);
        assert_eq!(entry.data, data);
        assert_eq!(entry.crc32, compute_crc32(&data));
    }

    #[test]
    fn test_record_evicts_oldest_when_full() {
        let mut h = VersionHistory::new();
        // Record MAX_VERSIONS + 5 entries.
        for i in 0..(MAX_VERSIONS + 5) {
            h.record(i as u64, vec![i as u8]);
        }
        // Length should be capped at MAX_VERSIONS.
        assert_eq!(h.len(), MAX_VERSIONS);
        // The first 5 versions should have been evicted.
        for v in 1..=5 {
            assert!(h.get(v as u64).is_none(), "version {} should be evicted", v);
        }
        // Versions 6..=15 should still exist.
        for v in 6..=15 {
            assert!(h.get(v as u64).is_some(), "version {} should exist", v);
        }
    }

    #[test]
    fn test_record_eviction_preserves_order() {
        let mut h = VersionHistory::new();
        for i in 0..(MAX_VERSIONS + 2) {
            h.record(i as u64, vec![i as u8]);
        }
        let versions = h.list_versions();
        assert_eq!(versions.len(), MAX_VERSIONS);
        // Versions should be in ascending order.
        for i in 1..versions.len() {
            assert!(versions[i - 1] < versions[i]);
        }
        // The oldest remaining should be version 3 (1 and 2 evicted).
        assert_eq!(versions[0], 3);
        assert_eq!(versions[versions.len() - 1], MAX_VERSIONS as u64 + 2);
    }

    #[test]
    fn test_record_saturating_version_counter() {
        // Test that saturating_add doesn't panic on overflow.
        let mut h = VersionHistory {
            versions: Vec::new(),
            next_version: u64::MAX,
        };
        let v1 = h.record(0, vec![1]);
        assert_eq!(v1, u64::MAX);
        let v2 = h.record(0, vec![2]);
        // saturating_add keeps it at MAX.
        assert_eq!(v2, u64::MAX);
    }

    // ---- VersionHistory::get ----

    #[test]
    fn test_get_existing_version() {
        let mut h = VersionHistory::new();
        let v = h.record(100, vec![1, 2, 3]);
        let entry = h.get(v).expect("should exist");
        assert_eq!(entry.version, v);
        assert_eq!(entry.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_get_missing_version() {
        let mut h = VersionHistory::new();
        h.record(100, vec![1]);
        assert!(h.get(999).is_none());
    }

    #[test]
    fn test_get_on_empty_history() {
        let h = VersionHistory::new();
        assert!(h.get(1).is_none());
    }

    // ---- VersionHistory::current_version ----

    #[test]
    fn test_current_version_after_multiple_records() {
        let mut h = VersionHistory::new();
        h.record(100, vec![1]);
        h.record(200, vec![2]);
        h.record(300, vec![3]);
        assert_eq!(h.current_version(), Some(3));
    }

    #[test]
    fn test_current_version_after_eviction() {
        let mut h = VersionHistory::new();
        let mut last = 0;
        for i in 0..(MAX_VERSIONS + 3) {
            last = h.record(i as u64, vec![i as u8]);
        }
        // Current version should be the last recorded, regardless of eviction.
        assert_eq!(h.current_version(), Some(last));
    }

    // ---- VersionHistory::list_versions ----

    #[test]
    fn test_list_versions_empty() {
        let h = VersionHistory::new();
        let v = h.list_versions();
        assert!(v.is_empty());
    }

    #[test]
    fn test_list_versions_ascending() {
        let mut h = VersionHistory::new();
        for i in 0..5 {
            h.record(i as u64, vec![i as u8]);
        }
        let versions = h.list_versions();
        assert_eq!(versions, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_list_versions_after_eviction() {
        let mut h = VersionHistory::new();
        for i in 0..(MAX_VERSIONS + 1) {
            h.record(i as u64, vec![i as u8]);
        }
        let versions = h.list_versions();
        assert_eq!(versions.len(), MAX_VERSIONS);
        // Version 1 evicted, versions 2..=11 remain.
        assert_eq!(versions[0], 2);
        assert_eq!(versions[versions.len() - 1], MAX_VERSIONS as u64 + 1);
    }

    // ---- VersionHistory::len & is_empty ----

    #[test]
    fn test_len_empty() {
        let h = VersionHistory::new();
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn test_len_non_empty() {
        let mut h = VersionHistory::new();
        h.record(0, vec![1]);
        h.record(0, vec![2]);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_len_capped_at_max() {
        let mut h = VersionHistory::new();
        for _ in 0..(MAX_VERSIONS + 10) {
            h.record(0, vec![1]);
        }
        assert_eq!(h.len(), MAX_VERSIONS);
    }

    #[test]
    fn test_is_empty_true() {
        let h = VersionHistory::new();
        assert!(h.is_empty());
    }

    #[test]
    fn test_is_empty_false() {
        let mut h = VersionHistory::new();
        h.record(0, vec![1]);
        assert!(!h.is_empty());
    }

    // ---- MAX_VERSIONS constant ----

    #[test]
    fn test_max_versions_value() {
        assert_eq!(MAX_VERSIONS, 10);
    }

    // ---- Integration: record + verify round trip ----

    #[test]
    fn test_record_then_verify_passes() {
        let mut h = VersionHistory::new();
        let data = b"port = 8080\nhost = \"localhost\"\n".to_vec();
        let version = h.record(1000, data);
        let entry = h.get(version).expect("version should exist");
        assert!(entry.verify());
    }

    #[test]
    fn test_record_multiple_then_get_each() {
        let mut h = VersionHistory::new();
        let mut versions = Vec::new();
        for i in 0..5 {
            let v = h.record(i as u64, vec![i as u8, (i + 1) as u8]);
            versions.push(v);
        }
        for (i, &v) in versions.iter().enumerate() {
            let entry = h.get(v).expect("version should exist");
            assert_eq!(entry.data, vec![i as u8, (i + 1) as u8]);
            assert_eq!(entry.timestamp, i as u64);
        }
    }

    #[test]
    fn test_history_debug_format() {
        let mut h = VersionHistory::new();
        h.record(100, vec![1]);
        let s = format!("{:?}", h);
        assert!(s.contains("VersionHistory"));
    }
}
