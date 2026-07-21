# v0.25.0 — 时序数据存储引擎（TSDB）Spec

## Why

Phase 1 后续版本 v0.52.0（四遥数据模型）、v0.53.0（SOE 事件存储）需要高效的时序数据存储后端。储能场景产生大量时序数据（SOC、功率、温度），列式存储 + 压缩可将存储成本降低 10 倍，TTL 自动清理防止存储溢满。v0.25.0 提供统一的 TSDB 引擎，为所有时序数据消费者提供写入/查询/过期清理能力。

**范围决策**：用户选择**完整版**（6 模块 + Snappy 压缩 + Delta-of-delta + 完整聚合），按蓝图 v0.25.0 详细设计实现。虽蓝图 §42.4 标注为"中度过度设计"，但用户评估后决定完整实现，保留列式 + 压缩 + 聚合的完整能力，避免 v0.52.0 时重构。

## What Changes

### v0.25.0 — 时序数据存储引擎（完整版，6 模块）

- **新增 crate** `crates/drivers/tsdb/`（eneros-tsdb，v0.25.0）
  - **路径修正**：蓝图原路径 `tsdb/src/` 不符合 §2.3.1 crate 分组规则，按 §2.3.2 归属 drivers 子系统（TSDB 属存储相关）
- 实现 6 个模块（按蓝图详细设计）：
  - `src/schema.rs` — 时序数据 schema 定义（TimeSeriesPoint/TsdbConfig/ColumnarChunk/ChunkHeader/Query/Aggregation）
  - `src/writer.rs` — 时序数据写入器（TsdbWriterImpl + TsdbWriter trait）
  - `src/reader.rs` — 时序数据查询器（TsdbReaderImpl + TsdbReader trait）
  - `src/compression.rs` — Snappy 压缩（Compressor trait + SnappyCompressor）
  - `src/retention.rs` — TTL 过期清理
  - `src/index.rs` — 时间索引（TimeIndex + IndexEntry）
- 实现 `TimeSeriesDB` 主入口（open/write/write_batch/query/query_range/compact/cleanup_expired/close）
- 实现 `TsdbError` 错误类型（8 变体：DiskFull/IoError/DecompressFailed/IndexCorrupted/InvalidQuery/DeviceNotFound/MetricNotFound/ChunkCorrupted）
- 实现 Delta-of-delta 时间戳编码（等间隔时间戳压缩率 50:1）
- 实现简单聚合（avg/max/min/sum/count）
- 文档：`docs/drivers/tsdb-design.md`
- 配置：`configs/tsdb.toml`

### Snappy 压缩 no_std 实现策略

**关键约束**：项目规则 §4.3 要求全项目 no_std，但 `snap` crate（v1.1.1）依赖 `std::io::Read/Write`，**不兼容 no_std**。

**实施策略**（按优先级）：
1. **优先**：尝试使用 `snap` crate 的 `raw::Encoder`/`raw::Decoder`（非 frame format，可能不依赖 std::io）。若 raw API 在 no_std 下可用，直接集成。
2. **备选 A**：使用 `compcol` crate（no_std + snappy feature，纯 Rust，2026-05 发布）。风险：成熟度未知。
3. **备选 B**：自研简化版 Snappy（纯 Rust no_std，实现 LZ77 哈希匹配 + 原始块格式）。符合"能源行业特有 + 无开源替代"自研范围。
4. **备选 C**：使用 `lz4_flex` crate（纯 Rust no_std，成熟）作为替代压缩算法，放弃 Snappy 格式兼容性。

**spec 立场**：实施阶段由子代理验证 snap raw API 的 no_std 兼容性，按优先级选择可行方案。无论选择哪个方案，`Compressor` trait 保持不变，确保可替换。

### TSDB 与 FileSystem 交互方式

**关键约束**：eneros-fs 的 `Lfs::read_file_at`/`write_file_at` 是 `pub(crate)`，跨 crate 不可见。`File::read/write` 签名为 `&mut crate::Lfs`（具体类型，非 trait object）。

**设计决策**：TSDB 持有 `Lfs` 实例（具体类型），通过 `FileSystem` trait 的 `open`/`create`/`remove`/`mkdir`/`stat` 方法操作文件，通过 `File` 句柄的 `read`/`write`（传入 `&mut Lfs`）读写数据。这是 eneros-fs 的预期用法，静态分发，无 trait object 开销。

## Impact

- **Affected specs**: v0.24.0（FileSystem，TSDB 依赖）、v0.52.0（四遥数据，消费 TSDB）、v0.53.0（SOE 事件，消费 TSDB）
- **Affected code**: 新增 `crates/drivers/tsdb/`，修改根 `Cargo.toml`（workspace members + version）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- **New dependency**: Snappy 压缩 crate（实施阶段确定，见上节策略）
- **License**: 取决于所选压缩 crate（snap/compcol = BSD-3-Clause，lz4_flex = BSD-3-Clause，自研 = MIT），deny.toml 已允许 BSD-3-Clause

## ADDED Requirements

### Requirement: TimeSeriesDB 主入口

系统 SHALL 提供统一的 `TimeSeriesDB` 结构，支持 open/write/write_batch/query/query_range/compact/cleanup_expired/close 操作，底层基于 v0.24.0 FileSystem 读写列式 chunk 文件。

#### Scenario: 写入并查询单个数据点
- **WHEN** 调用 `tsdb.write(&point)` 写入一个 TimeSeriesPoint，然后 `tsdb.query_range(device, metric, start, end)`
- **THEN** 返回包含该数据点的 Vec，timestamp/value/quality 完整

#### Scenario: 批量写入
- **WHEN** 调用 `tsdb.write_batch(&points)` 写入 1000 个数据点
- **THEN** 返回 Ok(())，所有数据点可被 query_range 查询到

#### Scenario: 范围查询过滤
- **GIVEN** 写入 t=100,200,300,400,500 五个数据点
- **WHEN** 调用 `tsdb.query_range(device, metric, 200, 400)`
- **THEN** 返回 t=200,300,400 三个数据点（含边界）

### Requirement: 列式存储与 Delta-of-delta 编码

系统 SHALL 将时序数据按列存储（timestamps/values/qualities 分列），时间戳列使用 Delta-of-delta 编码，数值列与品质列使用 Snappy 压缩。

#### Scenario: 等间隔时间戳高压缩
- **GIVEN** 1000 个等间隔时间戳（t=0,1000,2000,...,999000）
- **WHEN** 调用 `serialize_timestamps(&ts)` 编码
- **THEN** 编码后字节数 < 原始 8000 字节的 5%（Delta-of-delta 对等间隔序列压缩率 50:1+）

#### Scenario: 非等间隔时间戳仍正确编解码
- **GIVEN** 1000 个非等间隔时间戳
- **WHEN** 编码后解码
- **THEN** 解码结果与原始时间戳完全一致

### Requirement: Snappy 压缩

系统 SHALL 提供 `Compressor` trait（compress/decompress/ratio），实现 `SnappyCompressor`，对数值列和品质列进行压缩/解压。

#### Scenario: 压缩-解压往返
- **GIVEN** 任意字节数据 data
- **WHEN** `compressed = compressor.compress(&data)` 后 `decompressed = compressor.decompress(&compressed)`
- **THEN** decompressed == data

#### Scenario: 压缩比统计
- **WHEN** 压缩一批数据后调用 `compressor.ratio()`
- **THEN** 返回压缩比（原始大小/压缩后大小），≥ 1.0

### Requirement: 时间索引

系统 SHALL 提供 `TimeIndex`，基于 `alloc::collections::BTreeMap`，支持按时间范围快速定位 chunk 文件。

#### Scenario: 索引查找
- **GIVEN** 索引包含 entries: (t=1000, file="/tsdb/d1/m1/0001"), (t=2000, file="/tsdb/d1/m1/0002")
- **WHEN** 调用 `index.find_range(1500, 2500)`
- **THEN** 返回 ["/tsdb/d1/m1/0002"]（覆盖 1500-2500 范围的 chunk）

### Requirement: TTL 过期清理

系统 SHALL 提供 TTL 过期清理，按 `TsdbConfig.retention_ms` 自动删除超过保留期的 chunk 文件。

#### Scenario: 过期数据清理
- **GIVEN** retention_ms = 86400000（24小时），存在 t=now-25h 的 chunk 文件
- **WHEN** 调用 `tsdb.cleanup_expired()`
- **THEN** 返回删除的 chunk 数 ≥ 1，该 chunk 文件不再可查询

### Requirement: 聚合查询

系统 SHALL 支持简单聚合查询（avg/max/min/sum/count），按设备+指标+时间范围聚合。

#### Scenario: 平均值聚合
- **GIVEN** 写入 t=100,v=10; t=200,v=20; t=300,v=30
- **WHEN** 调用 `tsdb.query(&Query{ aggregation: Some(Aggregation::Avg), time_range:(0,500), ..})`
- **THEN** 返回聚合结果 avg=20.0

### Requirement: TsdbError 错误类型

系统 SHALL 提供 `TsdbError` 枚举（8 变体：DiskFull/IoError/DecompressFailed/IndexCorrupted/InvalidQuery/DeviceNotFound/MetricNotFound/ChunkCorrupted），实现 Debug + PartialEq + From<FsError>。

#### Scenario: FsError 自动转换
- **WHEN** FileSystem 操作返回 `FsError::DiskFull`
- **THEN** 通过 `?` 自动转换为 `TsdbError::DiskFull`

## MODIFIED Requirements

### Requirement: Workspace 版本号

根 `Cargo.toml` workspace.package.version 从 `0.24.1` 更新为 `0.25.0`。

### Requirement: CI 流水线

`.github/workflows/ci.yml` 添加 eneros-tsdb crate 的交叉编译步骤（aarch64-unknown-none）。

### Requirement: 质量门

`ci/src/gate.rs` 注释更新，添加 eneros-tsdb（v0.25.0 TSDB）说明。

## 设计决策记录

### 为什么完整版（用户决策）

| 维度 | 完整版（选定） | 折中版 | 简化版 |
|------|-------------|--------|--------|
| 模块数 | 6 | 5 | 3 |
| Snappy 压缩 | ✅ | ❌ | ❌ |
| Delta-of-delta | ✅ | ✅ | ❌ |
| 聚合查询 | ✅ 完整 | ✅ 简单 | ❌ |
| v0.52.0 重构风险 | 无 | 低 | 高 |
| 外部依赖 | Snappy crate | 无 | 无 |
| 蓝图 §42.4 评审 | 过度设计 | — | 推荐 |

**用户决策**：选择完整版，保留 Snappy + 完整聚合，避免 v0.52.0 重构。

### 为什么 TSDB 归属 drivers 子系统

蓝图原路径 `tsdb/src/` 不符合 §2.3.1（所有 crate 必须放入 `crates/<subsystem>/`）。按 §2.3.2 归属判定：
- TSDB 属存储相关（基于文件系统读写时序数据）
- 与 eneros-fs（drivers/fs）、eneros-storage（drivers/storage）同属存储栈
- 归入 `crates/drivers/tsdb/`，非 `crates/kernel/`（非内核态）或 `crates/ai/`（非 AI）

### Snappy no_std 兼容性策略

`snap` crate v1.1.1 依赖 `std::io::Read/Write`（frame format），不兼容 no_std。实施策略见 "What Changes → Snappy 压缩 no_std 实现策略"。`Compressor` trait 抽象确保压缩后端可替换，无论选择 snap raw / compcol / 自研 / lz4_flex，上层代码不变。

### TSDB 与 FileSystem 交互

eneros-fs 的 `Lfs::read_file_at`/`write_file_at` 是 `pub(crate)`，TSDB 通过 `FileSystem` trait + `File` 句柄（传入 `&mut Lfs`）读写。TSDB 持有 `Lfs` 具体类型实例，非 `Box<dyn FileSystem>`（因 `File::read/write` 签名要求具体类型）。这是 eneros-fs 的预期用法。
