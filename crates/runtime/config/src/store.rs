//! 配置存储层（ARCH-G01）— `ConfigStore`：读写 + 版本管理。
//!
//! 从 `ConfigManager`（52.7KB）拆分出的纯存储/版本组件：
//! - 持有 filesystem、root_dir、time source、内存缓存、格式表、版本历史；
//! - 提供 load/save/get/set_internal/reload/rollback/list_versions；
//! - **不**持有 watcher 与 audit log（编排职责归 `ConfigManager` facade，
//!   ADR-0029/ARCH-G01：事务与存储可独立测试）。
//!
//! 事务（两阶段提交 + audit）见 `transaction.rs`；编排 facade 见 `manager.rs`。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use eneros_fs::{FileSystem, Lfs, OpenFlags};

use crate::error::ConfigError;
use crate::loader::{ConfigLoader, JsonLoader, TomlLoader};
use crate::schema::ConfigValue;
use crate::version::VersionHistory;
use crate::ConfigFormat;

/// 配置存储：读写 + 版本（ARCH-G01）。
pub struct ConfigStore {
    /// The littlefs2 filesystem instance (concrete type, per v0.25.0 pattern).
    fs: Lfs,
    /// Root directory for config files (e.g. `"/config"`).
    root_dir: String,
    /// Injected time source (epoch seconds).
    time_fn: fn() -> u64,
    /// In-memory config cache: base name -> ConfigValue::Table.
    configs: BTreeMap<String, ConfigValue>,
    /// Per-config format (base name -> ConfigFormat), recorded when the config
    /// is first loaded or created so that subsequent saves use the same format.
    formats: BTreeMap<String, ConfigFormat>,
    /// Version history per base name.
    histories: BTreeMap<String, VersionHistory>,
}

impl ConfigStore {
    /// Creates a new store, opening (or creating) `root_dir`.
    pub fn new(mut fs: Lfs, root_dir: &str, time_fn: fn() -> u64) -> Result<Self, ConfigError> {
        // Create the config directory (ignore "already exists").
        let _ = fs.mkdir(root_dir);
        let mut store = Self {
            fs,
            root_dir: String::from(root_dir),
            time_fn,
            configs: BTreeMap::new(),
            formats: BTreeMap::new(),
            histories: BTreeMap::new(),
        };
        // Best-effort load of existing config files.
        let _ = store.load_all();
        Ok(store)
    }

    /// Returns the config format inferred from the file extension.
    pub(crate) fn format_for(filename: &str) -> ConfigFormat {
        if filename.ends_with(".json") {
            ConfigFormat::Json
        } else {
            ConfigFormat::Toml
        }
    }

    /// Returns the loader for the given format.
    pub(crate) fn loader_for(format: ConfigFormat) -> Box<dyn ConfigLoader> {
        match format {
            ConfigFormat::Toml => Box::new(TomlLoader::new()),
            ConfigFormat::Json => Box::new(JsonLoader::new()),
        }
    }

    /// Extracts the base name (without extension) from a filename or path
    /// component. `"device.toml"` -> `"device"`, `"network.json"` -> `"network"`,
    /// `"device"` -> `"device"`.
    pub(crate) fn base_name(component: &str) -> String {
        // Strip directory prefix if present.
        let last = match component.rfind('/') {
            Some(idx) => &component[idx + 1..],
            None => component,
        };
        // Strip extension if present.
        match last.rfind('.') {
            Some(idx) if idx > 0 => String::from(&last[..idx]),
            _ => String::from(last),
        }
    }

    /// Builds the full filesystem path for a base name + format.
    fn full_path(&self, base_name: &str, format: ConfigFormat) -> String {
        format!("{}/{}.{}", self.root_dir, base_name, format.extension())
    }

    /// Resolves the format for a config, defaulting to TOML.
    fn format_of(&self, base_name: &str) -> ConfigFormat {
        self.formats
            .get(base_name)
            .copied()
            .unwrap_or(ConfigFormat::Toml)
    }

    /// Loads all `.toml` and `.json` config files from the root directory.
    ///
    /// Files that fail to parse are silently skipped (best-effort load).
    pub fn load_all(&mut self) -> Result<(), ConfigError> {
        // CFG-003：先恢复持久化版本历史（重启后回滚能力保留）。
        self.load_histories();
        let entries = self.fs.readdir(&self.root_dir)?;
        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let filename = entry.name;
            if filename.ends_with(".toml") || filename.ends_with(".json") {
                let base = Self::base_name(&filename);
                let format = Self::format_for(&filename);
                let path = format!("{}/{}", self.root_dir, filename);
                let loader = Self::loader_for(format);
                if let Ok(value) = loader.load_from_file(&mut self.fs, &path) {
                    self.configs.insert(base.clone(), value);
                    self.formats.insert(base, format);
                }
            }
        }
        Ok(())
    }

    /// Loads a configuration file by filename and caches it under its base name.
    pub fn load(&mut self, filename: &str) -> Result<&ConfigValue, ConfigError> {
        let base = Self::base_name(filename);
        let format = Self::format_for(filename);
        let path = self.full_path(&base, format);
        let loader = Self::loader_for(format);
        let value = loader.load_from_file(&mut self.fs, &path)?;
        self.configs.insert(base.clone(), value);
        self.formats.insert(base.clone(), format);
        Ok(self.configs.get(&base).expect("just inserted"))
    }

    /// Saves a cached config to its file by base name. Records a new version.
    pub fn save(&mut self, name: &str) -> Result<(), ConfigError> {
        let base = Self::base_name(name);
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let path = self.full_path(&base, format);

        let value = self
            .configs
            .get(&base)
            .ok_or_else(|| ConfigError::NotFound { path: base.clone() })?;

        loader.save_to_file(&mut self.fs, &path, value)?;

        // Record version snapshot.
        let data = loader.serialize(value)?;
        let history = self.histories.entry(base).or_default();
        history.record((self.time_fn)(), data);

        Ok(())
    }

    /// Gets the value at a dotted path (e.g. `"device.port"`).
    ///
    /// The first component is the config base name (without extension).
    pub fn get(&self, path: &str) -> Option<&ConfigValue> {
        let (name, rest) = split_first_component(path)?;
        let base = Self::base_name(name);
        let root = self.configs.get(&base)?;
        get_nested(root, rest)
    }

    /// Gets the value at a dotted path, returning `default` if not found.
    pub fn get_or_default(&self, path: &str, default: ConfigValue) -> ConfigValue {
        match self.get(path) {
            Some(v) => v.clone(),
            None => default,
        }
    }

    /// Internal write path: loads the config on demand, updates the nested
    /// value, persists to disk, and records a version snapshot. Does **not**
    /// notify watchers or write the audit log — callers (`ConfigManager::set`,
    /// `compare_and_swap`, `Transaction::commit`) own those concerns.
    ///
    /// Returns the previous value at `path`, if any.
    pub(crate) fn set_internal(
        &mut self,
        path: &str,
        value: ConfigValue,
    ) -> Result<Option<ConfigValue>, ConfigError> {
        let (name, rest) = split_first_component(path)
            .ok_or_else(|| ConfigError::Internal(String::from("empty config path")))?;
        let base = Self::base_name(name);

        // CFG-003：NaN/Infinity 浮点值无法闭环序列化（JSON/TOML 均无 NaN），
        // 在写缓存与磁盘前拒绝（fail-closed，防止脏数据进盘）。
        if let ConfigValue::Float(f) = &value {
            if f.is_nan() || f.is_infinite() {
                return Err(ConfigError::InvalidValue(format!(
                    "Float({f}) is not serializable (NaN/Infinity)"
                )));
            }
        }

        // Load the config if not already cached. If the file doesn't exist,
        // start with an empty table and default to TOML format.
        if !self.configs.contains_key(&base) {
            let format = self
                .formats
                .get(&base)
                .copied()
                .unwrap_or(ConfigFormat::Toml);
            let loader = Self::loader_for(format);
            let fpath = self.full_path(&base, format);
            let value = match loader.load_from_file(&mut self.fs, &fpath) {
                Ok(v) => v,
                Err(ConfigError::Fs(eneros_fs::FsError::NotFound { .. })) => {
                    ConfigValue::Table(BTreeMap::new())
                }
                Err(e) => return Err(e),
            };
            self.configs.insert(base.clone(), value);
            self.formats.insert(base.clone(), format);
        }

        // Update the nested value.
        let old_value;
        {
            let root = self
                .configs
                .get_mut(&base)
                .ok_or_else(|| ConfigError::Internal(String::from("config missing after load")))?;
            old_value = set_nested(root, rest, value);
        }

        // Persist to file.
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let fpath = self.full_path(&base, format);
        // CFG-002：保存失败必须回滚内存缓存（旧实现缓存已被 set_nested
        // 修改，磁盘失败后缓存与磁盘不一致——失败写污染缓存）。
        if let Err(e) = loader.save_to_file(
            &mut self.fs,
            &fpath,
            self.configs.get(&base).expect("present"),
        ) {
            if let Some(prev) = old_value.clone() {
                if let Some(root) = self.configs.get_mut(&base) {
                    let _ = set_nested(root, rest, prev);
                }
            }
            return Err(e);
        }

        // Record version.
        let data = loader.serialize(self.configs.get(&base).expect("present"))?;
        let history = self.histories.entry(base).or_default();
        history.record((self.time_fn)(), data);
        // CFG-003：历史持久化（best-effort——历史是回滚辅助数据，主配置
        // 已写盘；磁盘满等异常下历史缺失不应使已成功的写回滚失败）。
        let _ = self.persist_histories();

        Ok(old_value)
    }

    /// CFG-002：删除嵌套键（缓存 + 磁盘 + 版本历史），返回被删值。
    ///
    /// 供事务回滚的 create-delete 路径使用（旧实现 revert 对新建路径静默
    /// 不删除，回滚后残留脏数据）。
    pub(crate) fn delete_internal(
        &mut self,
        path: &str,
    ) -> Result<Option<ConfigValue>, ConfigError> {
        let (name, rest) = split_first_component(path)
            .ok_or_else(|| ConfigError::Internal(String::from("empty config path")))?;
        let base = Self::base_name(name);

        if !self.configs.contains_key(&base) {
            return Ok(None);
        }

        let deleted;
        {
            let root = self
                .configs
                .get_mut(&base)
                .ok_or_else(|| ConfigError::Internal(String::from("config missing")))?;
            deleted = delete_nested(root, rest);
        }

        if deleted.is_none() {
            return Ok(None);
        }

        // 持久化删除后的配置。
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let fpath = self.full_path(&base, format);
        loader.save_to_file(
            &mut self.fs,
            &fpath,
            self.configs.get(&base).expect("present"),
        )?;

        // 记录版本。
        let data = loader.serialize(self.configs.get(&base).expect("present"))?;
        let history = self.histories.entry(base).or_default();
        history.record((self.time_fn)(), data);
        // CFG-003：历史持久化（best-effort）。
        let _ = self.persist_histories();

        Ok(deleted)
    }

    /// Reloads a config file from disk, returning `(old_value, new_value)`
    /// for watcher notification by the caller.
    pub(crate) fn reload_value(
        &mut self,
        name: &str,
    ) -> Result<(Option<ConfigValue>, ConfigValue), ConfigError> {
        let base = Self::base_name(name);
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let path = self.full_path(&base, format);
        let new_value = loader.load_from_file(&mut self.fs, &path)?;
        let old_value = self.configs.insert(base.clone(), new_value.clone());
        self.formats.insert(base.clone(), format);
        Ok((old_value, new_value))
    }

    /// Rolls back a config to a specific version, returning
    /// `(old_value, new_value)` for watcher notification by the caller.
    /// Verifies the CRC32 checksum before restoring; does not record a new version.
    pub(crate) fn rollback_value(
        &mut self,
        name: &str,
        version: u64,
    ) -> Result<(Option<ConfigValue>, ConfigValue), ConfigError> {
        let base = Self::base_name(name);

        let history = self
            .histories
            .get(&base)
            .ok_or(ConfigError::VersionNotFound { version })?;
        let entry = history
            .get(version)
            .ok_or(ConfigError::VersionNotFound { version })?;

        if !entry.verify() {
            return Err(ConfigError::ChecksumMismatch);
        }

        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let value = loader.parse(&entry.data)?;

        let old_value = self.configs.insert(base.clone(), value.clone());
        let fpath = self.full_path(&base, format);
        loader.save_to_file(&mut self.fs, &fpath, &value)?;

        Ok((old_value, value))
    }

    /// CFG-003：持久化全部版本历史到 `<root>/.history.json`。
    ///
    /// 旧实现版本历史只在内存（重启即丢失，无法回滚到磁盘记录的版本）。
    pub(crate) fn persist_histories(&mut self) -> Result<(), ConfigError> {
        let path = format!("{}/.history.json", self.root_dir);
        let data = serde_json::to_vec(&self.histories)
            .map_err(|e| ConfigError::Internal(format!("history serialize: {e}")))?;
        let mut file = self
            .fs
            .open(&path, OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE)
            .map_err(ConfigError::Fs)?;
        file.write(&mut self.fs, &data).map_err(ConfigError::Fs)?;
        Ok(())
    }

    /// CFG-003：从 `<root>/.history.json` 恢复版本历史（启动时调用）。
    pub(crate) fn load_histories(&mut self) {
        let path = format!("{}/.history.json", self.root_dir);
        let size = match self.fs.stat(&path) {
            Ok(s) => s.size,
            Err(eneros_fs::FsError::NotFound { .. }) => return,
            Err(_) => return, // 无法读取时不阻断启动（历史是辅助数据）
        };
        let mut file = match self.fs.open(&path, OpenFlags::READ) {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut buf = alloc::vec![0u8; size as usize];
        let mut filled = 0usize;
        loop {
            if filled >= buf.len() {
                break;
            }
            match file.read(&mut self.fs, &mut buf[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => filled += n,
            }
        }
        buf.truncate(filled);
        if let Ok(hist) = serde_json::from_slice::<BTreeMap<String, VersionHistory>>(&buf) {
            self.histories = hist;
        }
    }

    /// Lists all version numbers for a config.
    pub fn list_versions(&self, name: &str) -> Result<Vec<u64>, ConfigError> {
        let base = Self::base_name(name);
        let history = self
            .histories
            .get(&base)
            .ok_or(ConfigError::VersionNotFound { version: 0 })?;
        Ok(history.list_versions())
    }

    /// Returns the injected time function.
    pub(crate) fn time_source(&self) -> fn() -> u64 {
        self.time_fn
    }

    /// Mutable access to the filesystem (audit log persistence).
    pub(crate) fn fs_mut(&mut self) -> &mut Lfs {
        &mut self.fs
    }

    /// 测试访问器：内存缓存（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn configs_ref(&self) -> &BTreeMap<String, ConfigValue> {
        &self.configs
    }

    /// 测试访问器：filesystem。
    #[cfg(test)]
    pub(crate) fn fs_ref(&self) -> &Lfs {
        &self.fs
    }

    /// 测试访问器：可变内存缓存（characterization 测试用）。
    #[cfg(test)]
    pub(crate) fn configs_mut(&mut self) -> &mut BTreeMap<String, ConfigValue> {
        &mut self.configs
    }
}

/// Splits `"a.b.c"` into `("a", Some("b.c"))`.
pub(crate) fn split_first_component(path: &str) -> Option<(&str, Option<&str>)> {
    if path.is_empty() {
        return None;
    }
    match path.find('.') {
        Some(idx) => Some((&path[..idx], Some(&path[idx + 1..]))),
        None => Some((path, None)),
    }
}

/// Gets a value from a nested table structure following `rest` path components.
pub(crate) fn get_nested<'a>(root: &'a ConfigValue, rest: Option<&str>) -> Option<&'a ConfigValue> {
    let mut current = root;
    for part in rest.into_iter().flat_map(|s| s.split('.')) {
        match current {
            ConfigValue::Table(table) => current = table.get(part)?,
            _ => return None,
        }
    }
    Some(current)
}

/// Sets a nested value following `rest` path components. Returns the old value.
pub(crate) fn set_nested(
    root: &mut ConfigValue,
    rest: Option<&str>,
    value: ConfigValue,
) -> Option<ConfigValue> {
    let mut parts: Vec<&str> = rest.into_iter().flat_map(|s| s.split('.')).collect();
    if parts.is_empty() {
        let old = root.clone();
        *root = value;
        return Some(old);
    }
    let last = parts.pop().expect("non-empty");
    let mut current = root;
    for part in &parts {
        if let ConfigValue::Table(table) = current {
            current = table
                .entry(String::from(*part))
                .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
        } else {
            return None;
        }
    }
    if let ConfigValue::Table(table) = current {
        table.insert(String::from(last), value)
    } else {
        None
    }
}

/// CFG-002：从嵌套配置删除键，返回被删值（`None` 表示键不存在）。
pub(crate) fn delete_nested(root: &mut ConfigValue, rest: Option<&str>) -> Option<ConfigValue> {
    let mut parts: Vec<&str> = rest.into_iter().flat_map(|s| s.split('.')).collect();
    if parts.is_empty() {
        let old = root.clone();
        *root = ConfigValue::Table(BTreeMap::new());
        return Some(old);
    }
    let last = parts.pop().expect("non-empty");
    let mut current = root;
    for part in &parts {
        if let ConfigValue::Table(table) = current {
            current = table
                .entry(String::from(*part))
                .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
        } else {
            return None;
        }
    }
    if let ConfigValue::Table(table) = current {
        table.remove(last)
    } else {
        None
    }
}

/// Compares two values for equality (used by compare-and-swap).
pub(crate) fn values_equal(a: &ConfigValue, b: &ConfigValue) -> bool {
    a == b
}

impl fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigStore")
            .field("root_dir", &self.root_dir)
            .field("configs", &self.configs)
            .finish()
    }
}
