# EnerOS 配置管理系统设计文档 (v0.26.0)

> **范围**：基于文件系统的 TOML/JSON 配置加载与保存、热加载通知、版本管理与
> 回滚、默认值机制，为设备配置、网络配置、Agent 配置提供统一管理。
>
> **Crate**：`eneros-config` (`crates/runtime/config/`)
> **版本**：v0.26.0（Phase 1 Layer 6 基础服务）
> **状态**：已完整实现 — ConfigValue/Schema/Loader/Version/Watcher/Manager 全部
> 模块就绪；TomlLoader/JsonLoader 已接入官方 `toml` v1.0+（no_std）与
> `serde_json`（alloc feature），实现 `Value ↔ ConfigValue` 双向转换。

---

## 1. 概述

`eneros-config` 是 EnerOS 的统一配置管理后端。Edge Box 运行时需要管理大量
配置（设备参数、网络地址、Agent 参数、Solver 参数、TSDB 参数等），这些配置
必须可持久化、可热更新、可回滚，并在掉电后由 littlefs2 恢复。本 crate 为
v0.33.0+ Agent Runtime、v0.27.0 网络协议栈、v0.52.0 四遥数据模型等所有上层
模块提供配置读写服务。

### 业务价值

- **可持久化**：配置写入 littlefs2 文件系统，掉电不丢失。
- **可热更新**：运行中通过 `reload()` 重新读取文件并通知 watcher，无需重启。
- **可回滚**：每次 `set()` 记录版本快照（CRC32 校验），失败配置可回退到上一
  个已知良好版本。
- **统一格式**：TOML 为主、JSON 为辅，覆盖人工编辑与程序生成两种场景。

### Phase 1 Layer 6 定位

依据蓝图 `phase1.md` §v0.26.0，本版本属于 **P1-A 存储与文件系统** 子模块的
最后一个版本，与 v0.23.0（块设备）/ v0.24.0（文件系统）/ v0.25.0（TSDB）
共同构成 Layer 6 基础服务栈。配置管理是 Phase 1 所有上层模块（Agent Runtime、
网络、密码学、协议栈）的通用依赖。

### v0.26.0 交付物

| 组件 | 状态 | 说明 |
|------|------|------|
| `lib.rs` | 完成 | Crate 入口 + 架构文档注释 + 公共 re-export |
| `schema.rs` | 完成 | ConfigType / ConfigValue / ConfigField / ConfigSchema + `validate()` + From impls |
| `error.rs` | 完成 | ConfigError（8 变体）+ From<FsError> + Display |
| `loader.rs` | 完成 | ConfigLoader trait + TomlLoader / JsonLoader（已接入 `toml`/`serde_json`）+ ConfigFormat |
| `version.rs` | 完成 | ConfigVersion（CRC32）+ VersionHistory（max_versions=10） |
| `watcher.rs` | 完成 | ConfigWatcher trait + WatcherRegistry |
| `manager.rs` | 完成 | ConfigManager 主入口（new/load/get/set/reload/rollback/list_versions/register_watcher） |

> **注**：TomlLoader/JsonLoader 已接入官方 `toml` crate（v1.0+ no_std 配置：
> `default-features = false, features = ["serde", "parse", "display"]`）与
> `serde_json`（`default-features = false, features = ["alloc"]`），完整实现
> `toml::Value ↔ ConfigValue` 和 `serde_json::Value ↔ ConfigValue` 双向转换。
> 所有模块均具备完整功能逻辑与单元测试覆盖。

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────┐
│  Caller (Agent Runtime / Drivers / Kernel)    │
└─────────────┬────────────────────────────────┘
              │  ConfigManager API (get/set/load/reload/rollback)
┌─────────────▼────────────────────────────────┐
│  eneros-config::ConfigManager (this crate)    │
│  ┌────────────┐ ┌──────────┐ ┌────────────┐  │
│  │ ConfigValue│ │ Loaders  │ │  Version   │  │
│  │  Schema    │ │ TOML/JSON│ │  History   │  │
│  └────────────┘ └──────────┘ └────────────┘  │
│  ┌────────────┐ ┌──────────────────────┐    │
│  │  Watcher   │ │  ConfigManager       │    │
│  │ Registry   │ │  (main entry point)  │    │
│  └────────────┘ └──────────────────────┘    │
└─────────────┬────────────────────────────────┘
              │  FileSystem trait (open/stat/mkdir) + File::read/write
┌─────────────▼────────────────────────────────┐
│  eneros-fs::Lfs (v0.24.0, littlefs2)          │
│  ┌────────────────────────────────────────┐  │
│  │  BlockDeviceStorage adapter            │  │
│  └────────────────────────────────────────┘  │
└─────────────┬────────────────────────────────┘
              │  read_block / write_block / erase_block
┌─────────────▼────────────────────────────────┐
│  eneros-storage::BlockDevice (v0.23.0)        │
└──────────────────────────────────────────────┘
```

`ConfigManager` 是唯一对外入口，持有文件系统（`Lfs` 具体类型）、配置根目录、
注入的时间源函数指针、内存配置缓存、版本历史表与 watcher 注册表。写入路径
（`set`）更新内存缓存 → 序列化持久化 → 记录版本 → 通知 watcher；读取路径
（`get`）直接命中内存缓存，无需文件 IO。

### 模块划分

| 模块 | 职责 |
|------|------|
| `schema.rs` | 运行时配置值类型与 schema 声明 |
| `loader.rs` | TOML/JSON 格式加载器 trait 与实现 |
| `version.rs` | 版本快照 + CRC32 校验 + 历史环形缓冲 |
| `watcher.rs` | 配置变更回调 trait + 注册表 |
| `manager.rs` | 主入口，组合上述模块 |
| `error.rs` | 统一错误枚举 |

---

## 3. 数据结构

### 3.1 ConfigType

用于 schema 声明的配置值类型枚举：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigType {
    Bool,
    Int,
    Float,
    String,
    Array,
    Table,
}
```

### 3.2 ConfigValue

运行时配置值，支持 6 种基础类型。`Table` 使用 `BTreeMap` 以满足 no_std 约束
并提供确定性迭代顺序：

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<ConfigValue>),
    Table(BTreeMap<String, ConfigValue>),
}
```

`ConfigValue` 提供类型安全的访问器：`as_bool()` / `as_int()` / `as_float()`
/ `as_str()` / `as_array()` / `as_table()`，类型不匹配返回 `None`。
`config_type()` 返回对应的 `ConfigType` 枚举，用于 schema 校验。

### 3.3 ConfigField

单个配置字段的 schema 声明：

```rust
#[derive(Debug, Clone)]
pub struct ConfigField {
    pub path: String,                    // 点分路径，如 "device.port"
    pub config_type: ConfigType,         // 期望类型
    pub required: bool,                  // 是否必填
    pub default: Option<ConfigValue>,    // 缺省值
}
```

### 3.4 ConfigSchema

配置 schema，定义一份配置文件的预期结构。`name` 与配置文件名（去扩展名）
对应，供 `ConfigManager` 加载时校验：

```rust
#[derive(Debug, Clone)]
pub struct ConfigSchema {
    pub name: String,        // 如 "device"（对应 device.toml）
    pub fields: Vec<ConfigField>,
}

impl ConfigSchema {
    pub fn new(name: String) -> Self;
    pub fn add_field(&mut self, field: ConfigField);
}
```

### 3.5 ConfigVersion

单次配置快照，包含序列化后的数据与 CRC32 校验和：

```rust
#[derive(Debug, Clone)]
pub struct ConfigVersion {
    pub version: u64,        // 单调递增版本号
    pub timestamp: u64,      // 注入时间源（epoch seconds）
    pub data: Vec<u8>,       // 序列化的配置数据
    pub crc32: u32,          // data 的 CRC32 校验和
}

impl ConfigVersion {
    pub fn new(version: u64, timestamp: u64, data: Vec<u8>) -> Self;
    pub fn verify(&self) -> bool;   // 重新计算 CRC32 并比对
}
```

`ConfigVersion::new` 在构造时即时计算 CRC32；`verify()` 用于回滚前校验数据
完整性，不匹配则 `rollback` 返回 `ConfigError::ChecksumMismatch`。

### 3.6 VersionHistory

单个配置文件的版本历史，环形缓冲，最多保留 `MAX_VERSIONS = 10` 条：

```rust
pub const MAX_VERSIONS: usize = 10;

#[derive(Debug, Default)]
pub struct VersionHistory {
    versions: Vec<ConfigVersion>,   // 按版本号升序
    next_version: u64,              // 下一个分配的版本号（从 1 起）
}

impl VersionHistory {
    pub fn new() -> Self;
    pub fn record(&mut self, timestamp: u64, data: Vec<u8>) -> u64;  // 返回分配的版本号
    pub fn get(&self, version: u64) -> Option<&ConfigVersion>;
    pub fn current_version(&self) -> Option<u64>;
    pub fn list_versions(&self) -> Vec<u64>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

`record()` 满载时淘汰最早的条目（`remove(0)`），保证历史窗口固定为 10 个版本。

---

## 4. TOML/JSON 加载器

### 4.1 ConfigFormat

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Json,
}

impl ConfigFormat {
    pub fn from_extension(ext: &str) -> Option<Self>;  // "toml" / "json"
    pub fn extension(self) -> &'static str;
}
```

### 4.2 ConfigLoader trait

抽象加载器接口，使 `ConfigManager` 可统一处理 TOML/JSON：

```rust
pub trait ConfigLoader {
    /// 将原始字节解析为 ConfigValue::Table。
    fn parse(&self, data: &[u8]) -> Result<ConfigValue, ConfigError>;

    /// 将 ConfigValue 序列化为原始字节。
    fn serialize(&self, value: &ConfigValue) -> Result<Vec<u8>, ConfigError>;

    /// 从文件系统加载配置文件（默认实现）。
    fn load_from_file(&self, fs: &mut Lfs, path: &str) -> Result<ConfigValue, ConfigError> {
        let mut file = fs.open(path, OpenFlags::READ)?;
        let stat = fs.stat(path)?;
        let mut buf = vec![0u8; stat.size as usize];
        file.read(fs, &mut buf)?;
        // 裁剪 littlefs 可能追加的尾部 NUL 字节。
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(len);
        self.parse(&buf)
    }

    /// 保存配置值到文件系统（默认实现）。
    fn save_to_file(&self, fs: &mut Lfs, path: &str, value: &ConfigValue) -> Result<(), ConfigError> {
        let data = self.serialize(value)?;
        let mut file = fs.open(path, OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE)?;
        file.write(fs, &data)?;
        Ok(())
    }
}
```

`load_from_file` / `save_to_file` 提供默认实现，子类只需实现 `parse` /
`serialize`。文件读取时裁剪 littlefs 尾部 NUL，避免解析器报错。

### 4.3 TomlLoader / JsonLoader

```rust
pub struct TomlLoader;
pub struct JsonLoader;
```

两者均为零字段结构体（无内部状态），通过 `Box<dyn ConfigLoader>` 在
`ConfigManager::loader_for()` 中按格式分发。

### 4.4 Value 双向转换（已实现）

`TomlLoader::parse` 将调用 `toml::from_str` 把 TOML 文本解析为
`toml::Value`，再递归转换为 `ConfigValue`：

| `toml::Value` 变体 | `ConfigValue` 变体 |
|-------------------|-------------------|
| `Bool(b)` | `Bool(b)` |
| `Integer(i)` | `Int(i)` |
| `Float(f)` | `Float(f)` |
| `String(s)` | `String(s)` |
| `Array(a)` | `Array(Vec<ConfigValue>)` |
| `Table(t)` | `Table(BTreeMap<String, ConfigValue>)` |

`serialize` 反向执行：`ConfigValue → toml::Value → toml::to_string`。

`JsonLoader` 通过 `serde_json::Value` 实现相同的双向转换，`Number` 统一映射
到 `Int`（整数）或 `Float`（浮点）。

### 4.5 为什么用官方 toml crate

详见 §11.2。简言之：`toml` v1.0+ 在 `default-features = false` +
`features = ["serde", "parse", "display"]` 下 no_std 兼容（内部
`extern crate alloc`），优于 `boml`（功能有限）与 `tomling`（已废弃）。
`serde` feature 是必需的——`toml::Value`、`toml::Table`、`from_str`、
`to_string` 均通过 `cfg(feature = "serde")` 门控。

---

## 5. 版本管理与回滚

### 5.1 CRC32 校验

每个 `ConfigVersion` 在构造时由 `crc32fast::Hasher` 计算 `data` 的 CRC32。
回滚前 `ConfigVersion::verify()` 重新计算并比对，防止持久化数据损坏导致
配置错乱。

```rust
pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(data);
    hasher.finalize()
}
```

`crc32fast` 在 `default-features = false` 下纯 no_std，无 std 依赖。

### 5.2 VersionHistory 环形缓冲

- `MAX_VERSIONS = 10`：每个配置文件最多保留 10 个历史版本。
- `record(timestamp, data)`：分配单调递增的 `version`，满载时 `remove(0)`
  淘汰最早版本，保证固定窗口。
- `list_versions()`：返回升序版本号列表。

### 5.3 回滚不记录新版本

`ConfigManager::rollback(name, version)` 流程：

```text
1. 从 histories[name] 取出指定 version 的 ConfigVersion
2. entry.verify() → CRC32 校验
   失败 → 返回 ConfigError::ChecksumMismatch
3. loader.parse(&entry.data) → 还原 ConfigValue
4. configs.insert(name, value.clone())  ← 更新内存
5. loader.save_to_file(fs, path, &value)  ← 持久化到磁盘
6. watchers.notify(name, "*", old, Some(new))  ← 通知 watcher
```

**关键决策**：回滚不调用 `history.record()`，即不产生新版本号。理由：回滚
是"恢复到已知良好状态"，不是一次新的编辑；若记录新版本会污染历史链，使
`list_versions()` 无法反映真实的配置演进。回滚后版本历史保持不变，调用方
仍可再次回滚到任意历史版本。

---

## 6. 热加载通知

### 6.1 ConfigWatcher trait

```rust
pub trait ConfigWatcher {
    /// 配置值变更时回调。
    /// path 为点分配置键（如 "device.port"）；
    /// old/new 为旧值与新值（None 表示键被创建或删除）。
    fn on_config_changed(
        &mut self,
        name: &str,
        path: &str,
        old: Option<&ConfigValue>,
        new: Option<&ConfigValue>,
    );
}
```

### 6.2 WatcherRegistry

```rust
pub struct WatcherRegistry {
    watchers: BTreeMap<String, Vec<Box<dyn ConfigWatcher>>>,
}

impl WatcherRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, name: &str, watcher: Box<dyn ConfigWatcher>);
    pub fn notify(&mut self, name: &str, path: &str,
                  old: Option<&ConfigValue>, new: Option<&ConfigValue>);
}
```

每个配置名（如 `"device"`）可注册多个 watcher，`notify` 时按注册顺序遍历
回调。

### 6.3 reload() 流程

no_std RTOS 无 inotify，"热加载"由调用方显式触发：

```text
1. loader.load_from_file(fs, path)  ← 重新读取磁盘文件
2. configs.insert(name, new_value)  ← 更新内存缓存
3. watchers.notify(name, "*", old, Some(new))  ← path="*" 表示整份配置变更
```

`set()` 同样会通知 watcher，但 `path` 为具体变更的键（如 `"device.port"`），
而 `reload()` 的 `path` 为 `"*"`，表示可能整份配置都变了。

典型场景：运维人员通过串口/网络修改 `/config/device.toml` 后，调用
`mgr.reload("device")?` 让运行时感知变更，Agent Runtime 的 watcher 收到
回调后重新初始化相关参数。

---

## 7. ConfigManager API

```rust
pub struct ConfigManager {
    fs: Lfs,                                  // 具体类型（见 §11.1）
    root_dir: String,                         // 如 "/config"
    time_fn: fn() -> u64,                     // 注入时间源
    configs: BTreeMap<String, ConfigValue>,   // 内存缓存
    histories: BTreeMap<String, VersionHistory>,
    watchers: WatcherRegistry,
}

impl ConfigManager {
    /// 创建管理器，自动创建 root_dir 目录（已存在则忽略）。
    pub fn new(mut fs: Lfs, root_dir: &str, time_fn: fn() -> u64)
        -> Result<Self, ConfigError>;

    /// 按扩展名推断格式（.json → Json，其余 → Toml）。
    fn format_for(name: &str) -> ConfigFormat;

    /// 加载配置文件到内存缓存，返回根 Table 引用。
    pub fn load(&mut self, name: &str) -> Result<&ConfigValue, ConfigError>;

    /// 按点分路径读取值。首个分量是配置名（无扩展名默认 .toml）。
    /// 例: "device.port" → 查找 configs["device.toml"]["port"]。
    pub fn get(&self, path: &str) -> Option<&ConfigValue>;

    /// 设置值并持久化 + 记录版本 + 通知 watcher。
    /// 若配置未缓存，先从文件加载（文件不存在则从空 Table 起步）。
    pub fn set(&mut self, path: &str, value: ConfigValue) -> Result<(), ConfigError>;

    /// 从磁盘重新读取文件并通知 watcher（手动热加载）。
    pub fn reload(&mut self, name: &str) -> Result<(), ConfigError>;

    /// 回滚到指定版本（CRC32 校验，不记录新版本）。
    pub fn rollback(&mut self, name: &str, version: u64) -> Result<(), ConfigError>;

    /// 列出某配置的所有历史版本号（升序）。
    pub fn list_versions(&self, name: &str) -> Result<Vec<u64>, ConfigError>;

    /// 注册 watcher。
    pub fn register_watcher(&mut self, name: &str, watcher: Box<dyn ConfigWatcher>);
}
```

### 路径解析规则

- `get("device.port")` → 配置名 `device.toml`，嵌套键 `port`。
- `get("device.network.ip")` → 配置名 `device.toml`，嵌套键 `network.ip`，
  通过 `get_nested` 递归下钻 `Table`。
- 若首个分量已带扩展名（`.toml`/`.json`），直接用作配置名。

### get_or_default

蓝图接口含 `get_or_default(key, default)`，当前实现由调用方通过
`mgr.get(path).cloned().unwrap_or(default)` 等效完成，避免在 trait 层引入
`Clone` 开销。若后续确有需求，可在 `ConfigManager` 上补齐便捷方法。

### save

蓝图接口含独立 `save(name, value)`，当前实现将其合并入 `set()`：`set` 在
更新内存后即调用 `loader.save_to_file` 持久化，避免调用方遗忘保存。若需
"仅更新内存不落盘"的语义，可在后续版本补齐 `set_cached` + `flush` 组合。

---

## 8. 性能基准

蓝图 §v0.26.0 目标（待 QEMU/真机基准测试验证）：

| 指标 | 目标 | 说明 |
|------|------|------|
| 配置加载 | < 10ms | 单文件解析 + 内存缓存（TOML ~2KB） |
| 配置读取 | < 1μs | 纯内存 BTreeMap 查找，无文件 IO |
| 热加载 | < 50ms | 重新读文件 + 解析 + 通知 watcher |
| 版本记录 | < 1ms | 序列化 + CRC32 + push（数据量小） |
| 回滚 | < 20ms | CRC32 校验 + 反序列化 + 持久化 |

性能瓶颈预期在文件系统 IO（v0.24.0 littlefs2 每次操作 mount/unmount）。
读取路径完全命中内存，无 IO 开销，适合高频配置查询场景。

---

## 9. 文件布局

```text
/config/                        ← root_dir（默认 "/config"）
├── device.toml                 ← 设备配置
├── network.toml                ← 网络配置
├── agent.toml                  ← Agent 配置
├── solver.toml                 ← Solver 配置
├── tsdb.toml                   ← TSDB 配置
├── *.json                      ← JSON 格式配置（程序生成）
└── .versions/                  ← 版本历史持久化目录（规划中）
    └── <name>.dat              ← 单配置的版本快照存档
```

- 配置文件名即配置名，扩展名决定格式（`.toml` / `.json`）。
- `set()` 通过 `OpenFlags::WRITE | CREATE | TRUNCATE` 原子覆盖写入。
- `.versions/` 目录为后续版本规划：当前版本历史仅存内存，进程重启后丢失；
  后续可选将 `VersionHistory::serialize()` 落盘到 `.versions/<name>.dat`，
  实现跨重启的版本恢复。当前不实现，避免与 littlefs2 掉电安全语义耦合。

---

## 10. 设计决策记录

### 10.1 为什么持有 Lfs 具体类型

`eneros-fs::File::read/write` 签名为 `fn read(&mut self, fs: &mut Lfs, ...)`
（具体类型，非 `&mut dyn FileSystem`），因为 littlefs2 的闭包 API 要求具体
文件系统类型。故 `ConfigManager` 持有 `Lfs` 实例而非 `Box<dyn FileSystem>`：

- 静态分发，无虚调用开销。
- 与 v0.25.0 TSDB 保持同一模式（`TimeSeriesDB` 同样持有 `Lfs`）。
- 这是 eneros-fs 的预期用法（v0.24.0 设计决策）。

蓝图原接口 `fs: Box<dyn FileSystem>` 在实现时调整为 `fs: Lfs`，理由同上。

### 10.2 为什么用官方 toml crate（no_std）

`toml` crate v1.0+ 在 `default-features = false` 下移除 `std`，内部
`extern crate alloc`，配合 `features = ["parse", "display"]` 即可在 no_std
下解析与序列化。

备选方案对比：

| 方案 | no_std | 成熟度 | 选定 |
|------|--------|--------|------|
| `toml`（官方） | ✅ | 高 | ✅ |
| `boml` | ✅ | 中（功能有限） | ❌ |
| `tomling` | ✅ | 低（已废弃） | ❌ |
| 自研 TOML 解析器 | ✅ | — | ❌（违反 §5.5 不造轮子） |

`Cargo.toml` 配置：
```toml
toml = { version = "1.0", default-features = false, features = ["parse", "display"] }
```

### 10.3 为什么时间源注入

蓝图原设计未明确时间来源。实现采用 `fn() -> u64` 函数指针注入，而非硬编码
`crate::time::now()`：

- 解耦：`eneros-config` 不直接依赖 `eneros-time`，避免循环依赖。
- 可测：单元测试注入 `fn() -> u64 { 42 }` 固定时间，断言确定。
- 灵活：生产环境注入 RTC 时间（v0.12.0），模拟环境注入虚拟时钟。

### 10.4 为什么用 BTreeMap

`ConfigValue::Table`、`configs`、`histories`、`watchers` 全部使用
`alloc::collections::BTreeMap`：

- no_std 友好（无需 `HashMap` 的随机状态与 `std::collections::hash_map`）。
- 确定性迭代顺序，便于序列化、调试与比对。
- 蓝图 v1.1 §43.1 已将原 `HashMap` 修正为 `BTreeMap`。

### 10.5 为什么手动热加载

no_std RTOS 无 inotify/fsnotify 内核事件，文件变更无法被动感知。"热加载"
设计为显式的 `reload(name)` 调用：

- 调用方（如运维串口、网络管理 Agent）修改文件后主动触发。
- 避免后台轮询线程的开销（RTOS 控制大区零 GC、零后台抖动）。
- 与蓝图 §43.6 RTOS 控制大区 ≤ 32MB 内存预算一致。

### 10.6 为什么 rollback 不记录新版本

见 §5.3。回滚是"恢复"而非"编辑"，记录新版本会污染版本链。

---

## 11. 依赖关系

| 依赖 | 版本 | 用途 |
|------|------|------|
| eneros-fs | v0.24.0 | FileSystem trait + Lfs + File 句柄 + OpenFlags |
| eneros-storage | v0.23.0 | BlockDevice trait（被 eneros-fs 适配） |
| eneros-time | v0.12.0 | RTC 时间戳来源（通过 `time_fn` 注入，非直接依赖） |
| 用户堆 | v0.11.0 | Vec/String/BTreeMap/Box 分配 |
| `toml` | v1.0+ | TOML 解析/序列化（no_std：`parse` + `display`） |
| `serde_json` | v1.0+ | JSON 解析/序列化（no_std：`alloc` feature） |
| `crc32fast` | v1.4+ | CRC32 校验（no_std：`default-features = false`） |

依赖链（不可乱序）：
```
v0.11.0(用户堆) → v0.23.0(BlockDevice) → v0.24.0(FileSystem) → v0.26.0(Config)
                                        ↑
                            v0.12.0(RTC) ┘（注入）
```

---

## 12. 后续版本

| 版本 | 消费方式 | 说明 |
|------|---------|------|
| v0.27.0 ~ v0.30.0 | 网络协议栈 | 从 `network.toml` 读取 IP/网关/DNS |
| v0.33.0+ | Agent Runtime | 从 `agent.toml` 读取 max_agents/heartbeat/task_timeout |
| v0.52.0 | 四遥数据模型 | 从 `device.toml` 读取采样间隔、遥测点表 |
| v0.59.0 ~ v0.63.0 | LLM 推理 | 从 `llm.toml` 读取模型路径、量化等级、并发数 |
| v0.64.0 ~ v0.68.0 | Solver | 从 `solver.toml` 读取 backend/timeout/tolerance |

`ConfigManager` API 在上述版本中保持稳定，无需重构。新增配置项仅需扩展
`configs/default.toml` 与对应 schema。

---

## 13. no_std 合规性

本 crate 严格遵守 §4.3 no_std 要求：

```rust
#![cfg_attr(not(test), no_std)]
extern crate alloc;
```

- 使用 `alloc::string::String`、`alloc::vec::Vec`、`alloc::collections::BTreeMap`、
  `alloc::boxed::Box`、`alloc::format!`。
- 无 `std::sync::Mutex`（watcher 注册表为单线程同步访问，无需锁）。
- 无 `std::io` / `std::net` / `std::time`（时间戳为 `u64`，由 `time_fn` 注入）。
- 无 `std::collections::HashMap`（全部 `BTreeMap`）。
- `toml` / `serde_json` / `crc32fast` 均以 `default-features = false` 启用，
  纯 no_std。
- 蓝图 v1.1 §43.1 已清理原 v1.0 的 `use std::collections::HashMap` 违规。

---

## 14. 构建与测试

```bash
# 主机侧单元测试
cargo test -p eneros-config

# aarch64 交叉编译验证
cargo build -p eneros-config --target aarch64-unknown-none \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

# Lint
cargo clippy -p eneros-config --all-targets -- -D warnings

# 格式检查
cargo fmt -p eneros-config -- --check

# 文档生成
cargo doc -p eneros-config --no-deps
```

测试覆盖：
- `ConfigValue` 类型访问器与 `config_type` 映射。
- `VersionHistory::record` 满载淘汰、`list_versions` 升序、`get` 查找。
- `ConfigVersion::verify` CRC32 正确性。
- `ConfigManager::get` 点分路径嵌套查找、`set` 持久化 + 版本记录 +
  watcher 通知（使用内存 mock 文件系统或 `eneros-fs` 测试设施）。
- `rollback` CRC32 校验失败返回 `ChecksumMismatch`，成功不记录新版本。
- `reload` 通知 watcher（path = `"*"`）。

---

## 15. 参考

- 蓝图 `phase1.md` §v0.26.0（配置管理系统版本定义）
- 蓝图 `Blueprint.md` §42.4（架构评审）、§43.1（no_std 合规）、§5.5（默认集成清单）
- 项目规则 `记忆.md` §2.3.2（子系统归属：配置管理归 runtime）
- eneros-fs 设计：`docs/drivers/lfs-design.md`
- eneros-tsdb 设计（同模式参考）：`docs/drivers/tsdb-design.md`
- TOML 1.1 规范：https://toml.io/en/v1.1.0
- `toml` crate 文档：https://docs.rs/toml/latest/toml/
- `serde_json` no_std 用法：https://docs.rs/serde_json/latest/serde_json/
- `crc32fast` crate：https://crates.io/crates/crc32fast
