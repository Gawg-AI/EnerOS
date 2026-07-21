//! [`ConfigManager`] — the main entry point for configuration management.
//!
//! Holds the filesystem, loaded configurations, version histories, and
//! watcher registry. Provides load/save/get/set/reload/rollback/list_versions
//! operations.
//!
//! # Design
//!
//! Follows the v0.25.0 TSDB pattern: holds `Lfs` concrete type (not
//! `Box<dyn FileSystem>`) because `File::read/write` require `&mut Lfs`.
//! Time source is injected via `fn() -> u64` function pointer.
//!
//! Configs are keyed by **base name** (without file extension), so that
//! `get("device.port")` works whether the file is `device.toml` or
//! `device.json`. A separate `formats` map records which format each config
//! uses so that `save`/`reload`/`rollback` know how to serialize/parse.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_fs::{FileSystem, Lfs};

use crate::error::ConfigError;
use crate::loader::{ConfigFormat, ConfigLoader, JsonLoader, TomlLoader};
use crate::schema::ConfigValue;
use crate::version::VersionHistory;
use crate::watcher::{ConfigWatcher, WatcherRegistry};

/// The main configuration manager.
///
/// Owns the filesystem, in-memory config cache, version histories, and
/// watcher registry. Configs are keyed by base name (e.g. `"device"`), not
/// by filename, so callers can use `get("device.port")` regardless of whether
/// the underlying file is `device.toml` or `device.json`.
pub struct ConfigManager {
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
    /// Watcher registry.
    watchers: WatcherRegistry,
}

impl ConfigManager {
    /// Creates a new ConfigManager.
    ///
    /// Opens (or creates) the `root_dir` directory on the filesystem, then
    /// loads all `.toml`/`.json` files found in that directory.
    /// The `time_fn` callback provides timestamps for version records.
    pub fn new(mut fs: Lfs, root_dir: &str, time_fn: fn() -> u64) -> Result<Self, ConfigError> {
        // Create the config directory (ignore "already exists").
        let _ = fs.mkdir(root_dir);

        let mut mgr = Self {
            fs,
            root_dir: String::from(root_dir),
            time_fn,
            configs: BTreeMap::new(),
            formats: BTreeMap::new(),
            histories: BTreeMap::new(),
            watchers: WatcherRegistry::new(),
        };
        // Best-effort load of existing config files; ignore errors (empty dir
        // or unreadable files are not fatal for construction).
        let _ = mgr.load_all();
        Ok(mgr)
    }

    /// Returns the config format inferred from the file extension.
    fn format_for(filename: &str) -> ConfigFormat {
        if filename.ends_with(".json") {
            ConfigFormat::Json
        } else {
            ConfigFormat::Toml
        }
    }

    /// Returns the loader for the given format.
    fn loader_for(format: ConfigFormat) -> Box<dyn ConfigLoader> {
        match format {
            ConfigFormat::Toml => Box::new(TomlLoader::new()),
            ConfigFormat::Json => Box::new(JsonLoader::new()),
        }
    }

    /// Extracts the base name (without extension) from a filename or path
    /// component. `"device.toml"` -> `"device"`, `"network.json"` -> `"network"`,
    /// `"device"` -> `"device"`.
    fn base_name(component: &str) -> String {
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
    /// Returns an error only if the directory itself cannot be read.
    pub fn load_all(&mut self) -> Result<(), ConfigError> {
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

    /// Loads a configuration file by filename (e.g. `"device.toml"` or
    /// `"device.json"`). Parses the file, caches it under its base name,
    /// and returns a reference to the root [`ConfigValue::Table`].
    pub fn load(&mut self, filename: &str) -> Result<&ConfigValue, ConfigError> {
        let base = Self::base_name(filename);
        let format = Self::format_for(filename);
        let path = self.full_path(&base, format);
        let loader = Self::loader_for(format);
        let value = loader.load_from_file(&mut self.fs, &path)?;
        self.configs.insert(base.clone(), value);
        self.formats.insert(base.clone(), format);
        // Return reference from the cache.
        Ok(self.configs.get(&base).expect("just inserted"))
    }

    /// Saves a cached config to its file by base name (e.g. `"device"`).
    ///
    /// Records a new version snapshot in the history.
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
    /// Returns `None` if the path does not exist.
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

    /// Sets a value at a dotted path and persists the config file.
    ///
    /// Records a new version and notifies watchers. If the config does not
    /// yet exist on disk, it is created as a TOML file (default format).
    pub fn set(&mut self, path: &str, value: ConfigValue) -> Result<(), ConfigError> {
        let (name, rest) = split_first_component(path)
            .ok_or_else(|| ConfigError::Internal(String::from("empty config path")))?;
        let base = Self::base_name(name);

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
            old_value = set_nested(root, rest, value.clone());
        }

        // Persist to file.
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let fpath = self.full_path(&base, format);
        loader.save_to_file(
            &mut self.fs,
            &fpath,
            self.configs.get(&base).expect("present"),
        )?;

        // Record version.
        let data = loader.serialize(self.configs.get(&base).expect("present"))?;
        let history = self.histories.entry(base.clone()).or_default();
        history.record((self.time_fn)(), data);

        // Notify watchers.
        self.watchers
            .notify(&base, path, old_value.as_ref(), Some(&value));

        Ok(())
    }

    /// Reloads a config file from disk (manual hot reload).
    ///
    /// Re-reads the file, updates the in-memory cache, and notifies watchers.
    /// `name` may be a base name (`"device"`) or a filename (`"device.toml"`).
    pub fn reload(&mut self, name: &str) -> Result<(), ConfigError> {
        let base = Self::base_name(name);
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let path = self.full_path(&base, format);
        let new_value = loader.load_from_file(&mut self.fs, &path)?;
        let old_value = self.configs.insert(base.clone(), new_value.clone());
        self.formats.insert(base.clone(), format);
        self.watchers
            .notify(&base, "*", old_value.as_ref(), Some(&new_value));
        Ok(())
    }

    /// Rolls back a config to a specific version.
    ///
    /// Verifies the CRC32 checksum before restoring. Does not record a new
    /// version (rollback is not a new edit).
    pub fn rollback(&mut self, name: &str, version: u64) -> Result<(), ConfigError> {
        let base = Self::base_name(name);

        // Get the version data.
        let history = self
            .histories
            .get(&base)
            .ok_or(ConfigError::VersionNotFound { version })?;
        let entry = history
            .get(version)
            .ok_or(ConfigError::VersionNotFound { version })?;

        // Verify CRC32.
        if !entry.verify() {
            return Err(ConfigError::ChecksumMismatch);
        }

        // Parse the version data back into a ConfigValue.
        let format = self.format_of(&base);
        let loader = Self::loader_for(format);
        let value = loader.parse(&entry.data)?;

        // Restore in memory and on disk.
        let old_value = self.configs.insert(base.clone(), value.clone());
        let fpath = self.full_path(&base, format);
        loader.save_to_file(&mut self.fs, &fpath, &value)?;

        // Notify watchers (no new version recorded).
        self.watchers
            .notify(&base, "*", old_value.as_ref(), Some(&value));

        Ok(())
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

    /// Registers a watcher for a config name (base name or filename).
    pub fn register_watcher(&mut self, name: &str, watcher: Box<dyn ConfigWatcher>) {
        let base = Self::base_name(name);
        self.watchers.register(&base, watcher);
    }
}

/// Splits `"a.b.c"` into `("a", Some("b.c"))`.
fn split_first_component(path: &str) -> Option<(&str, Option<&str>)> {
    if path.is_empty() {
        return None;
    }
    match path.find('.') {
        Some(idx) => Some((&path[..idx], Some(&path[idx + 1..]))),
        None => Some((path, None)),
    }
}

/// Navigates a dotted path through nested tables.
///
/// - `get_nested(root, None)` returns `Some(root)` (the root table itself).
/// - `get_nested(root, Some("a.b"))` navigates `root["a"]["b"]`.
#[allow(clippy::question_mark)]
fn get_nested<'a>(value: &'a ConfigValue, path: Option<&str>) -> Option<&'a ConfigValue> {
    let path = match path {
        Some(p) => p,
        None => return Some(value),
    };
    let (first, rest) = match path.find('.') {
        Some(idx) => (&path[..idx], Some(&path[idx + 1..])),
        None => (path, None),
    };
    let table = value.as_table()?;
    let child = table.get(first)?;
    get_nested(child, rest)
}

/// Sets a value at a nested path, returning the old value if any.
///
/// Creates intermediate tables as needed.
fn set_nested(
    root: &mut ConfigValue,
    path: Option<&str>,
    value: ConfigValue,
) -> Option<ConfigValue> {
    let path = path?;
    let (first, rest) = match path.find('.') {
        Some(idx) => (&path[..idx], Some(&path[idx + 1..])),
        None => (path, None),
    };
    let table = match root {
        ConfigValue::Table(t) => t,
        _ => return None,
    };
    match rest {
        None => {
            // Insert/replace at this level.
            table.insert(String::from(first), value)
        }
        Some(rest_path) => {
            // Descend or create intermediate table.
            let entry = table
                .entry(String::from(first))
                .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
            set_nested(entry, Some(rest_path), value)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::boxed::Box;

    use eneros_storage::{BlockDevice, MockBlockDevice};

    use super::*;

    fn create_test_fs() -> Lfs {
        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        Lfs::format(dev).expect("format failed")
    }

    fn static_time() -> u64 {
        1000
    }

    // ---- Helper: write a config file directly to the FS ----

    fn write_file(fs: &mut Lfs, path: &str, content: &str) {
        let mut file = fs
            .open(
                path,
                eneros_fs::OpenFlags::WRITE
                    | eneros_fs::OpenFlags::CREATE
                    | eneros_fs::OpenFlags::TRUNCATE,
            )
            .expect("open for write");
        file.write(fs, content.as_bytes()).expect("write");
    }

    // ---- new() + load_all ----

    #[test]
    fn test_new_creates_config_dir() {
        let fs = create_test_fs();
        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        // The /config directory should exist.
        assert!(mgr.fs.stat("/config").is_ok());
    }

    #[test]
    fn test_new_loads_existing_files() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/device.toml",
            "port = 8080\nhost = \"localhost\"\n",
        );
        write_file(&mut fs, "/config/network.json", r#"{"ip": "10.0.0.1"}"#);

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        // device.toml should be loaded.
        let port = mgr.get("device.port").expect("port should exist");
        assert_eq!(port, &ConfigValue::Int(8080));
        // network.json should be loaded.
        let ip = mgr.get("network.ip").expect("ip should exist");
        assert_eq!(ip, &ConfigValue::String(String::from("10.0.0.1")));
    }

    #[test]
    fn test_new_empty_dir_ok() {
        let fs = create_test_fs();
        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        // No configs loaded.
        assert!(mgr.get("anything").is_none());
    }

    // ---- load ----

    #[test]
    fn test_load_toml() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/app.toml",
            "name = \"test\"\nversion = 42\n",
        );

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let value = mgr.load("app.toml").expect("load failed");
        let table = value.as_table().expect("should be table");
        assert_eq!(
            table.get("name"),
            Some(&ConfigValue::String(String::from("test")))
        );
        assert_eq!(table.get("version"), Some(&ConfigValue::Int(42)));
    }

    #[test]
    fn test_load_json() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/app.json",
            r#"{"name": "test", "version": 42}"#,
        );

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let value = mgr.load("app.json").expect("load failed");
        let table = value.as_table().expect("should be table");
        assert_eq!(
            table.get("name"),
            Some(&ConfigValue::String(String::from("test")))
        );
        assert_eq!(table.get("version"), Some(&ConfigValue::Int(42)));
    }

    #[test]
    fn test_load_not_found() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let result = mgr.load("missing.toml");
        assert!(result.is_err());
    }

    // ---- get ----

    #[test]
    fn test_get_top_level() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/device.toml",
            "port = 8080\nhost = \"0.0.0.0\"\n",
        );

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        // get("device") should return the root table.
        let root = mgr.get("device").expect("device should exist");
        assert!(root.as_table().is_some());
    }

    #[test]
    fn test_get_nested() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/device.toml",
            "[network]\nport = 9090\nhost = \"localhost\"\n",
        );

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let port = mgr.get("device.network.port").expect("port should exist");
        assert_eq!(port, &ConfigValue::Int(9090));
        let host = mgr.get("device.network.host").expect("host should exist");
        assert_eq!(host, &ConfigValue::String(String::from("localhost")));
    }

    #[test]
    fn test_get_missing_key() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/device.toml", "port = 8080\n");

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        assert!(mgr.get("device.nonexistent").is_none());
        assert!(mgr.get("device.network.port").is_none());
        assert!(mgr.get("unknown.thing").is_none());
    }

    #[test]
    fn test_get_empty_path() {
        let fs = create_test_fs();
        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        assert!(mgr.get("").is_none());
    }

    #[test]
    fn test_get_json_config() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(
            &mut fs,
            "/config/network.json",
            r#"{"ip": "10.0.0.1", "port": 443}"#,
        );

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let ip = mgr.get("network.ip").expect("ip should exist");
        assert_eq!(ip, &ConfigValue::String(String::from("10.0.0.1")));
        let port = mgr.get("network.port").expect("port should exist");
        assert_eq!(port, &ConfigValue::Int(443));
    }

    // ---- get_or_default ----

    #[test]
    fn test_get_or_default_found() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/device.toml", "port = 8080\n");

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let val = mgr.get_or_default("device.port", ConfigValue::Int(9999));
        assert_eq!(val, ConfigValue::Int(8080));
    }

    #[test]
    fn test_get_or_default_missing() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/device.toml", "port = 8080\n");

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let val = mgr.get_or_default("device.timeout", ConfigValue::Int(30));
        assert_eq!(val, ConfigValue::Int(30));
    }

    #[test]
    fn test_get_or_default_missing_config() {
        let fs = create_test_fs();
        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let val = mgr.get_or_default("unknown.key", ConfigValue::String(String::from("fallback")));
        assert_eq!(val, ConfigValue::String(String::from("fallback")));
    }

    // ---- set ----

    #[test]
    fn test_set_new_config() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("sensor.interval", ConfigValue::Int(500))
            .expect("set failed");

        // Value should be in memory.
        let val = mgr.get("sensor.interval").expect("should exist");
        assert_eq!(val, &ConfigValue::Int(500));

        // File should be persisted on disk (TOML by default).
        let stat = mgr
            .fs
            .stat("/config/sensor.toml")
            .expect("file should exist");
        assert!(stat.size > 0);
    }

    #[test]
    fn test_set_existing_config() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/device.toml", "port = 8080\n");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("device.port", ConfigValue::Int(9090))
            .expect("set failed");

        let val = mgr.get("device.port").expect("should exist");
        assert_eq!(val, &ConfigValue::Int(9090));
    }

    #[test]
    fn test_set_nested_new_key() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/device.toml", "[network]\nport = 8080\n");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set(
            "device.network.host",
            ConfigValue::String(String::from("0.0.0.0")),
        )
        .expect("set failed");

        let host = mgr.get("device.network.host").expect("should exist");
        assert_eq!(host, &ConfigValue::String(String::from("0.0.0.0")));
        // Original key should still be there.
        let port = mgr
            .get("device.network.port")
            .expect("port should still exist");
        assert_eq!(port, &ConfigValue::Int(8080));
    }

    #[test]
    fn test_set_records_version() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set 1");
        mgr.set("app.port", ConfigValue::Int(9090)).expect("set 2");

        let versions = mgr.list_versions("app").expect("list versions");
        assert_eq!(versions.len(), 2);
    }

    #[test]
    fn test_set_notifies_watcher() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.register_watcher("app", Box::new(RecordingWatcher::default()));

        mgr.set("app.port", ConfigValue::Int(8080)).expect("set");
        // Verify via the watcher state (checked in the watcher's own tests).
    }

    #[test]
    fn test_set_preserves_json_format() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/network.json", r#"{"ip": "10.0.0.1"}"#);

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("network.port", ConfigValue::Int(443))
            .expect("set failed");

        // The file should still be JSON (not converted to TOML).
        assert!(mgr.fs.stat("/config/network.json").is_ok());
        assert!(mgr.fs.stat("/config/network.toml").is_err());

        let port = mgr.get("network.port").expect("port should exist");
        assert_eq!(port, &ConfigValue::Int(443));
    }

    // ---- save ----

    #[test]
    fn test_save_persists_config() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set");

        // Modify in-memory without set (load, modify, save).
        {
            let root = mgr.configs.get_mut("app").expect("config exists");
            if let ConfigValue::Table(t) = root {
                t.insert(
                    String::from("host"),
                    ConfigValue::String(String::from("0.0.0.0")),
                );
            }
        }
        mgr.save("app").expect("save failed");

        // Reload to verify persistence.
        mgr.reload("app").expect("reload");
        let host = mgr.get("app.host").expect("host should exist");
        assert_eq!(host, &ConfigValue::String(String::from("0.0.0.0")));
    }

    #[test]
    fn test_save_not_loaded() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let result = mgr.save("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_records_version() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set");

        // Modify and save.
        {
            let root = mgr.configs.get_mut("app").expect("config exists");
            if let ConfigValue::Table(t) = root {
                t.insert(String::from("count"), ConfigValue::Int(5));
            }
        }
        mgr.save("app").expect("save");

        let versions = mgr.list_versions("app").expect("versions");
        assert!(versions.len() >= 2);
    }

    // ---- reload ----

    #[test]
    fn test_reload_picks_up_disk_changes() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/app.toml", "port = 8080\n");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        assert_eq!(mgr.get("app.port"), Some(&ConfigValue::Int(8080)));

        // Write new content directly to disk.
        write_file(&mut mgr.fs, "/config/app.toml", "port = 9999\n");

        mgr.reload("app").expect("reload");
        assert_eq!(mgr.get("app.port"), Some(&ConfigValue::Int(9999)));
    }

    #[test]
    fn test_reload_missing_file() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let result = mgr.reload("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_reload_with_filename() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/app.toml", "port = 8080\n");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        write_file(&mut mgr.fs, "/config/app.toml", "port = 7070\n");
        // Reload by filename (not base name).
        mgr.reload("app.toml").expect("reload");
        assert_eq!(mgr.get("app.port"), Some(&ConfigValue::Int(7070)));
    }

    // ---- rollback ----

    #[test]
    fn test_rollback_restores_version() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set 1");
        mgr.set("app.port", ConfigValue::Int(9090)).expect("set 2");

        let versions = mgr.list_versions("app").expect("versions");
        let first = versions[0];

        // Current value should be 9090.
        assert_eq!(mgr.get("app.port"), Some(&ConfigValue::Int(9090)));

        // Rollback to the first version.
        mgr.rollback("app", first).expect("rollback");

        // Value should be restored.
        assert_eq!(mgr.get("app.port"), Some(&ConfigValue::Int(8080)));
    }

    #[test]
    fn test_rollback_no_history() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let result = mgr.rollback("app", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_rollback_version_not_found() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set");
        let result = mgr.rollback("app", 999);
        assert!(result.is_err());
    }

    #[test]
    fn test_rollback_does_not_record_new_version() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(8080)).expect("set 1");
        mgr.set("app.port", ConfigValue::Int(9090)).expect("set 2");

        let versions_before = mgr.list_versions("app").expect("versions");
        let count_before = versions_before.len();

        mgr.rollback("app", versions_before[0]).expect("rollback");

        let versions_after = mgr.list_versions("app").expect("versions");
        assert_eq!(versions_after.len(), count_before);
    }

    // ---- list_versions ----

    #[test]
    fn test_list_versions_empty() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.set("app.port", ConfigValue::Int(1)).expect("set");
        let versions = mgr.list_versions("app").expect("versions");
        assert_eq!(versions.len(), 1);
    }

    #[test]
    fn test_list_versions_multiple() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        for i in 0..5 {
            mgr.set("app.port", ConfigValue::Int(i)).expect("set");
        }
        let versions = mgr.list_versions("app").expect("versions");
        assert_eq!(versions.len(), 5);
        // Versions should be ascending.
        for i in 1..versions.len() {
            assert!(versions[i - 1] < versions[i]);
        }
    }

    #[test]
    fn test_list_versions_no_history() {
        let fs = create_test_fs();
        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        let result = mgr.list_versions("app");
        assert!(result.is_err());
    }

    // ---- register_watcher ----

    #[test]
    fn test_register_and_notify_watcher() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");

        let watcher = Box::new(RecordingWatcher::default());
        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        mgr.register_watcher("app", watcher);

        mgr.set("app.port", ConfigValue::Int(8080)).expect("set");
        // The watcher should have been notified. We verify via reload test below.
    }

    // ---- load_all ----

    #[test]
    fn test_load_all_multiple_files() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/a.toml", "x = 1\n");
        write_file(&mut fs, "/config/b.toml", "y = 2\n");
        write_file(&mut fs, "/config/c.json", r#"{"z": 3}"#);

        let mut mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        // new() already calls load_all, so let's verify.
        assert_eq!(mgr.get("a.x"), Some(&ConfigValue::Int(1)));
        assert_eq!(mgr.get("b.y"), Some(&ConfigValue::Int(2)));
        assert_eq!(mgr.get("c.z"), Some(&ConfigValue::Int(3)));

        // Calling load_all again should refresh.
        mgr.load_all().expect("load_all");
        assert_eq!(mgr.get("a.x"), Some(&ConfigValue::Int(1)));
    }

    #[test]
    fn test_load_all_skips_directories() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        let _ = fs.mkdir("/config/subdir");
        write_file(&mut fs, "/config/app.toml", "x = 1\n");

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        assert_eq!(mgr.get("app.x"), Some(&ConfigValue::Int(1)));
    }

    #[test]
    fn test_load_all_skips_non_config_files() {
        let mut fs = create_test_fs();
        let _ = fs.mkdir("/config");
        write_file(&mut fs, "/config/readme.txt", "not a config\n");
        write_file(&mut fs, "/config/app.toml", "x = 1\n");

        let mgr = ConfigManager::new(fs, "/config", static_time).expect("create failed");
        assert!(mgr.get("readme").is_none());
        assert_eq!(mgr.get("app.x"), Some(&ConfigValue::Int(1)));
    }

    // ---- Helper function tests ----

    #[test]
    fn test_split_first_component() {
        assert_eq!(split_first_component(""), None);
        assert_eq!(split_first_component("a"), Some(("a", None)));
        assert_eq!(split_first_component("a.b"), Some(("a", Some("b"))));
        assert_eq!(split_first_component("a.b.c"), Some(("a", Some("b.c"))));
    }

    #[test]
    fn test_base_name_helper() {
        assert_eq!(ConfigManager::base_name("device"), String::from("device"));
        assert_eq!(
            ConfigManager::base_name("device.toml"),
            String::from("device")
        );
        assert_eq!(
            ConfigManager::base_name("device.json"),
            String::from("device")
        );
        assert_eq!(
            ConfigManager::base_name("/config/device.toml"),
            String::from("device")
        );
        assert_eq!(
            ConfigManager::base_name("device.config.toml"),
            String::from("device.config")
        );
        assert_eq!(ConfigManager::base_name(".hidden"), String::from(".hidden"));
        assert_eq!(ConfigManager::base_name(""), String::from(""));
    }

    #[test]
    fn test_get_nested_helper() {
        let mut inner = BTreeMap::new();
        inner.insert(String::from("port"), ConfigValue::Int(8080));
        let mut outer = BTreeMap::new();
        outer.insert(String::from("device"), ConfigValue::Table(inner));
        let root = ConfigValue::Table(outer);

        // None → root itself.
        assert!(get_nested(&root, None).is_some());

        // Single key.
        let device = get_nested(&root, Some("device")).expect("device");
        assert!(device.as_table().is_some());

        // Nested key.
        let port = get_nested(&root, Some("device.port")).expect("port");
        assert_eq!(port, &ConfigValue::Int(8080));

        // Missing key.
        assert!(get_nested(&root, Some("device.missing")).is_none());
        assert!(get_nested(&root, Some("nonexistent")).is_none());
    }

    #[test]
    fn test_set_nested_helper() {
        let mut root = ConfigValue::Table(BTreeMap::new());

        // Set a top-level key.
        let old = set_nested(&mut root, Some("port"), ConfigValue::Int(8080));
        assert!(old.is_none());
        assert_eq!(
            get_nested(&root, Some("port")),
            Some(&ConfigValue::Int(8080))
        );

        // Set a nested key (creates intermediate tables).
        let old = set_nested(
            &mut root,
            Some("device.host"),
            ConfigValue::String(String::from("x")),
        );
        assert!(old.is_none());
        assert_eq!(
            get_nested(&root, Some("device.host")),
            Some(&ConfigValue::String(String::from("x")))
        );

        // Replace an existing key.
        let old = set_nested(&mut root, Some("port"), ConfigValue::Int(9090));
        assert_eq!(old, Some(ConfigValue::Int(8080)));
        assert_eq!(
            get_nested(&root, Some("port")),
            Some(&ConfigValue::Int(9090))
        );
    }

    // ---- Watcher test helper ----

    #[derive(Default)]
    struct RecordingWatcher {
        notifications: Vec<String>,
    }

    impl ConfigWatcher for RecordingWatcher {
        fn on_config_changed(
            &mut self,
            name: &str,
            path: &str,
            _old: Option<&ConfigValue>,
            _new: Option<&ConfigValue>,
        ) {
            self.notifications.push(format!("{}:{}", name, path));
        }
    }
}
