//! Configuration hot reload (v0.9.0).
//!
//! Provides polling-based configuration reload for safe-to-update config
//! fields. When `eneros.toml` changes on disk, the watcher reloads the file,
//! applies environment overrides, validates, and updates the shared config
//! handle. Only fields that are safe to change at runtime are applied; fields
//! that require a restart (bind address, TLS, network model, device
//! connections) are logged but not applied.
//!
//! A manual reload endpoint is also exposed at `POST /api/config/reload`.
//!
//! The file watcher uses a 2-second polling interval (checking the file's
//! modification time) to avoid external dependencies on platform-specific
//! file notification systems. This is sufficient for config files which
//! change infrequently.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use eneros_core::config::EnerOSConfig;
use parking_lot::RwLock;
use serde::Serialize;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Shared, runtime-mutable configuration handle.
///
/// All components that need to read config values at runtime should hold a
/// clone of this `Arc` and call `.read()` to access the current config.
/// The config can be updated atomically via `.write()` or the
/// [`ConfigWatcher::reload`] method.
pub type SharedConfig = Arc<RwLock<EnerOSConfig>>;

/// Wrap an `EnerOSConfig` into a shared handle.
pub fn shared(config: EnerOSConfig) -> SharedConfig {
    Arc::new(RwLock::new(config))
}

/// Result of a config reload operation.
#[derive(Debug, Serialize)]
pub struct ReloadResult {
    pub success: bool,
    pub message: String,
    pub applied_fields: Vec<String>,
    pub skipped_fields: Vec<String>,
}

/// Watches a config file for changes and reloads it into a `SharedConfig`.
///
/// Uses polling (checking `mtime` every 2 seconds) to detect file changes.
/// This avoids platform-specific file notification dependencies while
/// providing reliable change detection for config files.
pub struct ConfigWatcher {
    config: SharedConfig,
    config_path: PathBuf,
    handle: Option<JoinHandle<()>>,
}

impl ConfigWatcher {
    /// Create a new config file watcher.
    ///
    /// The watcher is not started until [`ConfigWatcher::start`] is called.
    pub fn new(config: SharedConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            handle: None,
        }
    }

    /// Start watching the config file for changes.
    ///
    /// Spawns a background task that polls the file's modification time
    /// every 2 seconds and triggers a reload when the file changes.
    pub fn start(mut self) -> Self {
        let config = self.config.clone();
        let path = self.config_path.clone();

        // Record the initial mtime so we don't reload on startup
        let initial_mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let handle = tokio::spawn(async move {
            info!("Config file watcher started (polling: {})", path.display());
            let mut last_mtime = initial_mtime;
            let poll_interval = std::time::Duration::from_secs(2);

            loop {
                tokio::time::sleep(poll_interval).await;

                let current_mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
                    Ok(t) => t,
                    Err(_) => {
                        // File might have been deleted temporarily during save
                        continue;
                    }
                };

                if current_mtime <= last_mtime {
                    continue;
                }

                // File was modified — wait a bit for writes to settle
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                last_mtime = current_mtime;

                match reload_from_file(&config, &path) {
                    Ok(result) => {
                        if result.success {
                            info!(
                                "Config reloaded from {}: applied={}, skipped={}",
                                path.display(),
                                result.applied_fields.len(),
                                result.skipped_fields.len()
                            );
                        } else {
                            warn!("Config reload failed: {}", result.message);
                        }
                    }
                    Err(e) => {
                        error!("Config reload error: {}", e);
                    }
                }
            }
        });

        self.handle = Some(handle);
        self
    }

    /// Manually trigger a config reload from the file.
    pub fn reload(&self) -> Result<ReloadResult, String> {
        reload_from_file(&self.config, &self.config_path).map_err(|e| e.to_string())
    }

    /// Stop the watcher (aborts the background polling task).
    pub fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Reload config from file and apply safe-to-reload fields.
///
/// This function:
/// 1. Reads the config file from `path`
/// 2. Applies environment variable overrides
/// 3. Validates the new config
/// 4. Compares with the current config and applies safe-to-reload fields
/// 5. Logs which fields were applied vs skipped
fn reload_from_file(
    shared: &SharedConfig,
    path: &Path,
) -> Result<ReloadResult, Box<dyn std::error::Error>> {
    let new_config = EnerOSConfig::load_with_env_overrides(Some(path.to_str().unwrap_or("")))?;

    let current = shared.read().clone();

    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    // `updated` starts as a copy of `current` and gets mutated field-by-field.
    let mut updated = current;
    // We compare `updated` (original values) against `new_config` to detect
    // changes, then apply safe-to-reload fields to `updated`.

    // ── Safe to reload: observability.log_level ──
    if updated.observability.log_level != new_config.observability.log_level {
        let level = new_config.observability.log_level.clone();
        if let Ok(l) = parse_level(&level) {
            log::set_max_level(match l {
                tracing::Level::ERROR => log::LevelFilter::Error,
                tracing::Level::WARN => log::LevelFilter::Warn,
                tracing::Level::INFO => log::LevelFilter::Info,
                tracing::Level::DEBUG => log::LevelFilter::Debug,
                tracing::Level::TRACE => log::LevelFilter::Trace,
            });
            updated.observability.log_level = level;
            applied.push("observability.log_level".to_string());
            info!("Hot-reloaded log_level → {}", updated.observability.log_level);
        }
    }

    // ── Safe to reload: observability.enable_metrics ──
    if updated.observability.enable_metrics != new_config.observability.enable_metrics {
        updated.observability.enable_metrics = new_config.observability.enable_metrics;
        applied.push("observability.enable_metrics".to_string());
        info!("Hot-reloaded enable_metrics → {}", updated.observability.enable_metrics);
    }

    // ── Safe to reload: observability.enable_tracing ──
    if updated.observability.enable_tracing != new_config.observability.enable_tracing {
        // Tracing toggle requires subscriber re-init; log as skipped
        skipped.push("observability.enable_tracing (requires restart)".to_string());
    }

    // ── Safe to reload: scada.fast_interval_ms / normal_interval_ms ──
    // Note: These values are stored in the shared config but the running
    // dual scan pipelines read the interval at startup. The new values
    // will take effect on next pipeline restart. We still update the
    // shared config so that new pipelines (if restarted) pick them up.
    if updated.scada.fast_interval_ms != new_config.scada.fast_interval_ms {
        updated.scada.fast_interval_ms = new_config.scada.fast_interval_ms;
        applied.push("scada.fast_interval_ms (effective on next pipeline restart)".to_string());
    }
    if updated.scada.normal_interval_ms != new_config.scada.normal_interval_ms {
        updated.scada.normal_interval_ms = new_config.scada.normal_interval_ms;
        applied.push("scada.normal_interval_ms (effective on next pipeline restart)".to_string());
    }

    // ── Safe to reload: emergency thresholds ──
    if updated.emergency.alert_frequency_hz != new_config.emergency.alert_frequency_hz {
        updated.emergency.alert_frequency_hz = new_config.emergency.alert_frequency_hz;
        applied.push("emergency.alert_frequency_hz".to_string());
    }
    if updated.emergency.emergency_frequency_hz != new_config.emergency.emergency_frequency_hz {
        updated.emergency.emergency_frequency_hz = new_config.emergency.emergency_frequency_hz;
        applied.push("emergency.emergency_frequency_hz".to_string());
    }
    if updated.emergency.alert_voltage_pu != new_config.emergency.alert_voltage_pu {
        updated.emergency.alert_voltage_pu = new_config.emergency.alert_voltage_pu;
        applied.push("emergency.alert_voltage_pu".to_string());
    }
    if updated.emergency.emergency_voltage_pu != new_config.emergency.emergency_voltage_pu {
        updated.emergency.emergency_voltage_pu = new_config.emergency.emergency_voltage_pu;
        applied.push("emergency.emergency_voltage_pu".to_string());
    }

    // ── Safe to reload: powerflow parameters ──
    if updated.powerflow.tolerance != new_config.powerflow.tolerance {
        updated.powerflow.tolerance = new_config.powerflow.tolerance;
        applied.push("powerflow.tolerance".to_string());
    }
    if updated.powerflow.max_iterations != new_config.powerflow.max_iterations {
        updated.powerflow.max_iterations = new_config.powerflow.max_iterations;
        applied.push("powerflow.max_iterations".to_string());
    }

    // ── NOT safe to reload (require restart) ──
    // Note: `updated` still holds the original values for these fields
    // (we only modified safe-to-reload fields above), so we compare
    // `updated` against `new_config` to detect changes.
    if updated.api.host != new_config.api.host || updated.api.port != new_config.api.port {
        skipped.push("api.host/port (requires restart)".to_string());
    }
    if updated.api.enable_tls != new_config.api.enable_tls
        || updated.api.tls_cert_path != new_config.api.tls_cert_path
    {
        skipped.push("api.tls_* (requires restart)".to_string());
    }
    if updated.network.source != new_config.network.source
        || updated.network.path != new_config.network.path
    {
        skipped.push("network.* (requires restart)".to_string());
    }
    if updated.devices.len() != new_config.devices.len() {
        skipped.push("devices (requires restart)".to_string());
    }
    if updated.scada.source != new_config.scada.source
        || updated.scada.iec104_addr != new_config.scada.iec104_addr
    {
        skipped.push("scada.source/iec104_addr (requires restart)".to_string());
    }
    if updated.security.jwt_secret != new_config.security.jwt_secret {
        skipped.push("security.jwt_secret (requires restart to avoid invalidating tokens)".to_string());
    }
    if updated.eventbus.max_queue_size != new_config.eventbus.max_queue_size {
        skipped.push("eventbus.max_queue_size (requires restart)".to_string());
    }

    // Write the updated config back to the shared handle
    *shared.write() = updated;

    Ok(ReloadResult {
        success: true,
        message: format!(
            "Config reloaded: {} field(s) applied, {} field(s) skipped",
            applied.len(),
            skipped.len()
        ),
        applied_fields: applied,
        skipped_fields: skipped,
    })
}

fn parse_level(s: &str) -> Result<tracing::Level, String> {
    match s.to_lowercase().as_str() {
        "error" => Ok(tracing::Level::ERROR),
        "warn" => Ok(tracing::Level::WARN),
        "info" => Ok(tracing::Level::INFO),
        "debug" => Ok(tracing::Level::DEBUG),
        "trace" => Ok(tracing::Level::TRACE),
        other => Err(format!("invalid level '{}'", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_config_read_write() {
        let config = EnerOSConfig::default();
        let shared = shared(config);

        // Read
        let level = shared.read().observability.log_level.clone();
        assert_eq!(level, "info");

        // Write
        {
            let mut w = shared.write();
            w.observability.log_level = "debug".to_string();
        }

        // Read again
        assert_eq!(shared.read().observability.log_level, "debug");
    }

    #[test]
    fn test_parse_level() {
        assert_eq!(parse_level("info").unwrap(), tracing::Level::INFO);
        assert_eq!(parse_level("DEBUG").unwrap(), tracing::Level::DEBUG);
        assert!(parse_level("invalid").is_err());
    }
}
