//! Storage error type and recoverability classification.
//!
//! [`StorageError`] is the unified error enum returned by all storage driver
//! operations. Each variant carries enough context to drive retry, fallback,
//! or fault-isolation decisions in the upper layers (filesystem, journal).
//!
//! # Recoverability
//!
//! [`StorageError::is_recoverable`] classifies errors into:
//! - **Recoverable** (`Timeout`, `DmaError`) — transient; a retry may succeed.
//! - **Non-recoverable** (all others) — permanent; the block/device must be
//!   quarantined or replaced.

use core::fmt;

/// Unified error type for all storage driver operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    /// Driver has not been initialized (no `init()` call or hardware absent).
    NotInitialized,
    /// Operation did not complete within the timeout window.
    Timeout {
        /// Operation name (e.g. `"read"`, `"write"`, `"erase"`).
        operation: &'static str,
        /// Elapsed milliseconds before giving up.
        ms: u32,
    },
    /// Block is marked bad in the bad block table.
    BadBlock {
        /// Index of the bad block.
        block_idx: u64,
    },
    /// DMA engine reported an error code.
    DmaError {
        /// Vendor-specific DMA error code.
        code: u32,
    },
    /// Computed CRC did not match the stored CRC.
    CrcMismatch {
        /// Expected CRC value.
        expected: u32,
        /// Actually computed CRC value.
        actual: u32,
    },
    /// Block index is outside the device's valid range.
    OutOfRange {
        /// Requested block index.
        block_idx: u64,
        /// Maximum valid block index (exclusive).
        max: u64,
    },
    /// Hardware fault reported by the controller.
    HardwareFault {
        /// Static detail string describing the fault.
        detail: &'static str,
    },
    /// Device is write-protected and a write/erase was attempted.
    WriteProtected,
}

impl StorageError {
    /// Returns `true` if the error is transient and a retry may succeed.
    ///
    /// Only [`StorageError::Timeout`] and [`StorageError::DmaError`] are
    /// considered recoverable. All other variants indicate a permanent
    /// condition (bad block, CRC corruption, out-of-range, hardware fault,
    /// write protection, or uninitialized hardware).
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            StorageError::Timeout { .. } | StorageError::DmaError { .. }
        )
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::NotInitialized => write!(f, "storage driver not initialized"),
            StorageError::Timeout { operation, ms } => {
                write!(f, "timeout during '{}' after {} ms", operation, ms)
            }
            StorageError::BadBlock { block_idx } => {
                write!(f, "block {} is marked bad", block_idx)
            }
            StorageError::DmaError { code } => write!(f, "DMA error (code=0x{:08X})", code),
            StorageError::CrcMismatch { expected, actual } => {
                write!(
                    f,
                    "CRC mismatch: expected 0x{:08X}, actual 0x{:08X}",
                    expected, actual
                )
            }
            StorageError::OutOfRange { block_idx, max } => {
                write!(f, "block {} out of range (max={})", block_idx, max)
            }
            StorageError::HardwareFault { detail } => {
                write!(f, "hardware fault: {}", detail)
            }
            StorageError::WriteProtected => write!(f, "device is write-protected"),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_initialized() {
        let e = StorageError::NotInitialized;
        assert!(!e.is_recoverable());
        assert_eq!(format!("{}", e), "storage driver not initialized");
    }

    #[test]
    fn test_timeout_recoverable() {
        let e = StorageError::Timeout {
            operation: "read",
            ms: 500,
        };
        assert!(e.is_recoverable());
        assert_eq!(format!("{}", e), "timeout during 'read' after 500 ms");
    }

    #[test]
    fn test_timeout_write_recoverable() {
        let e = StorageError::Timeout {
            operation: "write",
            ms: 1000,
        };
        assert!(e.is_recoverable());
        assert_eq!(format!("{}", e), "timeout during 'write' after 1000 ms");
    }

    #[test]
    fn test_bad_block_not_recoverable() {
        let e = StorageError::BadBlock { block_idx: 42 };
        assert!(!e.is_recoverable());
        assert_eq!(format!("{}", e), "block 42 is marked bad");
    }

    #[test]
    fn test_dma_error_recoverable() {
        let e = StorageError::DmaError { code: 0xDEAD_BEEF };
        assert!(e.is_recoverable());
        assert_eq!(format!("{}", e), "DMA error (code=0xDEADBEEF)");
    }

    #[test]
    fn test_crc_mismatch_not_recoverable() {
        let e = StorageError::CrcMismatch {
            expected: 0x1234_5678,
            actual: 0xDEAD_BEEF,
        };
        assert!(!e.is_recoverable());
        assert_eq!(
            format!("{}", e),
            "CRC mismatch: expected 0x12345678, actual 0xDEADBEEF"
        );
    }

    #[test]
    fn test_out_of_range_not_recoverable() {
        let e = StorageError::OutOfRange {
            block_idx: 9999,
            max: 1024,
        };
        assert!(!e.is_recoverable());
        assert_eq!(format!("{}", e), "block 9999 out of range (max=1024)");
    }

    #[test]
    fn test_hardware_fault_not_recoverable() {
        let e = StorageError::HardwareFault {
            detail: "ECC uncorrectable",
        };
        assert!(!e.is_recoverable());
        assert_eq!(format!("{}", e), "hardware fault: ECC uncorrectable");
    }

    #[test]
    fn test_write_protected_not_recoverable() {
        let e = StorageError::WriteProtected;
        assert!(!e.is_recoverable());
        assert_eq!(format!("{}", e), "device is write-protected");
    }

    #[test]
    fn test_equality() {
        assert_eq!(StorageError::NotInitialized, StorageError::NotInitialized);
        assert_eq!(
            StorageError::Timeout {
                operation: "read",
                ms: 100
            },
            StorageError::Timeout {
                operation: "read",
                ms: 100
            }
        );
        assert_ne!(
            StorageError::Timeout {
                operation: "read",
                ms: 100
            },
            StorageError::Timeout {
                operation: "read",
                ms: 200
            }
        );
    }
}
