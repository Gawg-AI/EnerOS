//! Time-based index for locating chunk files.
//!
//! [`TimeIndex`] maps timestamps to [`IndexEntry`] records, allowing the
//! reader to quickly locate which chunk files cover a given time range.
//! Uses `alloc::collections::BTreeMap` for sorted, deterministic iteration
//! (no_std-compatible; no `HashMap` random state required).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::TsdbError;

/// A single index record pointing to a persisted chunk file.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// Start timestamp of the chunk (milliseconds).
    pub time: u64,
    /// Filesystem path to the chunk file.
    pub file_path: String,
    /// Unique chunk identifier within the device/metric partition.
    pub chunk_id: u32,
    /// Number of data points stored in the chunk.
    pub point_count: u32,
}

/// Time-ordered index of chunk files.
///
/// Multiple entries may share the same `time` key (e.g. when several
/// chunks are created at the same start timestamp for different
/// device/metric pairs); they are stored in a `Vec` per key.
pub struct TimeIndex {
    entries: BTreeMap<u64, Vec<IndexEntry>>,
}

impl TimeIndex {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Adds an entry to the index.
    pub fn add(&mut self, time: u64, file_path: String, chunk_id: u32, point_count: u32) {
        let entry = IndexEntry {
            time,
            file_path,
            chunk_id,
            point_count,
        };
        self.entries.entry(time).or_default().push(entry);
    }

    /// Returns references to all entries whose `time` falls within
    /// `[start, end]` (inclusive).
    pub fn find_range(&self, start: u64, end: u64) -> Vec<&IndexEntry> {
        let mut result = Vec::new();
        for entries in self.entries.range(start..=end).map(|(_, v)| v) {
            for entry in entries {
                result.push(entry);
            }
        }
        result
    }

    /// Removes all entries whose `time` is strictly less than `time`
    /// and returns them (used for TTL cleanup).
    pub fn remove_before(&mut self, time: u64) -> Vec<IndexEntry> {
        let mut removed = Vec::new();
        // Split the map into (< time) and (>= time), draining the lower half.
        let lower = self.entries.split_off(&(time));
        // `lower` now contains keys >= time; the original map retains keys < time.
        // Swap so that `self.entries` holds the retained (>= time) entries.
        let expired = core::mem::replace(&mut self.entries, lower);
        for (_, entries) in expired {
            removed.extend(entries);
        }
        removed
    }

    /// Serializes the index to a byte vector.
    ///
    /// Format (all integers little-endian):
    /// ```text
    /// entry_count: u32
    /// for each entry:
    ///   time: u64
    ///   chunk_id: u32
    ///   point_count: u32
    ///   path_len: u32
    ///   path_bytes: [u8; path_len]
    /// ```
    pub fn serialize(&self) -> Vec<u8> {
        let total_entries: u32 = self.entries.values().map(|v| v.len() as u32).sum();
        let mut buf = Vec::with_capacity(4 + (total_entries as usize) * 24);
        buf.extend_from_slice(&total_entries.to_le_bytes());
        for entries in self.entries.values() {
            for entry in entries {
                let path_bytes = entry.file_path.as_bytes();
                buf.extend_from_slice(&entry.time.to_le_bytes());
                buf.extend_from_slice(&entry.chunk_id.to_le_bytes());
                buf.extend_from_slice(&entry.point_count.to_le_bytes());
                buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(path_bytes);
            }
        }
        buf
    }

    /// Deserializes an index from a byte slice.
    ///
    /// Returns [`TsdbError::IndexCorrupted`] if the data is malformed.
    pub fn deserialize(data: &[u8]) -> Result<Self, TsdbError> {
        if data.len() < 4 {
            return Err(TsdbError::IndexCorrupted);
        }
        let entry_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let mut off = 4;
        let mut index = Self::new();

        for _ in 0..entry_count {
            // time: u64
            if off + 8 > data.len() {
                return Err(TsdbError::IndexCorrupted);
            }
            let time = u64::from_le_bytes([
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ]);
            off += 8;

            // chunk_id: u32
            if off + 4 > data.len() {
                return Err(TsdbError::IndexCorrupted);
            }
            let chunk_id =
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            off += 4;

            // point_count: u32
            if off + 4 > data.len() {
                return Err(TsdbError::IndexCorrupted);
            }
            let point_count =
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            off += 4;

            // path_len: u32
            if off + 4 > data.len() {
                return Err(TsdbError::IndexCorrupted);
            }
            let path_len =
                u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    as usize;
            off += 4;

            // path_bytes
            if off + path_len > data.len() {
                return Err(TsdbError::IndexCorrupted);
            }
            let path_str = String::from_utf8(data[off..off + path_len].to_vec())
                .map_err(|_| TsdbError::IndexCorrupted)?;
            off += path_len;

            index.add(time, path_str, chunk_id, point_count);
        }

        Ok(index)
    }

    /// Returns the total number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }

    /// Returns `true` if the index contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.values().all(|v| v.is_empty())
    }
}

impl Default for TimeIndex {
    fn default() -> Self {
        Self::new()
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
    fn test_new_index_is_empty() {
        let idx = TimeIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_add_and_len() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/tsdb/d1/m1/0001"), 1, 100);
        idx.add(2000, String::from("/tsdb/d1/m1/0002"), 2, 200);
        assert_eq!(idx.len(), 2);
        assert!(!idx.is_empty());
    }

    #[test]
    fn test_add_same_timestamp_multiple_entries() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/tsdb/d1/m1/0001"), 1, 100);
        idx.add(1000, String::from("/tsdb/d2/m1/0001"), 2, 50);
        assert_eq!(idx.len(), 2);
        let found = idx.find_range(1000, 1000);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_find_range_inclusive_bounds() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/tsdb/d1/m1/0001"), 1, 100);
        idx.add(2000, String::from("/tsdb/d1/m1/0002"), 2, 200);
        idx.add(3000, String::from("/tsdb/d1/m1/0003"), 3, 300);

        // Range [1500, 2500] should match only t=2000.
        let found = idx.find_range(1500, 2500);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].time, 2000);
        assert_eq!(found[0].file_path, "/tsdb/d1/m1/0002");

        // Range [1000, 3000] should match all three.
        let found = idx.find_range(1000, 3000);
        assert_eq!(found.len(), 3);

        // Range [500, 999] should match none.
        let found = idx.find_range(500, 999);
        assert!(found.is_empty());
    }

    #[test]
    fn test_remove_before() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/a"), 1, 10);
        idx.add(2000, String::from("/b"), 2, 20);
        idx.add(3000, String::from("/c"), 3, 30);

        // Remove entries with time < 2500 → removes t=1000 and t=2000.
        let removed = idx.remove_before(2500);
        assert_eq!(removed.len(), 2);
        // The remaining index should have only t=3000.
        assert_eq!(idx.len(), 1);
        let remaining = idx.find_range(0, u64::MAX);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].time, 3000);
    }

    #[test]
    fn test_remove_before_keeps_exact_boundary() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/a"), 1, 10);
        idx.add(2000, String::from("/b"), 2, 20);

        // Remove entries with time < 2000 → removes t=1000, keeps t=2000.
        let removed = idx.remove_before(2000);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].time, 1000);
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut idx = TimeIndex::new();
        idx.add(1000, String::from("/tsdb/d1/m1/0001"), 1, 100);
        idx.add(2000, String::from("/tsdb/d1/m1/0002"), 2, 200);
        idx.add(3000, String::from("/tsdb/d1/m1/0003"), 3, 300);

        let serialized = idx.serialize();
        let restored = TimeIndex::deserialize(&serialized).expect("deserialize");

        assert_eq!(restored.len(), 3);
        let all = restored.find_range(0, u64::MAX);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].time, 1000);
        assert_eq!(all[0].file_path, "/tsdb/d1/m1/0001");
        assert_eq!(all[0].chunk_id, 1);
        assert_eq!(all[0].point_count, 100);
        assert_eq!(all[1].time, 2000);
        assert_eq!(all[2].time, 3000);
    }

    #[test]
    fn test_serialize_deserialize_empty_index() {
        let idx = TimeIndex::new();
        let serialized = idx.serialize();
        assert_eq!(serialized.len(), 4); // just the count (0)
        let restored = TimeIndex::deserialize(&serialized).expect("deserialize");
        assert!(restored.is_empty());
    }

    #[test]
    fn test_deserialize_corrupted_too_short() {
        let result = TimeIndex::deserialize(&[0u8; 2]);
        assert!(matches!(result, Err(TsdbError::IndexCorrupted)));
    }

    #[test]
    fn test_deserialize_corrupted_truncated_entry() {
        // Claim 1 entry but provide no entry data.
        let data = 1u32.to_le_bytes();
        let result = TimeIndex::deserialize(&data);
        assert!(matches!(result, Err(TsdbError::IndexCorrupted)));
    }

    #[test]
    fn test_deserialize_corrupted_path_truncated() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // 1 entry
        buf.extend_from_slice(&1000u64.to_le_bytes()); // time
        buf.extend_from_slice(&1u32.to_le_bytes()); // chunk_id
        buf.extend_from_slice(&100u32.to_le_bytes()); // point_count
        buf.extend_from_slice(&100u32.to_le_bytes()); // path_len = 100 (but no bytes follow)
        let result = TimeIndex::deserialize(&buf);
        assert!(matches!(result, Err(TsdbError::IndexCorrupted)));
    }
}
