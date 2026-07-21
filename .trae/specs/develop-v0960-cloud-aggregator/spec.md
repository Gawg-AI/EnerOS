# v0.96.0 Cloud Coordinator 基础 — 数据汇聚 Spec

## Why

v0.95.0 完成云端策略下发，蓝图 phase2 v0.96.0（P2-D 收尾）要求实现 Cloud Coordinator **数据汇聚**能力：收集域内多个 Edge Box 状态（复用 v0.93.0 EdgeBoxState）→ 校验 Schema → 通过 DataSink 存储 → 支撑全局分析与审计，为 v0.112.0 云端孪生主节点提供数据基础。

## What Changes

- **复用既有 crate `eneros-cloud-coordinator`**（`crates/agents/cloud-coordinator/`，D1），新增 1 个源文件：
  - `src/aggregator.rs` — `DomainData` / `EventRecord` / `EventType` / `Severity` / `DataSource` trait / `DataSink` trait / `DataAggregator`（collect + store + 3 计数器）/ `AggError` + `MockDataSource` / `MockDataSink` 故障注入
  - `src/lib.rs` — 追加 `pub mod aggregator;` + 重导出（含 EdgeBoxState 复用声明）
- `Cargo.toml`（既有 crate 更新）：description 追加 v0.96.0；无新依赖（eneros-coordinator 已存在，EdgeBoxState 已导出）
- 新增 `configs/cloud_aggregator.toml`（数据源/存储/汇聚间隔/脱敏 + 中文注释 6 点）
- 新增 `docs/agents/cloud-aggregation-design.md`（12 章节 + 2 Mermaid + D1~D12 偏差表）
- 根目录 4 文件版本同步 0.95.0 → 0.96.0（Cargo.toml / Makefile / ci.yml / gate.rs 注释）
- 内嵌单元测试 40 个（T1~T40），含 collect+store 全链路集成、多 source 汇聚、NaN 风暴防御、Mock 故障注入
- **无 BREAKING**：既有全部 crate 零改动；v0.95.0 既有 3 模块（strategy/channel/publisher）零改动

## Impact

- Affected specs：无既有 spec 受影响（同 crate 追加模块）；关联 develop-v0950-cloud-strategy（前序策略下发）
- Affected code：新增 `crates/agents/cloud-coordinator/src/aggregator.rs`、configs/、docs/agents/、根 4 文件
- 依赖：无新第三方依赖（EdgeBoxState 复用 eneros-coordinator）
- 下游解锁：v0.112.0 云端孪生主节点数据基础

## 偏差声明（D1~D12，Karpathy Think Before Coding：显式取舍）

| 偏差 | 蓝图原文 | 本版本处理 |
|------|---------|-----------|
| **D1** | crate 路径 `crates/cloud_coordinator/src/{aggregator,storage,schema}.rs` | 复用既有 `crates/agents/cloud-coordinator/`（v0.95.0 已建），追加 `src/aggregator.rs`（§2.3.1 硬规则；P2-D 同一 crate 连续追加惯例） |
| **D2** | `domain_id: String` / metrics `HashMap<String, f32>` | 全部 `u64` / `BTreeMap<u64, f32>`（无堆字符串 + 确定性，v0.95.0 D2 惯例） |
| **D3** | `pub async fn collect/store/run`；`Duration::from_secs` / `interval_timer` / `ticker.tick().await` | sync 方法（no_std 无 async runtime，v0.95.0 D3 惯例）；`now_ms: u64` 参数注入；`run` 不实现（无 ticker，集成阶段由调用方循环驱动） |
| **D4** | `states: Vec<EdgeBoxState>` / `metrics: HashMap<String, f32>` | `Vec<EdgeBoxState>` 保留；`BTreeMap<u64, f32>` 替代 HashMap（no_std alloc 无 HashMap，u64 键确定性） |
| **D5** | `DataSink { Tsdb(String), File(String), S3(String) }`（枚举变体含外部存储后端） | `DataSink` 为 sync trait（`store(&mut self, data, now_ms)`）+ `MockDataSink`（测试覆盖存储失败/成功/记录）；不引入 TSDB/S3/File 外部依赖（§5.5 防重复造轮子，本版本仅定义接口与 Mock；真实存储后续由外部 crate 实现并注入 `Box<dyn DataSink>`） |
| **D6** | 蓝图 `EdgeBoxState` 内嵌定义 | 复用 `eneros_coordinator::EdgeBoxState`（v0.93.0，已导出；§5.5 防重复造轮子） |
| **D7** | `warn!("数据源失败: {}", e)` | 不用 warn! 宏（no_std 无 log crate）；数据源失败通过 `AggError::SourceFailed(u64)` + `timeout_count` 计数器暴露可观测 |
| **D8** | 测试 `tests/data_agg.rs` | crate 内嵌 `#[cfg(test)]` 40 测试（v0.87.0~v0.95.0 项目惯例） |
| **D9** | metrics 含 f32 值 | NaN 防御：metric 值非有限 → 存入前 sanitize 为 0.0；数据量计数独立用 u64（`collect_count`/`timeout_count`/`store_count`）不依赖 metric |
| **D10** | 蓝图 §5.4 "数据量大需压缩" | 本版本不做压缩（no_std 无标准压缩库）；域内数据聚合为轻量级汇总（仅保留 EdgeBoxState 快照，不保留原始点表），压缩列入后续版本评估 |
| **D11** | 蓝图 §8.5 "时区与时间戳统一" | 统一使用 u64 ms UTC epoch 时间戳（`now_ms` 外部注入），不涉及时区转换 |
| **D12** | 蓝图 §7.3 "数据脱敏" | 本版本定义脱敏标记字段 `is_sensitive: bool`（DomainData 字段），脱敏执行逻辑后续 v0.101.0 断网处理实现 |

## ADDED Requirements

### Requirement: 数据汇聚数据结构与接口

系统 SHALL 提供（全部 no_std + alloc 兼容）：`DomainData { domain_id: u64, timestamp: u64, states: Vec<EdgeBoxState>, events: Vec<EventRecord>, metrics: BTreeMap<u64, f32>, is_sensitive: bool }`（Debug/Clone/PartialEq）、`EventRecord { event_id: u64, event_type: EventType, timestamp: u64, severity: Severity }`（Debug/Clone/Copy/PartialEq）、`EventType { StateChange, Alarm, Command, Metric }`（Debug/Clone/Copy/PartialEq/Eq/Default）、`Severity { Info, Warning, Error, Critical }`（Debug/Clone/Copy/PartialEq/Eq/Default）；`EdgeBoxState` 复用 `eneros_coordinator::EdgeBoxState`（不重复定义）。

系统 SHALL 提供 `AggError { SourceFailed(u64), StoreFailed, EmptySources }`（Debug/Clone/Copy/PartialEq/Eq）。

系统 SHALL 提供 sync trait `DataSource { fn fetch(&mut self, now_ms: u64) -> Result<EdgeBoxState, AggError>; }` 与 sync trait `DataSink { fn store(&mut self, data: &DomainData, now_ms: u64) -> Result<(), AggError>; }`（no_std 单线程，无 Send+Sync 约束）。

#### Scenario: 数据结构构造与派生
- **WHEN** 构造 DomainData（domain_id=1, timestamp=1000, states 空, events 空, metrics 空, is_sensitive=false）
- **THEN** 字段值与输入一致；Clone 后相等；Debug 输出包含类型名

### Requirement: 数据汇聚器 DataAggregator

系统 SHALL 提供 `DataAggregator { sources: Vec<Box<dyn DataSource>>, sink: Box<dyn DataSink>, collect_count: u64, timeout_count: u64, store_count: u64 }`（字段全 pub）：
- `new(sink: Box<dyn DataSink>) -> Self`（sources 空、3 计数器全零）
- `add_source(&mut self, source: Box<dyn DataSource>)` 追加数据源
- `collect(&mut self, now_ms: u64) -> Result<DomainData, AggError>`：遍历 sources 逐个 `fetch(now_ms)`，Ok 加入 states，Err → `timeout_count += 1` 继续（不中断）；sources 空 → `Err(EmptySources)`；全部 fetch 后组装 DomainData（timestamp=now_ms，metrics 含 `states.len()` 计数 + 总容量汇总），`collect_count += 1`
- `store(&mut self, data: &DomainData, now_ms: u64) -> Result<(), AggError>`：委托 sink.store，Ok → `store_count += 1`，Err → `Err(StoreFailed)`

#### Scenario: collect 全通过与部分失败
- **WHEN** 2 个 MockDataSource（均成功），collect(1000)
- **THEN** `Ok(DomainData)`，`collect_count == 1`，`timeout_count == 0`，states.len() == 2
- **WHEN** 2 个 MockDataSource（第 1 个成功、第 2 个失败）
- **THEN** `Ok(DomainData)`，`timeout_count == 1`，states.len() == 1（失败不中断，继续收集）
- **WHEN** sources 为空
- **THEN** `Err(EmptySources)`

#### Scenario: store 成功与失败
- **WHEN** MockDataSink 成功，store 任意 DomainData
- **THEN** `Ok(())`，`store_count == 1`
- **WHEN** MockDataSink 恒失败
- **THEN** `Err(StoreFailed)`，`store_count == 0`

### Requirement: Mock 故障注入

系统 SHALL 提供 `MockDataSource { pub state: EdgeBoxState, pub fail_times: u32 }`：fetch 时 fail_times > 0 则减一并 `Err(SourceFailed(id))`，否则返回 `Ok(state)`（id 从 1 顺序分配）。

系统 SHALL 提供 `MockDataSink { pub stored: Vec<DomainData>, pub fail_times: u32 }`：store 时 fail_times > 0 则减一并 `Err(StoreFailed)`，否则 push data 克隆到 stored 并 `Ok(())`。

#### Scenario: Mock 故障注入
- **WHEN** MockDataSource fail_times=1，连续 2 次 fetch
- **THEN** 第 1 次 Err，第 2 次 Ok
- **WHEN** MockDataSink fail_times=2，连续 3 次 store
- **THEN** 前 2 次 Err，第 3 次 Ok 且 data 入 stored

### Requirement: NaN 防御与脱敏标记

系统 SHALL 在 collect 组装 DomainData.metrics 时，对 BTreeMap 中值执行 sanitize：非有限 f32 → 0.0（D9）。系统 SHALL 提供 `is_sensitive` 字段（bool），默认 false（D12）。

#### Scenario: NaN 风暴与脱敏
- **WHEN** metrics 中含 NaN/Inf 值，存入 DomainData
- **THEN** sanitize 后对应键值为 0.0
- **WHEN** DomainData is_sensitive = true
- **THEN** 字段保留 true，脱敏执行逻辑留待后续版本

### Requirement: 云端汇聚配置

系统 SHALL 提供 `configs/cloud_aggregator.toml`：`[cloud_aggregator]` 段（`collect_interval_ms = 5000` / `max_sources = 32` / `metric_sanitize = true`），中文注释含：汇聚 <5s（§7.2，集成阶段验收）/ 数据源超时跳过不中断（§4.4，D7）/ 存储失败重试（§4.4，D5 后续注入）/ 数据脱敏标记（§7.3，D12）/ NaN 防御（D9）/ 新数据源可扩展（§9，DataSource trait）。

## MODIFIED Requirements

### Requirement: workspace 集成与版本

根 `Cargo.toml`：`[workspace.package] version = "0.96.0"`。`Makefile` / `ci.yml` 版本注释同步。`ci/src/gate.rs` clippy/test 注释串尾追加 v0.96.0 类型清单（DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink）。**既有 crate 全部零改动**。

既有 `crates/agents/cloud-coordinator/Cargo.toml` description 追加 v0.96.0（无新依赖）。既有 `src/lib.rs` 追加 `pub mod aggregator;` + 重导出（v0.95.0 strategy/channel/publisher 零改动）。

## REMOVED Requirements

无。
