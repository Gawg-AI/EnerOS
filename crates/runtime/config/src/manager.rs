//! [`ConfigManager`] — 配置管理编排 facade（ARCH-G01 拆分）。
//!
//! 拆分前为 52.7KB 单一模块；拆分后：
//! - 存储与版本（fs/缓存/格式/历史 + load/save/get/set_internal/reload/rollback）
//!   归 [`crate::store::ConfigStore`]；
//! - 两阶段提交事务（stage/commit/abort + audit）见 `transaction.rs`；
//! - 本模块只保留**编排**：watcher 通知、audit log、事务入口与对外 API 转发。
//!
//! 公共 API 与拆分前完全一致（characterization 基线：`manager_tests.rs`
//! 48 项测试在拆分前后必须全部通过）。

use alloc::boxed::Box;
#[cfg(test)]
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_fs::Lfs;

use crate::audit::{AuditEntry, AuditLog, AuditOp, serialize_value_for_audit};
use crate::error::ConfigError;
use crate::schema::ConfigValue;
use crate::store::{self, ConfigStore};
use crate::transaction::{CasOutcome, Transaction};
use crate::watcher::{ConfigWatcher, WatcherRegistry};

/// The main configuration manager (编排 facade，ARCH-G01).
///
/// 持有 [`ConfigStore`]（存储/版本）、watcher 注册表与审计日志；
/// 对外提供与拆分前一致的 API。
pub struct ConfigManager {
    /// 存储层（fs + 缓存 + 版本历史）。
    store: ConfigStore,
    /// Watcher registry.
    watchers: WatcherRegistry,
    /// Append-only audit log of every mutating operation.
    audit_log: AuditLog,
}

impl ConfigManager {
    /// Creates a new ConfigManager.
    ///
    /// Opens (or creates) the `root_dir` directory on the filesystem, then
    /// loads all `.toml`/`.json` files found in that directory.
    /// The `time_fn` callback provides timestamps for version records.
    pub fn new(fs: Lfs, root_dir: &str, time_fn: fn() -> u64) -> Result<Self, ConfigError> {
        let store = ConfigStore::new(fs, root_dir, time_fn)?;
        let mut mgr = Self {
            store,
            watchers: WatcherRegistry::new(),
            audit_log: AuditLog::new(AuditLog::default_path()),
        };
        // Best-effort load of existing config files; ignore errors (empty dir
        // or unreadable files are not fatal for construction).
        let _ = mgr.load_all();
        Ok(mgr)
    }

    /// 测试访问器：filesystem（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn fs(&self) -> &Lfs {
        self.store.fs_ref()
    }

    /// 测试访问器：内存缓存（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn configs(&self) -> &BTreeMap<String, ConfigValue> {
        self.store.configs_ref()
    }

    /// 测试访问器：可变 filesystem（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn fs_mut(&mut self) -> &mut Lfs {
        self.store.fs_mut()
    }

    /// 测试访问器：可变内存缓存（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn configs_mut(&mut self) -> &mut BTreeMap<String, ConfigValue> {
        self.store.configs_mut()
    }

    /// Loads all `.toml` and `.json` config files from the root directory.
    pub fn load_all(&mut self) -> Result<(), ConfigError> {
        self.store.load_all()
    }

    /// Loads a configuration file by filename (e.g. `"device.toml"` or
    /// `"device.json"`).
    pub fn load(&mut self, filename: &str) -> Result<&ConfigValue, ConfigError> {
        self.store.load(filename)
    }

    /// Saves a cached config to its file by base name (e.g. `"device"`).
    pub fn save(&mut self, name: &str) -> Result<(), ConfigError> {
        self.store.save(name)
    }

    /// Gets the value at a dotted path (e.g. `"device.port"`).
    pub fn get(&self, path: &str) -> Option<&ConfigValue> {
        self.store.get(path)
    }

    /// Gets the value at a dotted path, returning `default` if not found.
    pub fn get_or_default(&self, path: &str, default: ConfigValue) -> ConfigValue {
        self.store.get_or_default(path, default)
    }

    /// Sets a value at a dotted path and persists the config file.
    ///
    /// Records a new version, notifies watchers, and appends an audit entry.
    pub fn set(&mut self, path: &str, value: ConfigValue) -> Result<(), ConfigError> {
        let old_value = self.set_internal(path, value.clone())?;

        // Audit (after the write succeeded).
        self.audit(AuditEntry {
            timestamp: (self.time_source())(),
            actor: String::from("config"),
            op: AuditOp::Set,
            path: String::from(path),
            old_value: old_value.as_ref().map(serialize_value_for_audit),
            new_value: Some(serialize_value_for_audit(&value)),
            success: true,
        })?;

        // Notify watchers.
        let (name, _) = store::split_first_component(path)
            .ok_or_else(|| ConfigError::Internal(String::from("empty config path")))?;
        let base = ConfigStore::base_name(name);
        self.watchers
            .notify(&base, path, old_value.as_ref(), Some(&value));

        Ok(())
    }

    /// Internal write path (storage + version, no watcher/audit) —
    /// used by `set`, `compare_and_swap`, and `Transaction::commit`.
    pub(crate) fn set_internal(
        &mut self,
        path: &str,
        value: ConfigValue,
    ) -> Result<Option<ConfigValue>, ConfigError> {
        self.store.set_internal(path, value)
    }

    /// CFG-002：内部删除路径（事务回滚 create-delete 用）。
    pub(crate) fn delete_internal(
        &mut self,
        path: &str,
    ) -> Result<Option<ConfigValue>, ConfigError> {
        self.store.delete_internal(path)
    }

    /// Atomic compare-and-swap: only applies `new` if the current value at
    /// `path` equals `expected`. Returns [`CasOutcome::Swapped`] on success,
    /// or [`CasOutcome::Mismatch`] (with the current value) when the
    /// comparison fails — the mismatch is **not** an error.
    ///
    /// `expected = None` means "path must not currently exist" (create-only).
    pub fn compare_and_swap(
        &mut self,
        path: &str,
        expected: Option<&ConfigValue>,
        new: ConfigValue,
    ) -> Result<CasOutcome, ConfigError> {
        let current = self.get(path).cloned();
        let matches = match (expected, &current) {
            (None, None) => true,
            (Some(e), Some(c)) => store::values_equal(e, c),
            _ => false,
        };
        if !matches {
            self.audit(AuditEntry {
                timestamp: (self.time_source())(),
                actor: String::from("config"),
                op: AuditOp::Cas,
                path: String::from(path),
                old_value: current.as_ref().map(serialize_value_for_audit),
                new_value: Some(serialize_value_for_audit(&new)),
                success: false,
            })?;
            return Ok(CasOutcome::Mismatch {
                path: String::from(path),
                current: current.as_ref().map(serialize_value_for_audit),
            });
        }
        self.set_internal(path, new.clone())?;
        self.audit(AuditEntry {
            timestamp: (self.time_source())(),
            actor: String::from("config"),
            op: AuditOp::Cas,
            path: String::from(path),
            old_value: current.as_ref().map(serialize_value_for_audit),
            new_value: Some(serialize_value_for_audit(&new)),
            success: true,
        })?;
        Ok(CasOutcome::Swapped)
    }

    /// Begins a new atomic transaction. Stage writes with
    /// [`Transaction::stage`], then [`Transaction::commit`] or
    /// [`Transaction::abort`].
    pub fn begin_transaction(&mut self, actor: String) -> Transaction<'_> {
        Transaction::new(self, actor)
    }

    /// Appends an entry to the durable audit log. Exposed so that
    /// `Transaction` (in the same crate) can record commit/abort entries.
    pub(crate) fn audit(&mut self, entry: AuditEntry) -> Result<(), ConfigError> {
        self.audit_log.append(self.store.fs_mut(), entry)
    }

    /// Returns a slice of the most recent in-memory audit entries.
    pub fn audit_recent(&self) -> &[AuditEntry] {
        self.audit_log.recent()
    }

    /// Reads back every parseable entry from the durable audit file.
    pub fn audit_read_all(&mut self) -> Result<Vec<AuditEntry>, ConfigError> {
        AuditLog::read_all(self.store.fs_mut(), &AuditLog::default_path())
    }

    /// Returns the injected time function (used by `Transaction`).
    pub(crate) fn time_source(&self) -> fn() -> u64 {
        self.store.time_source()
    }

    /// Reloads a config file from disk (manual hot reload).
    pub fn reload(&mut self, name: &str) -> Result<(), ConfigError> {
        let (old_value, new_value) = self.store.reload_value(name)?;
        let base = ConfigStore::base_name(name);
        self.watchers
            .notify(&base, "*", old_value.as_ref(), Some(&new_value));
        Ok(())
    }

    /// Rolls back a config to a specific version.
    ///
    /// Verifies the CRC32 checksum before restoring. Does not record a new
    /// version (rollback is not a new edit).
    pub fn rollback(&mut self, name: &str, version: u64) -> Result<(), ConfigError> {
        let (old_value, value) = self.store.rollback_value(name, version)?;
        let base = ConfigStore::base_name(name);
        self.watchers
            .notify(&base, "*", old_value.as_ref(), Some(&value));
        Ok(())
    }

    /// Lists all version numbers for a config.
    pub fn list_versions(&self, name: &str) -> Result<Vec<u64>, ConfigError> {
        self.store.list_versions(name)
    }

    /// Registers a watcher for a config name (base name or filename).
    pub fn register_watcher(&mut self, name: &str, watcher: Box<dyn ConfigWatcher>) {
        let base = ConfigStore::base_name(name);
        self.watchers.register(&base, watcher);
    }
}

// ============================================================================
// Characterization 行为基线（ARCH-G01：拆分前后必须全绿的 48 项测试）
// ============================================================================
#[cfg(test)]
mod manager_tests;
