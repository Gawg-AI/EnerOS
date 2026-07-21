# v0.26.0 — 配置管理系统 Spec

## Why

Phase 1 所有需要配置的上层模块（Agent 配置、设备参数、网络配置、Solver 参数）依赖统一的配置管理服务。v0.26.0 提供 TOML/JSON 配置加载/保存、热加载通知、版本管理与回滚、默认值机制，使配置可持久化、可热更新、可回滚。

**架构决策**：依据蓝图 v0.26.0 设计（未被 §42.4 标注为过度设计），采用完整 5 模块设计。关键 no_std 适配：
- 官方 `toml` crate v1.0+ 已支持 no_std + alloc（2026-02-11 发布），作为 TOML 解析后端
- `serde_json` 配合 `alloc` feature 作为 JSON 解析后端（次要格式）
- ConfigManager 持有 `Lfs` 具体类型（非 `Box<dyn FileSystem>`），因 `File::read/write` 需 `&mut Lfs`（v0.25.0 TSDB 已验证此模式）
- 时间源通过函数指针注入（`fn() -> u64`），避免硬依赖 `crate::time::now()`

## What Changes

### v0.26.0 — 配置管理系统

- **新增 crate** `crates/runtime/config/`（eneros-config，v0.26.0）
  - **注意**：蓝图原路径 `config/src/` → 修正为 `crates/runtime/config/`（§2.3.2 归属判定：配置管理是用户态运行时服务，非硬件驱动）
- 实现 5 个模块：
  - `src/schema.rs` — 配置 schema 定义（ConfigSchema/ConfigField/ConfigType）
  - `src/loader.rs` — TOML/JSON 加载器（ConfigLoader trait + TomlLoader + JsonLoader）
  - `src/manager.rs` — 配置管理器（ConfigManager：load/save/get/set/reload/rollback/list_versions）
  - `src/watcher.rs` — 热加载监听器（ConfigWatcher trait + 通知机制）
  - `src/version.rs` — 版本管理（ConfigVersion + 历史记录 + CRC32 校验）
- 实现 `ConfigValue` 枚举（Bool/Int/Float/String/Array/Table，Table 用 BTreeMap）
- 实现 `ConfigError` 错误类型（8 变体）
- 文档：`docs/runtime/config-design.md`
- 配置：`configs/default.toml`（默认配置模板）

### 关键设计决策

1. **持有 `Lfs` 具体类型**：与 v0.25.0 TSDB 一致，`File::read/write` 签名要求 `&mut Lfs`，故 ConfigManager 持有 `Lfs` 实例而非 `Box<dyn FileSystem>`。蓝图原设计 `Box<dyn FileSystem>` 无法通过编译（File I/O 需具体类型）。

2. **时间源注入**：蓝图用 `crate::time::now()`，但 config crate 不应硬依赖 time crate。改用 `fn() -> u64` 函数指针注入，由调用方提供时间源（与 v0.19.0 分区调度的 TimeSource 模式一致）。

3. **`toml` crate no_std**：官方 `toml` v1.0+ 支持 `default-features = false` + `alloc` feature 实现 no_std。替代方案 `boml`（无依赖但功能简陋）或 `tomling`（已废弃）。选择官方 crate 保证 TOML 1.1 兼容性与长期维护。

4. **BTreeMap 替代 HashMap**：蓝图 v1.1 已标注 `HashMap → BTreeMap`（no_std 合规），ConfigValue::Table、configs、watchers、versions 均用 `alloc::collections::BTreeMap`。

5. **热加载 = 手动 reload()**：no_std RTOS 无 inotify，"热加载"实为调用 `reload(name)` 重新读取文件 + schema 校验 + 通知 watcher。非自动文件监听。

6. **版本历史持久化**：版本记录保留在内存（max_versions=10），同时持久化到 `/config/.versions/<name>.dat` 文件，重启后可恢复。

## Impact

- **Affected specs**: v0.24.0（FileSystem，提供文件 API）、v0.25.0（TSDB，配置可管理 TSDB 参数）、v0.33.0+（Agent Runtime，Agent 配置加载）
- **Affected code**: 新增 `crates/runtime/config/`，修改根 `Cargo.toml`（workspace members + version）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- **New dependencies**: `toml`（v1.0+，no_std + alloc，MIT/Apache-2.0）、`serde_json`（alloc feature，MIT/Apache-2.0）、`crc32fast`（no_std，MIT/Apache-2.0，用于版本校验和）
- **License**: toml (MIT/Apache-2.0) + serde_json (MIT/Apache-2.0) + crc32fast (MIT/Apache-2.0)，deny.toml 已允许

## ADDED Requirements

### Requirement: ConfigManager 配置管理器

系统 SHALL 提供统一的 `ConfigManager`，支持从文件系统加载 TOML/JSON 配置、get/set 配置值、保存配置、热加载、版本管理与回滚。

#### Scenario: 加载 TOML 配置文件
- **GIVEN** `/config/device.toml` 文件存在，内容为 `[device]\nname = "edge-box-01"\nport = 8080`
- **WHEN** 调用 `ConfigManager::new(fs, "/config", time_fn)` 后 `mgr.get("device")`
- **THEN** 返回 `ConfigValue::Table` 包含 `name` 和 `port` 字段

#### Scenario: 保存配置
- **WHEN** 调用 `mgr.set("device.port", ConfigValue::Int(9090))`
- **THEN** 内存配置更新 + 持久化到 `/config/device.toml` + 记录新版本 + 通知 watcher

#### Scenario: 热加载
- **GIVEN** 外部修改了 `/config/device.toml` 文件内容
- **WHEN** 调用 `mgr.reload("device")`
- **THEN** 重新加载文件 + schema 校验通过则更新内存配置 + 通知所有 watcher

#### Scenario: 版本回滚
- **GIVEN** 配置 `device` 有版本历史 [1, 2, 3]，当前为版本 3
- **WHEN** 调用 `mgr.rollback("device", 1)`
- **THEN** 配置恢复到版本 1 的内容 + 持久化 + 不记录新版本（回滚不产生新版本）

### Requirement: ConfigValue 配置值类型

系统 SHALL 提供运行时类型安全的配置值枚举，支持 Bool/Int/Float/String/Array/Table 类型，Table 用 BTreeMap 保证 no_std 兼容与确定性迭代顺序。

#### Scenario: 类型转换
- **WHEN** 调用 `config_value.as_int()`
- **THEN** 若为 `ConfigValue::Int(n)` 返回 `Some(n)`，否则 `None`

### Requirement: ConfigLoader 配置加载器

系统 SHALL 提供可扩展的 `ConfigLoader` trait，支持 TOML 和 JSON 两种格式，通过 trait 抽象确保未来可扩展新格式。

#### Scenario: TOML 加载
- **WHEN** 调用 `TomlLoader.load_from_file(&mut fs, "/config/device.toml")`
- **THEN** 返回 `ConfigValue::Table` 包含解析后的配置

#### Scenario: 格式自动识别
- **WHEN** ConfigManager 加载配置目录
- **THEN** `.toml` 文件用 TomlLoader，`.json` 文件用 JsonLoader

### Requirement: ConfigWatcher 热加载通知

系统 SHALL 提供 `ConfigWatcher` trait，允许模块注册配置变更回调，在配置热加载或 set 操作后收到通知。

#### Scenario: 注册 watcher 并触发通知
- **WHEN** 注册 watcher 监听 "device" 配置后调用 `mgr.set("device.port", ...)`
- **THEN** watcher 的 `on_config_changed("device.port", old, new)` 被调用

### Requirement: 版本管理与回滚

系统 SHALL 提供配置版本管理，每次 save/set 记录版本（含时间戳、序列化数据、CRC32 校验和），支持回滚到任意历史版本，最多保留 10 个版本。

#### Scenario: 版本记录
- **WHEN** 调用 `mgr.set("device.port", 9090)`
- **THEN** `mgr.list_versions("device")` 返回包含新版本号的列表

#### Scenario: CRC 校验
- **GIVEN** 版本数据被篡改（CRC 不匹配）
- **WHEN** 尝试回滚到该版本
- **THEN** 返回 `ConfigError::ChecksumMismatch`

## MODIFIED Requirements

### Requirement: Workspace 版本号

根 `Cargo.toml` workspace.package.version 从 `0.25.0` 更新为 `0.26.0`。

## 设计决策记录

### 为什么持有 Lfs 具体类型而非 Box<dyn FileSystem>

| 维度 | Lfs 具体类型 | Box<dyn FileSystem> |
|------|-------------|---------------------|
| File I/O | ✅ `file.read(&mut fs, buf)` 可调用 | ❌ File::read 需 `&mut Lfs` 具体类型 |
| 静态分发 | ✅ 无虚调用开销 | ❌ 动态分发 |
| 测试隔离 | ✅ 可通过 trait 方法 mock | ❌ 无法 mock File I/O |
| v0.25.0 先例 | ✅ TSDB 已验证此模式 | — |

### 为什么用官方 toml crate 而非 boml/tomling

| 维度 | toml v1.0+ | boml v2.0 | tomling（已废弃） |
|------|-----------|-----------|------------------|
| no_std | ✅ alloc feature | ✅ | ✅ |
| TOML 版本 | 1.1 | 1.0 子集 | 1.0 |
| 维护状态 | ✅ 官方活跃维护 | 个人项目 | ❌ 已废弃 |
| serde 支持 | ✅ | ❌ | ✅ |
| 许可证 | MIT/Apache-2.0 | MIT/Apache-2.0 | MIT |
| deny.toml | ✅ 已允许 | ✅ | ✅ |

### crate 归属：crates/runtime/config/

蓝图原路径 `config/src/` 违反 §2.3.1（禁止根目录 crate）。按 §2.3.2 归属判定：
- 配置管理是用户态运行时服务（非硬件驱动、非内核态）
- 与 `crates/runtime/runtime/`、`crates/runtime/user/` 同属 runtime 子系统
- 归入 `crates/runtime/config/`
