//! Filesystem error type and corruption classification.
//!
//! [`FsError`] is the unified error enum returned by all filesystem operations.
//! Each variant carries enough context to drive retry, fallback, or
//! fault-isolation decisions. Path-carrying variants use owned [`String`] so
//! that dynamically constructed paths can be reported.
//!
//! # Corruption Detection
//!
//! [`FsError::is_corruption`] returns `true` for variants that indicate on-disk
//! data corruption ([`CorruptedBlock`], [`CrcMismatch`], [`BadSuperBlock`]).
//! Callers can use this to trigger a reformat or quarantine the storage device.

use alloc::string::String;
use core::fmt;

use eneros_storage::StorageError;
use littlefs2::io::Error as LfsError;

/// Unified error type for all filesystem operations.
#[derive(Debug, Clone, PartialEq)]
pub enum FsError {
    /// No entry found at the given path.
    NotFound {
        /// The path that was not found.
        path: String,
    },
    /// A file or directory already exists at the given path.
    AlreadyExists {
        /// The path that already exists.
        path: String,
    },
    /// A path component that should be a directory is a file.
    NotADirectory {
        /// The path that is not a directory.
        path: String,
    },
    /// A path that should be a file is a directory.
    IsADirectory {
        /// The path that is a directory.
        path: String,
    },
    /// The path is invalid (too long, non-ASCII, or malformed).
    InvalidPath {
        /// The invalid path.
        path: String,
    },
    /// No space left on the device.
    NoSpace,
    /// The filesystem or file is read-only.
    ReadOnly,
    /// An I/O error occurred during the operation.
    IoError {
        /// Detail string describing the I/O failure.
        detail: String,
    },
    /// A storage block is corrupted.
    CorruptedBlock {
        /// Index of the corrupted block.
        block: u64,
    },
    /// A CRC check failed for a block.
    CrcMismatch {
        /// Index of the block with the CRC failure.
        block: u64,
        /// Expected CRC value.
        expected: u32,
        /// Actual CRC value computed from the data.
        actual: u32,
    },
    /// The superblock is missing or invalid (filesystem not formatted).
    BadSuperBlock,
    /// Too many files are open simultaneously.
    TooManyOpenFiles,
    /// An invalid argument was supplied to the operation.
    InvalidArgument,
    /// A cross-device link was attempted (source and target on different devices).
    CrossDeviceLink,
    /// The directory is not empty and cannot be removed.
    DirectoryNotEmpty,
}

impl FsError {
    /// Returns `true` if the error indicates on-disk data corruption.
    ///
    /// Corruption errors ([`CorruptedBlock`], [`CrcMismatch`], [`BadSuperBlock`])
    /// may warrant a reformat or device quarantine.
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            FsError::CorruptedBlock { .. } | FsError::CrcMismatch { .. } | FsError::BadSuperBlock
        )
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::NotFound { path } => write!(f, "not found: {}", path),
            FsError::AlreadyExists { path } => write!(f, "already exists: {}", path),
            FsError::NotADirectory { path } => write!(f, "not a directory: {}", path),
            FsError::IsADirectory { path } => write!(f, "is a directory: {}", path),
            FsError::InvalidPath { path } => write!(f, "invalid path: {}", path),
            FsError::NoSpace => write!(f, "no space left on device"),
            FsError::ReadOnly => write!(f, "read-only filesystem or file"),
            FsError::IoError { detail } => write!(f, "I/O error: {}", detail),
            FsError::CorruptedBlock { block } => write!(f, "corrupted block {}", block),
            FsError::CrcMismatch {
                block,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "CRC mismatch at block {}: expected 0x{:08X}, actual 0x{:08X}",
                    block, expected, actual
                )
            }
            FsError::BadSuperBlock => write!(f, "bad superblock (not formatted?)"),
            FsError::TooManyOpenFiles => write!(f, "too many open files"),
            FsError::InvalidArgument => write!(f, "invalid argument"),
            FsError::CrossDeviceLink => write!(f, "cross-device link not permitted"),
            FsError::DirectoryNotEmpty => write!(f, "directory not empty"),
        }
    }
}

/// Convert a littlefs2 error into an [`FsError`].
///
/// The littlefs2 [`Error`] is a code-based struct; we map known codes to
/// semantic [`FsError`] variants and fall back to [`FsError::IoError`] for
/// unrecognized codes.
impl From<LfsError> for FsError {
    fn from(err: LfsError) -> Self {
        if err == LfsError::NO_SUCH_ENTRY {
            FsError::NotFound {
                path: String::new(),
            }
        } else if err == LfsError::ENTRY_ALREADY_EXISTED {
            FsError::AlreadyExists {
                path: String::new(),
            }
        } else if err == LfsError::PATH_NOT_DIR {
            FsError::NotADirectory {
                path: String::new(),
            }
        } else if err == LfsError::PATH_IS_DIR {
            FsError::IsADirectory {
                path: String::new(),
            }
        } else if err == LfsError::DIR_NOT_EMPTY {
            FsError::DirectoryNotEmpty
        } else if err == LfsError::NO_SPACE {
            FsError::NoSpace
        } else if err == LfsError::CORRUPTION {
            FsError::CorruptedBlock { block: 0 }
        } else if err == LfsError::INVALID {
            FsError::InvalidArgument
        } else if err == LfsError::IO {
            FsError::IoError {
                detail: String::from("littlefs I/O error"),
            }
        } else if err == LfsError::NO_MEMORY {
            FsError::TooManyOpenFiles
        } else if err == LfsError::BAD_FILE_DESCRIPTOR {
            FsError::InvalidArgument
        } else if err == LfsError::FILE_TOO_BIG {
            FsError::NoSpace
        } else if err == LfsError::FILENAME_TOO_LONG {
            FsError::InvalidPath {
                path: String::new(),
            }
        } else {
            FsError::IoError {
                detail: alloc::format!("littlefs error code {}", err.code()),
            }
        }
    }
}

/// Convert a storage driver error into an [`FsError`].
///
/// [`StorageError::CrcMismatch`] maps to [`FsError::CrcMismatch`],
/// [`StorageError::BadBlock`] maps to [`FsError::CorruptedBlock`], and all
/// other variants map to [`FsError::IoError`] with a descriptive detail.
impl From<StorageError> for FsError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::CrcMismatch { expected, actual } => FsError::CrcMismatch {
                block: 0,
                expected,
                actual,
            },
            StorageError::BadBlock { block_idx } => FsError::CorruptedBlock { block: block_idx },
            StorageError::NotInitialized => FsError::IoError {
                detail: String::from("storage not initialized"),
            },
            StorageError::Timeout { operation, ms } => FsError::IoError {
                detail: alloc::format!("timeout during '{}' after {} ms", operation, ms),
            },
            StorageError::DmaError { code } => FsError::IoError {
                detail: alloc::format!("DMA error (code=0x{:08X})", code),
            },
            StorageError::OutOfRange { block_idx, max } => FsError::IoError {
                detail: alloc::format!("block {} out of range (max={})", block_idx, max),
            },
            StorageError::HardwareFault { detail } => FsError::IoError {
                detail: alloc::format!("hardware fault: {}", detail),
            },
            StorageError::WriteProtected => FsError::ReadOnly,
        }
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
    fn test_not_found() {
        let e = FsError::NotFound {
            path: String::from("/missing"),
        };
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "not found: /missing");
    }

    #[test]
    fn test_already_exists() {
        let e = FsError::AlreadyExists {
            path: String::from("/exists"),
        };
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "already exists: /exists");
    }

    #[test]
    fn test_not_a_directory() {
        let e = FsError::NotADirectory {
            path: String::from("/file"),
        };
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "not a directory: /file");
    }

    #[test]
    fn test_is_a_directory() {
        let e = FsError::IsADirectory {
            path: String::from("/dir"),
        };
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "is a directory: /dir");
    }

    #[test]
    fn test_invalid_path() {
        let e = FsError::InvalidPath {
            path: String::from("bad\0path"),
        };
        assert!(!e.is_corruption());
        assert!(format!("{}", e).contains("invalid path"));
    }

    #[test]
    fn test_no_space() {
        let e = FsError::NoSpace;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "no space left on device");
    }

    #[test]
    fn test_read_only() {
        let e = FsError::ReadOnly;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "read-only filesystem or file");
    }

    #[test]
    fn test_io_error() {
        let e = FsError::IoError {
            detail: String::from("disk failure"),
        };
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "I/O error: disk failure");
    }

    #[test]
    fn test_corrupted_block() {
        let e = FsError::CorruptedBlock { block: 42 };
        assert!(e.is_corruption());
        assert_eq!(format!("{}", e), "corrupted block 42");
    }

    #[test]
    fn test_crc_mismatch() {
        let e = FsError::CrcMismatch {
            block: 7,
            expected: 0xDEAD_BEEF,
            actual: 0x1234_5678,
        };
        assert!(e.is_corruption());
        assert!(format!("{}", e).contains("CRC mismatch at block 7"));
        assert!(format!("{}", e).contains("DEADBEEF"));
        assert!(format!("{}", e).contains("12345678"));
    }

    #[test]
    fn test_bad_super_block() {
        let e = FsError::BadSuperBlock;
        assert!(e.is_corruption());
        assert_eq!(format!("{}", e), "bad superblock (not formatted?)");
    }

    #[test]
    fn test_too_many_open_files() {
        let e = FsError::TooManyOpenFiles;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "too many open files");
    }

    #[test]
    fn test_invalid_argument() {
        let e = FsError::InvalidArgument;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "invalid argument");
    }

    #[test]
    fn test_cross_device_link() {
        let e = FsError::CrossDeviceLink;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "cross-device link not permitted");
    }

    #[test]
    fn test_directory_not_empty() {
        let e = FsError::DirectoryNotEmpty;
        assert!(!e.is_corruption());
        assert_eq!(format!("{}", e), "directory not empty");
    }

    #[test]
    fn test_is_corruption_only_for_corruption_variants() {
        assert!(FsError::CorruptedBlock { block: 0 }.is_corruption());
        assert!(FsError::CrcMismatch {
            block: 0,
            expected: 0,
            actual: 1
        }
        .is_corruption());
        assert!(FsError::BadSuperBlock.is_corruption());
        assert!(!FsError::NoSpace.is_corruption());
        assert!(!FsError::NotFound {
            path: String::new()
        }
        .is_corruption());
        assert!(!FsError::IoError {
            detail: String::new()
        }
        .is_corruption());
    }

    // ---- From<littlefs2::io::Error> ----

    #[test]
    fn test_from_lfs_no_such_entry() {
        let e: FsError = LfsError::NO_SUCH_ENTRY.into();
        assert!(matches!(e, FsError::NotFound { .. }));
    }

    #[test]
    fn test_from_lfs_already_exists() {
        let e: FsError = LfsError::ENTRY_ALREADY_EXISTED.into();
        assert!(matches!(e, FsError::AlreadyExists { .. }));
    }

    #[test]
    fn test_from_lfs_path_not_dir() {
        let e: FsError = LfsError::PATH_NOT_DIR.into();
        assert!(matches!(e, FsError::NotADirectory { .. }));
    }

    #[test]
    fn test_from_lfs_path_is_dir() {
        let e: FsError = LfsError::PATH_IS_DIR.into();
        assert!(matches!(e, FsError::IsADirectory { .. }));
    }

    #[test]
    fn test_from_lfs_dir_not_empty() {
        let e: FsError = LfsError::DIR_NOT_EMPTY.into();
        assert_eq!(e, FsError::DirectoryNotEmpty);
    }

    #[test]
    fn test_from_lfs_no_space() {
        let e: FsError = LfsError::NO_SPACE.into();
        assert_eq!(e, FsError::NoSpace);
    }

    #[test]
    fn test_from_lfs_corruption() {
        let e: FsError = LfsError::CORRUPTION.into();
        assert!(e.is_corruption());
        assert!(matches!(e, FsError::CorruptedBlock { .. }));
    }

    #[test]
    fn test_from_lfs_invalid() {
        let e: FsError = LfsError::INVALID.into();
        assert_eq!(e, FsError::InvalidArgument);
    }

    #[test]
    fn test_from_lfs_io() {
        let e: FsError = LfsError::IO.into();
        assert!(matches!(e, FsError::IoError { .. }));
    }

    #[test]
    fn test_from_lfs_no_memory() {
        let e: FsError = LfsError::NO_MEMORY.into();
        assert_eq!(e, FsError::TooManyOpenFiles);
    }

    // ---- From<StorageError> ----

    #[test]
    fn test_from_storage_crc_mismatch() {
        let e: FsError = StorageError::CrcMismatch {
            expected: 0xAA,
            actual: 0xBB,
        }
        .into();
        assert!(e.is_corruption());
        match e {
            FsError::CrcMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 0xAA);
                assert_eq!(actual, 0xBB);
            }
            _ => panic!("expected CrcMismatch"),
        }
    }

    #[test]
    fn test_from_storage_bad_block() {
        let e: FsError = StorageError::BadBlock { block_idx: 99 }.into();
        assert!(e.is_corruption());
        match e {
            FsError::CorruptedBlock { block } => assert_eq!(block, 99),
            _ => panic!("expected CorruptedBlock"),
        }
    }

    #[test]
    fn test_from_storage_write_protected() {
        let e: FsError = StorageError::WriteProtected.into();
        assert_eq!(e, FsError::ReadOnly);
    }

    #[test]
    fn test_from_storage_not_initialized() {
        let e: FsError = StorageError::NotInitialized.into();
        assert!(matches!(e, FsError::IoError { .. }));
    }

    #[test]
    fn test_from_storage_timeout() {
        let e: FsError = StorageError::Timeout {
            operation: "read",
            ms: 100,
        }
        .into();
        assert!(matches!(e, FsError::IoError { .. }));
    }

    #[test]
    fn test_from_storage_hardware_fault() {
        let e: FsError = StorageError::HardwareFault {
            detail: "ECC error",
        }
        .into();
        assert!(matches!(e, FsError::IoError { .. }));
    }

    #[test]
    fn test_equality() {
        assert_eq!(FsError::NoSpace, FsError::NoSpace);
        assert_eq!(FsError::BadSuperBlock, FsError::BadSuperBlock);
        assert_ne!(FsError::NoSpace, FsError::ReadOnly);
        assert_eq!(
            FsError::NotFound {
                path: String::from("/a")
            },
            FsError::NotFound {
                path: String::from("/a")
            }
        );
        assert_ne!(
            FsError::NotFound {
                path: String::from("/a")
            },
            FsError::NotFound {
                path: String::from("/b")
            }
        );
    }
}
