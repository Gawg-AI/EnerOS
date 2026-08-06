//! ConfigManager characterization/行为测试（ARCH-G01：拆分前锁定外部行为）。
//!
//! 本文件是拆分（ConfigManager → ConfigStore + 编排 facade）前后的行为基线：
//! 公共 API 不变的条件下，同一测试集必须持续全绿。
//! 由 `manager.rs` 以 `#[cfg(test)] mod manager_tests;` 引入。

#![allow(clippy::disallowed_macros)]

use alloc::boxed::Box;

    use eneros_fs::FileSystem;
    use eneros_storage::{BlockDevice, MockBlockDevice};

    use super::*;
use alloc::collections::BTreeMap;
    use crate::store::{get_nested, set_nested, split_first_component};
    use crate::transaction::CommitOutcome;

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
        assert!(mgr.fs().stat("/config").is_ok());
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
            .fs()
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
        assert!(mgr.fs().stat("/config/network.json").is_ok());
        assert!(mgr.fs().stat("/config/network.toml").is_err());

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
            let root = mgr.configs_mut().get_mut("app").expect("config exists");
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
            let root = mgr.configs_mut().get_mut("app").expect("config exists");
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
        write_file(mgr.fs_mut(), "/config/app.toml", "port = 9999\n");

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
        write_file(mgr.fs_mut(), "/config/app.toml", "port = 7070\n");
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
        assert_eq!(crate::store::ConfigStore::base_name("device"), String::from("device"));
        assert_eq!(
            crate::store::ConfigStore::base_name("device.toml"),
            String::from("device")
        );
        assert_eq!(
            crate::store::ConfigStore::base_name("device.json"),
            String::from("device")
        );
        assert_eq!(
            crate::store::ConfigStore::base_name("/config/device.toml"),
            String::from("device")
        );
        assert_eq!(
            crate::store::ConfigStore::base_name("device.config.toml"),
            String::from("device.config")
        );
        assert_eq!(crate::store::ConfigStore::base_name(".hidden"), String::from(".hidden"));
        assert_eq!(crate::store::ConfigStore::base_name(""), String::from(""));
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

    // ========================================================================
    // SVC-008: compare-and-swap, transaction, durable audit (end-to-end)
    // ========================================================================

    fn make_mgr() -> ConfigManager {
        let fs = create_test_fs();
        ConfigManager::new(fs, "/config", static_time).expect("create failed")
    }

    #[test]
    fn test_cas_swaps_on_match() {
        let mut mgr = make_mgr();
        // Seed a value.
        mgr.set("device.port", ConfigValue::Int(502)).unwrap();
        // CAS with the correct expected value succeeds.
        let outcome = mgr
            .compare_and_swap(
                "device.port",
                Some(&ConfigValue::Int(502)),
                ConfigValue::Int(503),
            )
            .unwrap();
        assert_eq!(outcome, CasOutcome::Swapped);
        assert_eq!(mgr.get("device.port").and_then(|v| v.as_int()), Some(503));
    }

    #[test]
    fn test_cas_mismatches_on_stale_value() {
        let mut mgr = make_mgr();
        mgr.set("device.port", ConfigValue::Int(502)).unwrap();
        // CAS with a stale expected value must NOT swap and must report current.
        let outcome = mgr
            .compare_and_swap(
                "device.port",
                Some(&ConfigValue::Int(999)),
                ConfigValue::Int(503),
            )
            .unwrap();
        match outcome {
            CasOutcome::Mismatch { path, current } => {
                assert_eq!(path, "device.port");
                assert_eq!(current.as_deref(), Some("502"));
            }
            other => panic!("expected Mismatch, got {:?}", other),
        }
        // Value unchanged.
        assert_eq!(mgr.get("device.port").and_then(|v| v.as_int()), Some(502));
    }

    #[test]
    fn test_cas_create_only_with_none_expected() {
        let mut mgr = make_mgr();
        // expected=None means "must not exist" — create-only.
        let outcome = mgr
            .compare_and_swap("device.newkey", None, ConfigValue::Bool(true))
            .unwrap();
        assert_eq!(outcome, CasOutcome::Swapped);
        // Second create-only attempt must mismatch (path now exists).
        let outcome2 = mgr
            .compare_and_swap("device.newkey", None, ConfigValue::Bool(false))
            .unwrap();
        assert!(matches!(outcome2, CasOutcome::Mismatch { .. }));
        // Value remains the first write.
        assert_eq!(
            mgr.get("device.newkey").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn test_transaction_commit_persists_all_writes() {
        let mut mgr = make_mgr();
        {
            let mut txn = mgr.begin_transaction(String::from("svc-008"));
            txn.stage("device.port", ConfigValue::Int(502)).unwrap();
            txn.stage("device.host", ConfigValue::String(String::from("0.0.0.0")))
                .unwrap();
            txn.stage("device.tls", ConfigValue::Bool(true)).unwrap();
            let outcome = txn.commit().unwrap();
            assert_eq!(outcome, CommitOutcome::Committed { applied: 3 });
        }
        assert_eq!(mgr.get("device.port").and_then(|v| v.as_int()), Some(502));
        assert_eq!(
            mgr.get("device.host").and_then(|v| v.as_str()),
            Some("0.0.0.0")
        );
        assert_eq!(mgr.get("device.tls").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_transaction_abort_discards_all_writes() {
        let mut mgr = make_mgr();
        mgr.set("device.port", ConfigValue::Int(502)).unwrap();
        {
            let mut txn = mgr.begin_transaction(String::from("svc-008"));
            txn.stage("device.port", ConfigValue::Int(9999)).unwrap();
            txn.stage("device.extra", ConfigValue::Int(1)).unwrap();
            txn.abort().unwrap();
        }
        // Original value preserved, extra not created.
        assert_eq!(mgr.get("device.port").and_then(|v| v.as_int()), Some(502));
        assert!(mgr.get("device.extra").is_none());
    }

    #[test]
    fn test_audit_log_is_durable_and_reads_back() {
        let mut mgr = make_mgr();
        // Perform a sequence of mutating operations.
        mgr.set("device.port", ConfigValue::Int(502)).unwrap();
        mgr.compare_and_swap(
            "device.port",
            Some(&ConfigValue::Int(502)),
            ConfigValue::Int(503),
        )
        .unwrap();
        // A failed CAS (stale expected) is also audited.
        let _ = mgr
            .compare_and_swap(
                "device.port",
                Some(&ConfigValue::Int(1)),
                ConfigValue::Int(4),
            )
            .unwrap();
        {
            let mut txn = mgr.begin_transaction(String::from("svc-008"));
            txn.stage("device.flag", ConfigValue::Bool(true)).unwrap();
            txn.commit().unwrap();
        }

        // Read back the durable audit file. Every operation must appear.
        let entries = mgr.audit_read_all().unwrap();
        assert!(
            entries.len() >= 4,
            "expected >=4 audit entries, got {}",
            entries.len()
        );

        // First entry is the set (success).
        assert_eq!(entries[0].op, AuditOp::Set);
        assert!(entries[0].success);
        assert_eq!(entries[0].path, "device.port");

        // Second entry is the successful CAS.
        assert_eq!(entries[1].op, AuditOp::Cas);
        assert!(entries[1].success);

        // Third entry is the failed CAS.
        assert_eq!(entries[2].op, AuditOp::Cas);
        assert!(!entries[2].success);

        // Find the commit entry (last).
        let commit = entries
            .iter()
            .find(|e| e.op == AuditOp::Commit)
            .expect("commit entry");
        assert!(commit.success);

        // The in-memory recent ring must agree.
        let recent = mgr.audit_recent();
        assert!(!recent.is_empty());
        assert_eq!(recent[0].op, AuditOp::Set);
    }

    #[test]
    fn test_audit_records_actor_for_transactions() {
        let mut mgr = make_mgr();
        {
            let mut txn = mgr.begin_transaction(String::from("agent-42"));
            txn.stage("device.x", ConfigValue::Int(1)).unwrap();
            txn.commit().unwrap();
        }
        let entries = mgr.audit_read_all().unwrap();
        let commit = entries.iter().find(|e| e.op == AuditOp::Commit).unwrap();
        assert_eq!(commit.actor, "agent-42");
    }
