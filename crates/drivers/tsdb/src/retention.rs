//! TTL-based retention policy — expires chunk files older than the configured
//! retention period.
//!
//! The [`cleanup_expired`] function scans the index for entries whose start
//! time precedes the retention cutoff, removes the corresponding chunk files
//! from the filesystem, and prunes the index.

use eneros_fs::{FileSystem, FsError, Lfs};

use crate::error::TsdbError;
use crate::index::TimeIndex;

// ============================================================================
// Retention logic
// ============================================================================

/// Returns `true` if a chunk starting at `time` has expired relative to `now`
/// and `retention_ms`.
///
/// A chunk is expired when `now - time > retention_ms`. Saturating subtraction
/// is used so that a `time` in the future (clock skew) is treated as
/// non-expired.
pub fn should_expire(time: u64, now: u64, retention_ms: u64) -> bool {
    now.saturating_sub(time) > retention_ms
}

/// Removes all expired chunk files from the filesystem and index.
///
/// `now` is the current timestamp (milliseconds). Chunks whose start time
/// precedes `now - retention_ms` are deleted. Returns the number of chunk
/// files successfully removed.
///
/// Files that no longer exist on disk (`FsError::NotFound`) are silently
/// skipped — the index entry is still pruned. Other I/O errors propagate.
pub fn cleanup_expired(
    index: &mut TimeIndex,
    fs: &mut Lfs,
    now: u64,
    retention_ms: u64,
) -> Result<u64, TsdbError> {
    // Compute the cutoff: entries with time < cutoff are expired.
    // Using `>` (not `>=`) in `should_expire` means an entry at exactly
    // `now - retention_ms` is NOT expired, so we remove entries strictly
    // before that point.
    let cutoff = now.saturating_sub(retention_ms);

    // Remove all entries with time < cutoff from the index.
    let expired = index.remove_before(cutoff);

    let mut removed: u64 = 0;
    for entry in &expired {
        match fs.remove(&entry.file_path) {
            Ok(()) => removed += 1,
            Err(FsError::NotFound { .. }) => {
                // File already gone — still count it as removed from the
                // index's perspective.
                removed += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(removed)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::string::String;

    use eneros_storage::MockBlockDevice;

    use super::*;
    use crate::index::TimeIndex;

    fn make_fs() -> Lfs {
        let dev: Box<dyn eneros_storage::BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        Lfs::format(dev).expect("format should succeed")
    }

    #[test]
    fn test_should_expire_old_data() {
        // now=100000, time=10000, retention=50000 → 100000-10000=90000 > 50000.
        assert!(should_expire(10000, 100000, 50000));
    }

    #[test]
    fn test_should_expire_recent_data() {
        // now=100000, time=80000, retention=50000 → 100000-80000=20000 ≤ 50000.
        assert!(!should_expire(80000, 100000, 50000));
    }

    #[test]
    fn test_should_expire_exact_boundary() {
        // now=100000, time=50000, retention=50000 → 50000 > 50000 is false.
        assert!(!should_expire(50000, 100000, 50000));
    }

    #[test]
    fn test_should_expire_just_past_boundary() {
        // now=100000, time=49999, retention=50000 → 50001 > 50000 is true.
        assert!(should_expire(49999, 100000, 50000));
    }

    #[test]
    fn test_should_expire_future_time() {
        // time > now (clock skew) → saturating_sub yields 0, not expired.
        assert!(!should_expire(200000, 100000, 50000));
    }

    #[test]
    fn test_cleanup_expired_removes_old_entries() {
        let mut fs = make_fs();
        let mut index = TimeIndex::new();

        // Create the /tsdb directory first (littlefs2 requires parent dirs).
        use eneros_fs::{FileMode, FileSystem};
        let _ = fs.mkdir("/tsdb");

        // Create chunk files on disk and add index entries.
        let old_path = "/tsdb/old";
        let new_path = "/tsdb/new";
        let _ = fs.create(old_path, FileMode::default_file());
        let _ = fs.create(new_path, FileMode::default_file());

        // old entry at t=1000, new entry at t=90000.
        index.add(1000, String::from(old_path), 1, 100);
        index.add(90000, String::from(new_path), 2, 100);

        // now=100000, retention=50000 → cutoff=50000.
        // Entry at t=1000 < 50000 → expired.
        // Entry at t=90000 >= 50000 → retained.
        let removed = cleanup_expired(&mut index, &mut fs, 100000, 50000).expect("cleanup");
        assert_eq!(removed, 1);
        assert_eq!(index.len(), 1);

        // The old file should be gone.
        assert!(fs.stat(old_path).is_err());
        // The new file should still exist.
        assert!(fs.stat(new_path).is_ok());
    }

    #[test]
    fn test_cleanup_expired_handles_missing_file() {
        let mut fs = make_fs();
        let mut index = TimeIndex::new();

        // Add an index entry pointing to a non-existent file.
        index.add(1000, String::from("/tsdb/ghost"), 1, 100);

        // cleanup should succeed and count the entry as removed.
        let removed = cleanup_expired(&mut index, &mut fs, 100000, 50000).expect("cleanup");
        assert_eq!(removed, 1);
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_cleanup_expired_nothing_to_remove() {
        let mut fs = make_fs();
        let mut index = TimeIndex::new();

        // Add a recent entry that should NOT be expired.
        use eneros_fs::{FileMode, FileSystem};
        let _ = fs.create("/tsdb/recent", FileMode::default_file());
        index.add(90000, String::from("/tsdb/recent"), 1, 100);

        let removed = cleanup_expired(&mut index, &mut fs, 100000, 50000).expect("cleanup");
        assert_eq!(removed, 0);
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn test_cleanup_expired_empty_index() {
        let mut fs = make_fs();
        let mut index = TimeIndex::new();

        let removed = cleanup_expired(&mut index, &mut fs, 100000, 50000).expect("cleanup");
        assert_eq!(removed, 0);
        assert_eq!(index.len(), 0);
    }
}
