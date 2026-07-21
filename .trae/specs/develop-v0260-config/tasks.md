# Tasks — v0.26.0 Config Management

- [x] Task 1: 创建 eneros-config crate 骨架
  - [x] SubTask 1.1: 创建 `crates/runtime/config/Cargo.toml`（name=eneros-config, version=0.26.0, 依赖 eneros-fs + toml + serde_json + crc32fast）
  - [x] SubTask 1.2: 创建 `crates/runtime/config/src/lib.rs`（#![cfg_attr(not(test), no_std)] + extern crate alloc + 模块声明）
  - [x] SubTask 1.3: 根 `Cargo.toml` workspace.members 添加 "crates/runtime/config"，workspace.package.version 改为 "0.26.0"
  - [x] 验证: `cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 2: 实现 schema.rs — 配置 schema 定义
  - [x] SubTask 2.1: 定义 ConfigValue 枚举（Bool/Int/Float/String/Array/Table，Table 用 BTreeMap）+ as_bool/as_int/as_float/as_str/as_array/as_table 转换方法
  - [x] SubTask 2.2: 定义 ConfigType 枚举（Bool/Int/Float/String/Array/Table）+ Display
  - [x] SubTask 2.3: 定义 ConfigField { path, config_type, required, default } + builder 方法 (new/optional/with_default)
  - [x] SubTask 2.4: 定义 ConfigSchema { name, fields: Vec<ConfigField> } + validate(&ConfigValue) -> Result<(), ConfigError>
  - [x] 验证: ConfigValue 类型转换测试 + schema 校验测试 (40+ tests)

- [x] Task 3: 实现 error.rs — ConfigError 错误类型
  - [x] SubTask 3.1: 定义 ConfigError 枚举（8 变体：Fs/NotFound/TomlParse/JsonParse/SchemaViolation/ChecksumMismatch/VersionNotFound/Internal）
  - [x] SubTask 3.2: 实现 Debug + Display（core::fmt::Display）
  - [x] SubTask 3.3: 实现 From<FsError> for ConfigError（统一转换为 ConfigError::Fs(e)）
  - [x] 验证: FsError→ConfigError 转换测试 (13 tests)

- [x] Task 4: 实现 loader.rs — TOML/JSON 加载器
  - [x] SubTask 4.1: 定义 ConfigLoader trait { parse/serialize/load_from_file/save_to_file } + 默认实现
  - [x] SubTask 4.2: 实现 TomlLoader（用 toml crate 的 toml::Table::parse / toml::Value::to_string，toml::Value → ConfigValue 转换）
  - [x] SubTask 4.3: 实现 JsonLoader（用 serde_json::from_slice / serde_json::to_string，serde_json::Value → ConfigValue 转换）
  - [x] SubTask 4.4: 实现 toml::Value ↔ ConfigValue 和 serde_json::Value ↔ ConfigValue 双向转换
  - [x] SubTask 4.5: 定义 ConfigFormat 枚举（Toml/Json）+ from_extension / extension
  - [x] 验证: TOML/JSON 加载-保存往返测试 (37+ tests)

- [x] Task 5: 实现 version.rs — 版本管理
  - [x] SubTask 5.1: 定义 ConfigVersion { version: u64, timestamp: u64, data: Vec<u8>, crc32: u32 }
  - [x] SubTask 5.2: 实现 ConfigVersion::new(version, timestamp, data) -> Self（即时计算 CRC32）
  - [x] SubTask 5.3: 实现 ConfigVersion::verify(&self) -> bool（CRC32 校验）
  - [x] SubTask 5.4: 实现 compute_crc32 公共函数 + crc32fast crate 集成
  - [x] SubTask 5.5: 实现 VersionHistory { versions: Vec<ConfigVersion>, next_version } + record/get/current_version/list_versions/len/is_empty + MAX_VERSIONS=10 强制淘汰
  - [x] 验证: 版本创建/校验/历史管理测试 (35+ tests)

- [x] Task 6: 实现 watcher.rs — 热加载监听器
  - [x] SubTask 6.1: 定义 ConfigWatcher trait { on_config_changed(&mut self, name, path, old: Option<&ConfigValue>, new: Option<&ConfigValue>) }
  - [x] SubTask 6.2: 实现 WatcherRegistry { watchers: BTreeMap<String, Vec<Box<dyn ConfigWatcher>>> } + register/notify
  - [x] SubTask 6.3: 实现 notify 方法（遍历匹配 name 的 watcher，调用 on_config_changed）
  - [x] 验证: watcher 注册与通知测试 (20+ tests)

- [x] Task 7: 实现 manager.rs — ConfigManager 主入口
  - [x] SubTask 7.1: 定义 ConfigManager { fs: Lfs, root_dir, time_fn, configs, formats, histories, watchers }
  - [x] SubTask 7.2: 实现 ConfigManager::new(fs, root_dir, time_fn) -> Result<Self, ConfigError>（创建目录 + load_all）
  - [x] SubTask 7.3: 实现 load_all()（readdir + 按扩展名选择 loader + 加载到 configs，以 base name 为 key）
  - [x] SubTask 7.4: 实现 get(&self, path) -> Option<&ConfigValue> + get_or_default
  - [x] SubTask 7.5: 实现 set(&mut self, path, value) -> Result<(), ConfigError>（更新内存 + 持久化 + 记录版本 + 通知 watcher）
  - [x] SubTask 7.6: 实现 save(&mut self, name) -> Result<(), ConfigError>（保存单个配置文件 + 记录版本）
  - [x] SubTask 7.7: 实现 load(&mut self, filename) -> Result<&ConfigValue, ConfigError>（加载单个配置文件）
  - [x] SubTask 7.8: 实现 reload(&mut self, name) -> Result<(), ConfigError>（重新加载 + 通知 watcher，path="*"）
  - [x] SubTask 7.9: 实现 rollback(&mut self, name, version) -> Result<(), ConfigError>（CRC32 校验 + 回滚 + 持久化 + 不记录新版本）
  - [x] SubTask 7.10: 实现 list_versions(&self, name) -> Result<Vec<u64>, ConfigError>
  - [x] SubTask 7.11: 实现 register_watcher(&mut self, name, watcher)（以 base name 注册）
  - [x] 验证: 集成测试 (41+ tests，加载→get→set→reload→rollback 全流程)

- [x] Task 8: lib.rs 导出与文档注释
  - [x] SubTask 8.1: lib.rs 添加模块导出（pub mod schema/error/loader/version/watcher/manager）+ pub use 关键类型
  - [x] SubTask 8.2: lib.rs 添加 crate 文档注释（架构图 + 使用示例 + 设计决策）
  - [x] 验证: `cargo doc -p eneros-config --no-deps` 生成文档无警告（clippy doc_lazy_continuation 已修复）

- [x] Task 9: 文档与配置
  - [x] SubTask 9.1: 创建 `docs/runtime/config-design.md`（设计文档：架构 + 数据结构 + 加载器 + 版本管理 + 热加载 + 性能基准）+ 状态更新为"已完整实现"
  - [x] SubTask 9.2: 创建 `configs/default.toml`（默认配置模板：[device]/[network]/[agent]/[solver]/[tsdb]/[storage]/[logging] 配置）
  - [x] 验证: 文档位于 `docs/runtime/`（§2.3.3 文档分类），非 docs/ 根

- [x] Task 10: 版本标识更新
  - [x] SubTask 10.1: 根 `Cargo.toml` workspace.package.version = "0.26.0"
  - [x] SubTask 10.2: `Makefile` VERSION := 0.26.0
  - [x] SubTask 10.3: `.github/workflows/ci.yml` Version: v0.26.0 + 添加 eneros-config 交叉编译步骤
  - [x] SubTask 10.4: `ci/src/gate.rs` 注释添加 eneros-config（v0.26.0 配置管理）说明
  - [x] 验证: 版本号一致性 + ci.yml 含 eneros-config 构建步骤

- [x] Task 11: 构建与质量验证
  - [x] SubTask 11.1: `cargo fmt -p eneros-config -- --check` 通过
  - [x] SubTask 11.2: `cargo clippy -p eneros-config --all-targets -- -D warnings` 通过
  - [x] SubTask 11.3: `cargo test -p eneros-config` 通过（209 单元测试 + 1 ignored doc-test，覆盖率 ≥ 80%）
  - [x] SubTask 11.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试全部 PASS）
  - [x] SubTask 11.5: `cargo run -p eneros-ci` 通过（**Overall: PASS** — fmt/clippy/audit/test 全绿）
  - [x] SubTask 11.6: aarch64 交叉编译通过（WSL2 Ubuntu-22.04 + aarch64-linux-gnu-gcc 13.3.0，**10.67s 编译成功**）
  - [x] SubTask 11.7: `cargo deny check advisories licenses bans sources` 通过（**advisories ok / licenses ok / bans ok / sources ok**）
  - [x] 验证: 所有检查项 PASS

# Task Dependencies

- Task 2 (schema) 无依赖，可先开始
- Task 3 (error) 无依赖，可与 Task 2 并行
- Task 4 (loader) 依赖 Task 2 (ConfigValue) + Task 3 (ConfigError)
- Task 5 (version) 依赖 Task 2 (ConfigValue) + Task 3 (ConfigError)
- Task 6 (watcher) 依赖 Task 2 (ConfigValue)
- Task 7 (manager) 依赖 Task 2,3,4,5,6
- Task 8 (lib.rs) 依赖 Task 7
- Task 9 (文档) 依赖 Task 7
- Task 10 (版本) 可与 Task 2-7 并行（仅改配置文件）
- Task 11 (验证) 依赖 Task 8,9,10 全部完成

# 并行化建议

- **Wave 1（并行）**: Task 1（骨架）、Task 10（版本标识）
- **Wave 2（并行）**: Task 2（schema）、Task 3（error）
- **Wave 3（并行）**: Task 4（loader，依赖 2,3）、Task 5（version，依赖 2,3）、Task 6（watcher，依赖 2）
- **Wave 4**: Task 7（manager，依赖 2,3,4,5,6）
- **Wave 5（并行）**: Task 8（lib.rs）、Task 9（文档）
- **Wave 6**: Task 11（验证）
