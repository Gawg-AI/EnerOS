//! Error types for the configuration management crate.
//!
//! [`ConfigError`] covers all failure modes: file I/O, parsing, schema
//! validation, version/rollback, and internal invariant violations.

use alloc::string::String;

use eneros_fs::FsError;

/// Error returned by configuration operations.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem error (read/write/open/create).
    Fs(FsError),
    /// File not found at the given path.
    NotFound { path: String },
    /// TOML parse error (malformed TOML).
    TomlParse(String),
    /// JSON parse error (malformed JSON).
    JsonParse(String),
    /// Schema validation failure (type mismatch, missing required field, etc.).
    SchemaViolation { field: String, reason: String },
    /// CRC32 checksum mismatch on version rollback (data corruption).
    ChecksumMismatch,
    /// Requested version does not exist in the history.
    VersionNotFound { version: u64 },
    /// Internal error (e.g. serialization failure, invariant violation).
    Internal(String),
}

impl From<FsError> for ConfigError {
    fn from(e: FsError) -> Self {
        ConfigError::Fs(e)
    }
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Fs(e) => write!(f, "filesystem error: {}", e),
            ConfigError::NotFound { path } => write!(f, "config file not found: {}", path),
            ConfigError::TomlParse(msg) => write!(f, "TOML parse error: {}", msg),
            ConfigError::JsonParse(msg) => write!(f, "JSON parse error: {}", msg),
            ConfigError::SchemaViolation { field, reason } => {
                write!(f, "schema violation on '{}': {}", field, reason)
            }
            ConfigError::ChecksumMismatch => write!(f, "CRC32 checksum mismatch (data corruption)"),
            ConfigError::VersionNotFound { version } => {
                write!(f, "version {} not found in history", version)
            }
            ConfigError::Internal(msg) => write!(f, "internal error: {}", msg),
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

    // ---- Display formatting ----

    #[test]
    fn test_display_fs_error() {
        let err = ConfigError::Fs(FsError::NotFound {
            path: String::from("/x"),
        });
        let msg = format!("{}", err);
        assert!(msg.contains("filesystem error"));
    }

    #[test]
    fn test_display_not_found() {
        let err = ConfigError::NotFound {
            path: String::from("/config/device.toml"),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("config file not found"));
        assert!(msg.contains("/config/device.toml"));
    }

    #[test]
    fn test_display_toml_parse() {
        let err = ConfigError::TomlParse(String::from("unexpected token at line 3"));
        let msg = format!("{}", err);
        assert!(msg.contains("TOML parse error"));
        assert!(msg.contains("unexpected token at line 3"));
    }

    #[test]
    fn test_display_json_parse() {
        let err = ConfigError::JsonParse(String::from("unexpected character '}'"));
        let msg = format!("{}", err);
        assert!(msg.contains("JSON parse error"));
        assert!(msg.contains("unexpected character '}'"));
    }

    #[test]
    fn test_display_schema_violation() {
        let err = ConfigError::SchemaViolation {
            field: String::from("device.port"),
            reason: String::from("expected type int, got string"),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("schema violation"));
        assert!(msg.contains("device.port"));
        assert!(msg.contains("expected type int, got string"));
    }

    #[test]
    fn test_display_checksum_mismatch() {
        let err = ConfigError::ChecksumMismatch;
        let msg = format!("{}", err);
        assert!(msg.contains("CRC32 checksum mismatch"));
    }

    #[test]
    fn test_display_version_not_found() {
        let err = ConfigError::VersionNotFound { version: 42 };
        let msg = format!("{}", err);
        assert!(msg.contains("version 42 not found"));
    }

    #[test]
    fn test_display_internal() {
        let err = ConfigError::Internal(String::from("TOML root value must be a Table"));
        let msg = format!("{}", err);
        assert!(msg.contains("internal error"));
        assert!(msg.contains("TOML root value must be a Table"));
    }

    // ---- From<FsError> ----

    #[test]
    fn test_from_fs_error_not_found() {
        let fs_err = FsError::NotFound {
            path: String::from("/missing.toml"),
        };
        let config_err: ConfigError = fs_err.into();
        assert!(matches!(
            config_err,
            ConfigError::Fs(FsError::NotFound { .. })
        ));
    }

    #[test]
    fn test_from_fs_error_no_space() {
        let fs_err = FsError::NoSpace;
        let config_err: ConfigError = fs_err.into();
        assert!(matches!(config_err, ConfigError::Fs(FsError::NoSpace)));
    }

    #[test]
    fn test_from_fs_error_io() {
        let fs_err = FsError::IoError {
            detail: String::from("read failed"),
        };
        let config_err: ConfigError = fs_err.into();
        assert!(matches!(
            config_err,
            ConfigError::Fs(FsError::IoError { .. })
        ));
    }

    #[test]
    fn test_from_fs_error_via_question_mark() {
        fn fallible() -> Result<(), ConfigError> {
            // Simulate an FsError being raised inside a function that returns ConfigError.
            let fs_err = FsError::NotFound {
                path: String::from("/x"),
            };
            let _result: Result<(), FsError> = Err(fs_err);
            // Use `?` operator: From<FsError> for ConfigError is invoked.
            Err(FsError::NotFound {
                path: String::from("/x"),
            })?;
            Ok(())
        }
        let result = fallible();
        assert!(matches!(
            result,
            Err(ConfigError::Fs(FsError::NotFound { .. }))
        ));
    }

    // ---- Debug formatting ----

    #[test]
    fn test_debug_format() {
        let err = ConfigError::Internal(String::from("test"));
        let debug = format!("{:?}", err);
        assert!(debug.contains("Internal"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_debug_schema_violation() {
        let err = ConfigError::SchemaViolation {
            field: String::from("port"),
            reason: String::from("missing"),
        };
        let debug = format!("{:?}", err);
        assert!(debug.contains("SchemaViolation"));
        assert!(debug.contains("port"));
        assert!(debug.contains("missing"));
    }
}
