# EnerOS TSDB 设计文档 (v0.25.0)

> **范围**：基于文件系统的列式时序数据存储引擎，为四遥数据、SOE 事件、
> 设备状态历史提供高效存储与查询能力。
>
> **Crate**：`eneros-tsdb` (`crates/drivers/tsdb/`)
> **版本**：v0.25.0（Phase 1 Layer 6 基础服务）
> **状态**：已实现 — 主机测试通过，aarch64 交叉编译验证通过。

---

## 1. 概述

`eneros-tsdb` 是 EnerOS 的时序数据存储后端。储能场景产生大量时序数据
（SOC、功率、温度等），列式存储 + 压缩可将存储成本降低约 10 倍，TTL 自动
清理防止存储溢满。本 crate 为 v0.52.0 四遥数据模型、v0.53.0 SOE 事件存储
提供统一的写入/查询/过期清理能力。

### 为什么完整版（用户决策）

依据项目规则 §5.5（默认集成清单）与蓝图 §42.4（架构评审），本版本原可选用
简化方案。用户评估后选择**完整版**（6 模块 + 列式存储 + Delta-of-delta +
LZ4 压缩 + 完整聚合），保留完整能力避免 v0.52.0 四遥数据落地时重构。

| 维度 | 完整版（选定） | 折中版 | 简化版 |
|------|-------------|--------|--------|
| 模块数 | 7（含 error/db） | 5 | 3 |
| 压缩 | ✅ LZ4 | ❌ | ❌ |
| Delta-of-delta | ✅ | ✅ | ❌ |
| 聚合查询 | ✅ 完整 | ✅ 简单 | ❌ |
| v0.52.0 重构风险 | 无 | 低 | 高 |
| 蓝图 §42.4 评审 | 过度设计 | — | 推荐 |

### v0.25.0 交付物

| 组件 | 状态 | 说明 |
|------|------|------|
| `schema.rs` | 完成 | TimeSeriesPoint / TsdbConfig / ColumnarChunk / ChunkHeader / Query / Aggregation |
| `error.rs` | 完成 | TsdbError（8 变体）+ From<FsError> |
| `compression.rs` | 完成 | Compressor trait + SnappyCompressor（lz4_flex 后端）+ NoopCompressor |
| `index.rs` | 完成 | TimeIndex（BTreeMap）+ IndexEntry + 序列化 |
| `writer.rs` | 完成 | TsdbWriterImpl + append/flush + Delta-of-delta 编码 |
| `reader.rs` | 完成 | TsdbReaderImpl + read_range/read_last/aggregate |
| `retention.rs` | 完成 | cleanup_expired + should_expire |
| `db.rs` | 完成 | TimeSeriesDB 主入口（open/write/query/compact/close） |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────┐
│  Caller (v0.52.0 四遥 / v0.53.0 SOE)          │
└─────────────┬────────────────────────────────┘
              │  TimeSeriesDB API (write/query/aggregate)
┌─────────────▼────────────────────────────────┐
│  eneros-tsdb::TimeSeriesDB (this crate)       │
│  ┌────────────────────────────────────────┐  │
│  │  Writer (Delta-of-delta + LZ4 压缩)    │  │
│  │  Reader (范围查询 + 聚合)               │  │
│  │  Index (BTreeMap 时间索引)             │  │
│  │  Retention (TTL 过期清理)              │  │
│  └────────────────────────────────────────┘  │
└─────────────┬────────────────────────────────┘
              │  FileSystem trait (open/create/remove/mkdir)
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

`TimeSeriesDB` 是唯一对外入口，持有文件系统（`Lfs` 具体类型）、配置、时间
索引、内存 chunk 缓冲与压缩器实例。写入路径先缓冲到内存 chunk，达到阈值后
触发 flush（列式压缩 + 写文件 + 更新索引）；查询路径扫描索引定位 chunk 文件，
读取并解压后按时间范围过滤。

---

## 3. 数据结构

### 3.1 TimeSeriesPoint

时序数据的最小单元，对应一个采样点：

```rust
pub struct TimeSeriesPoint {
    pub timestamp: u64,           // 毫秒级 Unix 时间戳
    pub device_id: DeviceId,      // 设备 ID（newtype: DeviceId(pub u32)）
    pub metric: MetricId,         // 指标 ID（newtype: MetricId(pub u32)）
    pub value: f64,               // 数值
    pub quality: DataQuality,     // 数据品质（Good/Uncertain/Bad，u8 编码）
}
```

`DeviceId` 与 `MetricId` 为 `#[repr(transparent)]` newtype，可直接以
`DeviceId(1)` 构造，序列化为 4 字节小端。

### 3.2 ColumnarChunk

内存中的列式缓冲块，按列存储以便各列独立压缩：

```rust
pub struct ColumnarChunk {
    pub header: ChunkHeader,      // 元数据头
    pub timestamps: Vec<u64>,     // 时间戳列
    pub values: Vec<f64>,         // 数值列
    pub qualities: Vec<u8>,       // 品质列（DataQuality as u8）
    pub compressed: Vec<u8>,      // flush 后的压缩载荷
}
```

### 3.3 ChunkHeader

固定 32 字节的 chunk 元数据头，序列化为小端字节：

```rust
pub struct ChunkHeader {
    pub device_id: DeviceId,      // 4 字节
    pub metric: MetricId,         // 4 字节
    pub start_time: u64,          // 8 字节 — chunk 首个点时间戳
    pub end_time: u64,            // 8 字节 — chunk 末个点时间戳
    pub point_count: u32,         // 4 字节 — 数据点数
    pub crc32: u32,               // 4 字节 — 载荷 CRC32 校验
}
```

`CHUNK_HEADER_SIZE = 32`。`ChunkHeader::to_bytes()` / `from_bytes()` 提供
序列化/反序列化，长度不足时返回 `None`。

### 3.4 TsdbConfig

```rust
pub struct TsdbConfig {
    pub data_dir: String,            // 数据目录（默认 "/tsdb"）
    pub chunk_duration_ms: u64,      // chunk 时间跨度（默认 3_600_000 = 1 小时）
    pub max_points_per_chunk: u32,   // chunk 最大点数（默认 10_000）
    pub compression: CompressionType,// 压缩算法（默认 Snappy，即 LZ4）
    pub retention_ms: u64,           // 保留期（默认 2_592_000_000 = 30 天）
    pub flush_interval_ms: u64,      // 刷盘间隔（默认 5_000 = 5 秒）
}
```

`CompressionType` 为枚举：`None`（透传）/ `Snappy`（实际由 lz4_flex 实现，
变体名保留以维持配置兼容性）。

---

## 4. 列式存储与压缩

### 4.1 Delta-of-delta 时间戳编码

时间戳列采用 Delta-of-delta 编码。储能场景的采样通常等间隔（如每 100ms），
此时二阶差分恒为 0，压缩率可达 50:1+。

编码算法：
1. 首个时间戳原值写入（8 字节小端）。
2. 计算一阶差分 `delta = ts[i] - ts[i-1]`。
3. 计算二阶差分 `dd = delta - prev_delta`。
4. 对 `dd` 进行有符号 varint 编码：等间隔序列 `dd = 0`，仅占 1 字节。
5. 非等间隔序列仍能正确编解码，仅压缩率下降。

```text
原始 1000 个等间隔时间戳（8000 字节）
  → Delta-of-delta 编码后 < 400 字节（压缩率 > 20:1）
```

### 4.2 LZ4 压缩（替代 Snappy）

**关键约束**：项目规则 §4.3 要求全项目 no_std。`snap` crate v1.1.1 依赖
`std::io::Read/Write`，交叉编译到 `aarch64-unknown-none` 失败
（`E0463: can't find crate for std`）。

**方案**：改用 `lz4_flex`（纯 Rust，`default-features = false` 下 no_std 兼容）。
使用 `compress_prepend_size` / `decompress_size_prepended` 带长度前缀的帧格式，
解压无需外部长度提示。

`Compressor` trait 抽象确保后端可替换：

```rust
pub trait Compressor {
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError>;
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError>;
    fn ratio(&self) -> f32;  // 累计 原始/压缩 比，≥ 1.0 表示有压缩收益
}
```

`SnappyCompressor`（实际为 LZ4）通过 `AtomicU64` 跟踪累计输入/输出字节数，
`ratio()` 返回累计压缩比。`NoopCompressor` 为透传实现，用于
`CompressionType::None`。

### 4.3 压缩比统计

| 列 | 编码 | 典型压缩比 |
|------|------|-----------|
| 时间戳（等间隔） | Delta-of-delta + LZ4 | 50:1+ |
| 时间戳（非等间隔） | Delta-of-delta + LZ4 | 3:1~5:1 |
| 数值（f64） | LZ4 | 2:1~4:1 |
| 品质（u8） | LZ4 | 1.5:1~3:1 |

整体压缩比目标 ≥ 5:1（蓝图 §6.3 性能基准）。

---

## 5. 时间索引

### 5.1 设计

`TimeIndex` 基于 `alloc::collections::BTreeMap<u64, Vec<IndexEntry>>`，键为
chunk 起始时间戳。同一时间戳可能对应多个 chunk（不同 device/metric 对），
故值为 `Vec<IndexEntry>`。

```rust
pub struct IndexEntry {
    pub time: u64,            // chunk 起始时间（毫秒）
    pub file_path: String,    // chunk 文件路径
    pub chunk_id: u32,        // chunk 唯一 ID
    pub point_count: u32,     // 数据点数
}

pub struct TimeIndex {
    entries: BTreeMap<u64, Vec<IndexEntry>>,
}
```

### 5.2 核心操作

- **`find_range(start, end)`**：利用 `BTreeMap::range(start..=end)` 定位覆盖
  `[start, end]` 区间的所有 chunk 条目，返回 `Vec<&IndexEntry>`。
- **`remove_before(time)`**：TTL 清理核心。使用 `split_off(&time)` 将 map 分为
  `< time`（过期）与 `>= time`（保留）两半，返回过期条目供调用方删除文件。
- **`serialize()` / `deserialize()`**：索引持久化到 `{data_dir}/index.bin`。
  格式：`entry_count: u32` 后跟每个条目的 `time:u64 | chunk_id:u32 |
  point_count:u32 | file_path_len:u32 | file_path:[u8]`（全小端）。

### 5.3 为什么用 BTreeMap

- no_std 友好（`alloc::collections::BTreeMap` 无需 `HashMap` 的随机状态）。
- 确定性迭代顺序，便于序列化与调试。
- `range()` / `split_off()` 原语天然适配时间范围查询与 TTL 清理。
- 无需自研 B+ 树（蓝图原设计提及 B+ 树或跳表，BTreeMap 已满足需求）。

---

## 6. TTL 过期清理

### 6.1 配置

`TsdbConfig::retention_ms` 定义数据保留期（默认 30 天）。超过保留期的 chunk
文件由 `cleanup_expired(now)` 删除。

### 6.2 cleanup_expired 流程

```text
1. 计算 cutoff = now - retention_ms
2. index.remove_before(cutoff) → 返回过期 IndexEntry 列表
3. 对每个过期条目：
   a. fs.remove(&entry.file_path) 删除 chunk 文件
   b. 失败（NotFound）视为已删除，继续
4. 返回成功删除的 chunk 数
```

`TimeSeriesDB::cleanup_expired(&mut self, now: u64) -> Result<u64, TsdbError>`
返回删除的文件数。调用方负责传入当前时间（来自 v0.12.0 RTC）。

### 6.3 空目录清理

删除 chunk 文件后，device/metric 子目录可能变空。当前实现不主动删除空目录
（littlefs2 的 GC 会回收），避免增加 IO 开销。

---

## 7. 聚合查询

`TimeSeriesDB::aggregate(&mut self, q: &Query) -> Result<AggResult, TsdbError>`
支持 5 种聚合：

| 聚合 | 计算 | 空集行为 |
|------|------|---------|
| `Avg` | sum / count | 返回 0.0 |
| `Max` | `fold(NEG_INFINITY, max)` | 返回 -∞ |
| `Min` | `fold(INFINITY, min)` | 返回 +∞ |
| `Sum` | `iter().sum()` | 返回 0.0 |
| `Count` | `len() as f64` | 返回 0.0 |

实现先通过 `query_range` 收集匹配点（含磁盘 + 内存缓冲），再对 `value` 列
计算聚合。`AggResult` 包含 `aggregation` / `value` / `count` 三字段。

`query()` 方法若检测到 `q.aggregation.is_some()`，则返回单个合成
`TimeSeriesPoint`（timestamp = 查询区间末尾，value = 聚合结果）。

---

## 8. 性能基准

蓝图 §6.3 目标（待 QEMU/真机基准测试验证）：

| 指标 | 目标 | 说明 |
|------|------|------|
| 写入吞吐 | ≥ 50000 点/s | 内存缓冲 + 批量 flush |
| 单点查询延迟 | < 1ms | BTreeMap 索引 + 单 chunk 读取 |
| 范围查询（1 小时） | < 50ms | 单 chunk 解压 + 过滤 |
| 压缩比 | ≥ 5:1 | Delta-of-delta + LZ4 |

性能瓶颈预期在文件系统 IO（v0.24.0 littlefs2 每次操作 mount/unmount）。
高频写入场景建议调用方批量 `write_batch` 并控制 `flush_interval_ms`。

---

## 9. 文件布局

```text
/tsdb/                              ← data_dir（默认 "/tsdb"）
├── index.bin                       ← 持久化时间索引
├── {device_id}/                    ← 设备目录（如 "1"）
│   ├── {metric_id}/                ← 指标目录（如 "2"）
│   │   ├── 00000000000000001000    ← chunk 文件（20 位零填充 start_time）
│   │   ├── 00000000000036001000
│   │   └── ...
│   └── ...
└── ...
```

chunk 文件名由 `make_chunk_path` 生成：
`format!("{}/{}/{}/{:020}", data_dir, device.0, metric.0, start_time)`

20 位零填充保证字典序与时间序一致，便于 `readdir` 顺序扫描。

---

## 10. chunk 文件格式

每个 chunk 文件由固定大小的头 + 三段压缩载荷组成：

```text
┌──────────────────────────────────────────────────────────┐
│  ChunkHeader (32 字节，固定大小)                          │
│  ├─ device_id:    u32 LE                                 │
│  ├─ metric:       u32 LE                                 │
│  ├─ start_time:   u64 LE                                 │
│  ├─ end_time:     u64 LE                                 │
│  ├─ point_count:  u32 LE                                 │
│  └─ crc32:        u32 LE                                 │
├──────────────────────────────────────────────────────────┤
│  compressed_timestamps   (变长，Delta-of-delta + LZ4)     │
├──────────────────────────────────────────────────────────┤
│  compressed_values       (变长，LZ4)                      │
├──────────────────────────────────────────────────────────┤
│  compressed_qualities    (变长，LZ4)                      │
└──────────────────────────────────────────────────────────┘
```

读取时按顺序：先读 32 字节头，再依次读三段压缩载荷（每段长度由 LZ4
size-prepended 帧自描述）。`crc32` 校验载荷完整性，不匹配返回
`TsdbError::ChunkCorrupted`。

---

## 11. 设计决策记录

### 11.1 为什么完整版（用户决策）

蓝图 §42.4 标注完整 TSDB 为"中度过度设计"，MVP 阶段建议用"追加日志 + 定期
归档"。用户评估后选择完整版，理由：
- v0.52.0 四遥数据模型需要聚合查询，简化版需重构。
- 列式 + 压缩对储能场景的长期数据存储成本影响显著（10 倍差距）。
- 一次性投入避免 Phase 1 后期返工。

### 11.2 为什么 LZ4 替代 Snappy

`snap` crate v1.1.1 不兼容 no_std（依赖 `std::io::Read/Write` 与
`std::convert::TryInto`）。交叉编译到 `aarch64-unknown-none` 失败：
`E0463: can't find crate for std`。

备选方案对比：
| 方案 | no_std | 成熟度 | 选定 |
|------|--------|--------|------|
| snap raw API | ❌ | 高 | ❌ |
| compcol | ✅ | 低（2026-05 发布） | ❌ |
| 自研 Snappy | ✅ | — | ❌（违反 §5.5 不造轮子） |
| **lz4_flex** | ✅ | 高 | ✅ |

`Compressor` trait 抽象确保后端可替换，未来若需要 Snappy 格式兼容仅需新增
一个实现。`CompressionType::Snappy` 变体名保留以维持配置兼容性。

### 11.3 为什么 TSDB 归属 drivers 子系统

蓝图原路径 `tsdb/src/` 不符合 §2.3.1（所有 crate 必须放入
`crates/<subsystem>/`）。按 §2.3.2 归属判定：
- TSDB 属存储相关（基于文件系统读写时序数据）。
- 与 eneros-fs（drivers/fs）、eneros-storage（drivers/storage）同属存储栈。
- 归入 `crates/drivers/tsdb/`，非 `crates/kernel/`（非内核态）或
  `crates/ai/`（非 AI）。

### 11.4 为什么 TSDB 持有 Lfs 具体类型

`eneros-fs::File::read/write` 签名为 `fn read(&mut self, fs: &mut Lfs, ...)`
（具体类型，非 `&mut dyn FileSystem`），因为 littlefs2 的闭包 API 要求具体
文件系统类型。故 `TimeSeriesDB` 持有 `Lfs` 实例而非 `Box<dyn FileSystem>`：
- 静态分发，无虚调用开销。
- 这是 eneros-fs 的预期用法（v0.24.0 设计决策）。
- `Lfs::read_file_at`/`write_file_at` 为 `pub(crate)`，跨 crate 不可见，
  必须通过 `File` 句柄 + `FileSystem` trait 方法操作。

---

## 12. 依赖关系

| 依赖 | 版本 | 用途 |
|------|------|------|
| eneros-fs | v0.24.0 | FileSystem trait + Lfs + File 句柄 |
| eneros-storage | v0.23.0 | BlockDevice trait（被 eneros-fs 适配） |
| eneros-time | v0.12.0 | RTC 时间戳来源（调用方传入） |
| 用户堆 | v0.11.0 | Vec/String/BTreeMap 分配 |
| lz4_flex | 外部 | LZ4 压缩（no_std，BSD-3-Clause） |

依赖链（不可乱序）：
```
v0.11.0(用户堆) → v0.23.0(BlockDevice) → v0.24.0(FileSystem) → v0.25.0(TSDB)
                                        ↑
                            v0.12.0(RTC) ┘
```

---

## 13. 后续版本

| 版本 | 消费方式 | 说明 |
|------|---------|------|
| v0.52.0 | 四遥数据模型 | 遥测/遥信/遥调/遥控数据写入 TSDB |
| v0.53.0 | SOE 事件存储 | 事件序列记录写入 TSDB |
| v0.53.1/v0.53.2 | （刚性子版本） | SOE 增强 |

TSDB 的 `TimeSeriesDB` API 在 v0.52.0/v0.53.0 中保持稳定，无需重构。

---

## 14. no_std 合规性

本 crate 严格遵守 §4.3 no_std 要求：

```rust
#![cfg_attr(not(test), no_std)]
extern crate alloc;
```

- 使用 `alloc::string::String`、`alloc::vec::Vec`、`alloc::collections::BTreeMap`、
  `alloc::boxed::Box`、`alloc::format!`。
- 无 `std::sync::Mutex`（压缩统计用 `core::sync::atomic::AtomicU64`）。
- 无 `std::io` / `std::net` / `std::time`（时间戳为 `u64` 毫秒，由调用方提供）。
- `lz4_flex` 以 `default-features = false` 启用，纯 no_std。

---

## 15. 构建与测试

```bash
# 主机侧单元测试（104 个）
cargo test -p eneros-tsdb

# aarch64 交叉编译验证
cargo build -p eneros-tsdb --target aarch64-unknown-none \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

# Lint
cargo clippy -p eneros-tsdb --all-targets -- -D warnings

# 文档生成
cargo doc -p eneros-tsdb --no-deps
```

---

## 16. 参考

- 蓝图 §42.4（TSDB 架构评审）、§43.1（no_std 合规）、§5.5（默认集成清单）
- spec 文档：`.trae/specs/develop-v0250-tsdb/spec.md`
- eneros-fs 设计：`docs/drivers/lfs-design.md`
- lz4_flex crate: https://crates.io/crates/lz4_flex
- Delta-of-delta 编码：Gorilla 论文（Facebook, VLDB 2015）
