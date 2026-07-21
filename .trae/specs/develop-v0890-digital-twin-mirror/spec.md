# v0.89.0 Digital Twin Agent — 数据镜像 Spec

## Why

v0.75.0~v0.88.0 已完成 DDS 总线、路由器、电网/市场 Agent 与多目标调度，但各源状态分散在总线上，无统一实时镜像。本版本新建 `eneros-twin-agent` crate，实现 Digital Twin Agent：旁路订阅 `/power/state/*` 主题，将设备/电网/市场状态实时镜像到 `TwinModel`，周期发布快照到 `/power/twin/update`，为 v0.90.0 短期预测、v0.91.0 What-if、v0.112.0 云端孪生主节点提供实时状态输入（蓝图 §1 出口关联）。

## What Changes

- **ADDED**：新 crate `crates/agents/twin-agent/`（包名 `eneros-twin-agent`，D1）
  - `src/lib.rs` — crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + pub mod + 重导出 + 文档
  - `src/model.rs` — `TwinModel` / `DeviceTwin` / `TwinSnapshot` / `MarketMirror` 数据结构
  - `src/mirror.rs` — `TwinMirror`（new / on_tick / apply_update / snapshot / publish）+ `TwinError` + payload DTO
  - 内嵌 `#[cfg(test)] mod tests`（T1~T40，D12）
- **ADDED**：`configs/twin_mirror.toml` — 订阅主题清单 + 发布周期配置模板
- **ADDED**：`docs/agents/digital-twin-design.md` — 设计文档（12 章 + 2 Mermaid 图 + D1~D12 偏差表，D12）
- **MODIFIED**：根 `Cargo.toml` — members 追加 `"crates/agents/twin-agent"`；workspace 版本 `0.88.0` → `0.89.0`
- **MODIFIED**：`Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` — 版本同步

无 **BREAKING** 变更：全部为新增文件；既有 crate 源码零改动（surgical）。

## Impact

- **Affected specs**：v0.75.0 agent-bus-dds（复用 `DdsNode` trait / `DdsSample` / `MockDdsNode`）；v0.82.0 grid-agent（复用 `GridState`）；v0.73.0 device-agent（复用 `DeviceState`）；为 v0.90.0 预测 / v0.91.0 What-if / v0.112.0 云端孪生提供上游
- **Affected code**：
  - `crates/agents/twin-agent/`（新建 crate）
  - 根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs`（版本 + member 同步）
- **新增依赖**：仅 workspace 内 path 依赖（`eneros-agent-bus-dds` / `eneros-grid-agent` / `eneros-device-agent`）+ 既有第三方 `serde` / `serde_json`（与 energy-market-agent 同版本同 features，SBOM 不变）
- **回归面**：grid-agent 130+1、device-agent 24、energy-market-agent 185、agent-bus-dds 63 必须全部通过

## ADDED Requirements

### Requirement: TwinModel 数据结构（model.rs）

系统 SHALL 提供孪生模型数据类型：

- `MarketMirror`：`{ timestamp: u64, current_price: f32 }`，派生 `Debug, Clone, Copy, PartialEq, Default`（D10 极简本地类型）
- `DeviceTwin`：`{ device_id: u64, state: DeviceState }`，派生 `Debug, Clone, PartialEq, Default`；`DeviceState` 复用 `eneros_device_agent::DeviceState`（含 soc/voltage/current/temperature/power/online/last_update_ms，D6）
- `TwinModel`：`{ devices: BTreeMap<u64, DeviceTwin>, grid: GridState, market: Option<MarketMirror>, last_update: u64 }`，派生 `Debug, Clone, Default`；`GridState` 复用 `eneros_grid_agent::GridState`（D7）；`BTreeMap` 保证设备迭代有序（D2）
- `TwinSnapshot`：`{ timestamp: u64, model: TwinModel }`，派生 `Debug, Clone`
- `TwinModel::device_count(&self) -> usize`
- `TwinSnapshot::summary_json(&self) -> String` — 摘要 JSON（D9：timestamp / last_update / device_count / grid_timestamp / market_timestamp / applied_count / published_count 由调用方补充计数器则放入 mirror 侧；本方法仅含 model 自身字段：timestamp/last_update/device_count/grid_timestamp/market_timestamp）

#### Scenario: 默认值
- **WHEN** `TwinModel::default()`
- **THEN** devices 空 / grid 全零 / market None / last_update == 0；`device_count() == 0`

#### Scenario: BTreeMap 有序
- **WHEN** 乱序插入设备 30/10/20
- **THEN** `devices.keys()` 顺序为 [10, 20, 30]

### Requirement: TwinMirror 旁路镜像（mirror.rs）

系统 SHALL 提供旁路镜像器：

```rust
pub enum TwinError { Dds(DdsError) }  // D8：单变体

pub struct TwinMirror {
    pub model: TwinModel,
    pub node: Box<dyn DdsNode>,
    pub participant: ParticipantId,
    pub readers: Vec<(String, ReaderId)>,   // 显式主题列表（D11）
    pub writer: WriterId,
    pub publish_interval_ms: u64,
    pub last_publish_ms: u64,
    pub applied_count: u64,                  // 成功应用消息计数（§9 可观测）
    pub skipped_count: u64,                  // 过期/无效消息计数
    pub published_count: u64,                // 快照发布计数
}

impl TwinMirror {
    pub fn new(node: Box<dyn DdsNode>, topics: &[&str], publish_interval_ms: u64) -> Result<Self, TwinError>;
    pub fn on_tick(&mut self, now_ms: u64) -> Result<bool, TwinError>;   // D3 sync；返回本次是否发布
    pub fn apply_update(&mut self, topic: &str, payload: &[u8], now_ms: u64) -> bool; // 公开便于测试
    pub fn snapshot(&self) -> TwinSnapshot;
    pub fn publish(&mut self) -> Result<(), TwinError>;
}
```

- `new`：`create_participant` → 逐 topic `create_reader`（QoS 默认）→ `create_writer("/power/twin/update")`；任一失败 → `Err(TwinError::Dds(..))`
- `on_tick(now_ms)`（D3，替代蓝图 async run/ticker）：
  1. 逐 reader `take(100)`（蓝图 §4.5 批量）；每样本 `apply_update(topic, &payload, now_ms)`
  2. `now_ms - last_publish_ms >= publish_interval_ms` → `publish()` 并更新 `last_publish_ms`，返回 `Ok(true)`；否则 `Ok(false)`
- `apply_update` 返回是否实际应用（false = 过期/无效被跳过，`skipped_count += 1`；应用成功 `applied_count += 1` 且 `model.last_update = now_ms`）：
  - `topic == "/power/state/grid"` → 解析 `GridPayload`（全 Option 字段 DTO）→ **逐字段合并**（字段缺失保留旧值，蓝图 §4.4）；过期判定（D11）：payload.timestamp 存在且 `< model.grid.timestamp` → 跳过
  - `topic` 以 `/power/state/battery/` 开头 → 后缀解析 `u64`（失败 → 跳过）；解析 `DevicePayload`（soc/voltage/current/temperature/power/online 全 Option）→ `devices.entry(id).or_default()` 逐字段合并，`state.last_update_ms = now_ms`；过期判定：payload 含 `last_update_ms` 且 `< entry.state.last_update_ms` → 跳过
  - `topic == "/power/market/price"` → 解析 `MarketPayload { timestamp, current_price }`（两字段必填，缺失/无效 → 跳过保留旧值）；`timestamp < model.market.timestamp` → 过期跳过
  - 其余 topic → 跳过（不更新）
  - payload 非合法 JSON → 跳过（保留旧值，蓝图 §4.4）
- `snapshot()` → `TwinSnapshot { timestamp: model.last_update, model: model.clone() }`
- `publish()` → 快照摘要 JSON（D9：`snapshot.summary_json()` 注入 applied/skipped/published 计数器后的 DTO）→ `serde_json::to_vec` → `node.write(writer, &bytes)` → `published_count += 1`；write 失败 → `Err(TwinError::Dds(..))`

#### Scenario: 电网状态逐字段合并
- **WHEN** grid 已有 frequency=50.0，收到 payload `{"active_power": 120.0}`
- **THEN** frequency 保留 50.0，active_power 更新 120.0，apply 返回 true，applied_count 增加

#### Scenario: 过期消息不更新
- **WHEN** grid.timestamp==1000，收到 `{"frequency": 49.9, "timestamp": 500}`
- **THEN** 跳过（返回 false），grid 不变，skipped_count 增加

#### Scenario: 电池设备 upsert
- **WHEN** 收到 topic `/power/state/battery/7` payload `{"soc": 0.8, "power": 1.5}`，now_ms=2000
- **THEN** devices[7] 创建，soc==0.8 / power==1.5 / last_update_ms==2000

#### Scenario: on_tick 端到端
- **WHEN** MockDdsNode 上另有 writer 向 `/power/state/grid` 写入 JSON 样本，publish_interval_ms=1000
- **THEN** `on_tick(1000)` 后 grid 已更新；`on_tick(1000)` 返回 true（首 tick 即发布，`0 - 0 >= 1000` 不成立则首次不发布——实现定义：`last_publish_ms` 初始 0，now_ms=1000 时 `1000-0>=1000` 成立 → 发布）

### Requirement: no_std Compliance

- `lib.rs` crate 级 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复加
- 仅 `alloc::*` / `core::*`；禁止 `std` / `async` / `panic!` / `unsafe` / `todo!` / `unimplemented!` / 主代码 `unwrap()` / `HashMap` / `Instant::now()`
- 时间注入：`now_ms: u64` 参数（D5）

## MODIFIED Requirements

### Requirement: workspace 集成

- 根 `Cargo.toml`：`members` 追加 `"crates/agents/twin-agent"`；`[workspace.package] version = "0.89.0"`
- `Makefile`：`# Version: v0.89.0` + `VERSION := 0.89.0`
- `.github/workflows/ci.yml`：`# Version: v0.89.0`
- `ci/src/gate.rs`：clippy 段 + test 段注释追加 `+ v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError`
- 既有 crate 源码零改动

## REMOVED Requirements

无。本版本仅追加。

## 偏差声明（D1~D12，Karpathy "Think Before Coding"）

| 偏差 | 蓝图原文 | 本版本处理 | 理由 |
|------|---------|-----------|------|
| **D1** | `crates/agents/twin_agent/` | `crates/agents/twin-agent/`（包名 `eneros-twin-agent`） | 规则 §2.3.1 目录名与包名去前缀一致；与 device-agent/energy-market-agent 连字符惯例统一（grid_agent 下划线为历史例外） |
| **D2** | `devices: HashMap<DeviceId, DeviceTwin>` | `BTreeMap<u64, DeviceTwin>` | no_std 无 std HashMap；BTreeMap 迭代有序，快照输出确定（沿用 v0.87.0 D3/v0.88.0 D2） |
| **D3** | `async fn run()` + `interval` ticker 无限循环 | sync `on_tick(&mut self, now_ms) -> Result<bool, TwinError>` | no_std 无 async runtime/定时器；沿用 v0.82.0 grid_agent `on_tick` 模式；周期判定由调用方驱动，可测试 |
| **D4** | `subscriber: DdsReader` 独立 subscriber.rs | 不独立建 subscriber 模块；`Box<dyn DdsNode>` + `ReaderId`/`WriterId` 句柄 | v0.75.0 D7 已将 DdsReader/DdsWriter 合并为 DdsNode 统一 trait；蓝图 DdsReader 类型不存在 |
| **D5** | `now_ms()` 全局调用 | `now_ms: u64` 参数注入 | no_std 无 `Instant::now()`（沿用 v0.87.0 D11/v0.88.0 D4） |
| **D6** | `DeviceTwin { device_id: String, state: DeviceState, soc: Option<f32>, power: f32, timestamp: u64 }` | `DeviceTwin { device_id: u64, state: DeviceState }`（复用 eneros-device-agent `DeviceState`，已含 soc/power/last_update_ms） | device_id String→u64（no_std Copy，沿用 v0.87.0 D4）；soc/power/timestamp 与内嵌 state 重复，删除避免双写不一致 |
| **D7** | `grid: GridState`（类型未定义） | 复用 `eneros_grid_agent::GridState`（12 字段 + DataQuality） | 既有权威类型，重复定义导致两份电网状态语义漂移 |
| **D8** | `TwinError`（未定义变体） | 单变体 `Dds(DdsError)`；消息过期/字段缺失/无效 JSON 均为"跳过保留旧值"非错误 | 蓝图 §4.4 两条规则均为降级非硬错误；DDS 操作失败是唯一硬错误 |
| **D9** | `publish()` 发布快照到 `/power/twin/update`（格式未定义） | 发布快照**摘要 JSON**（timestamp/last_update/device_count/grid_timestamp/market_timestamp/计数器） | GridState/DeviceState 无 serde 派生，全量序列化需改 v0.82.0/v0.73.0 既有 crate（违反 surgical）；摘要满足 v0.112.0 云端观测心跳语义 |
| **D10** | `market: Option<MarketData>` | `Option<MarketMirror { timestamp, current_price }>` 本地极简类型 | energy-market-agent `MarketData` 含 96 段预测 Vec 且传递依赖 LLM/Solver crate；镜像只需当前价观测（Simplicity First） |
| **D11** | 订阅 `/power/state/*` + "消息过期 → 不更新" | 订阅主题为显式列表；过期判定 = payload 自带时间戳 `<` 现有值 → 跳过 | Mock 广播为 topic 精确匹配（`{id}` 通配仅 v0.76.0 注册表层）；过期判定须确定性可测试 |
| **D12** | `docs/phase2/digital_twin.md` + `tests/twin_mirror.rs` | `docs/agents/digital-twin-design.md` + 文件内嵌 `#[cfg(test)]` | 规则 §2.3.3 禁止 docs/phase2 平面化；内嵌测试沿用 v0.82.0~v0.88.0 模式 |

## 测试计划（T1~T40，新 crate 独立编号）

- `model.rs`：T1~T6（MarketMirror/DeviceTwin/TwinModel/TwinSnapshot 默认值、派生、device_count、BTreeMap 有序、summary_json 关键字段）
- `mirror.rs`：T7~T34 + T39~T40
  - T7~T12：grid 合并（全字段 / 部分字段保留旧值 / 无效 JSON 跳过 / 过期 timestamp 跳过 / 同 timestamp 接受 / grid_timestamp 更新）
  - T13~T19：battery（id 解析 / 新设备 or_default / 部分合并保留旧值 / 无效 id 跳过 / 过期 last_update_ms 跳过 / online 合并 / applied/skipped 计数）
  - T20~T23：market（正常设置 / 缺字段保留旧 / 无效 JSON / 过期 timestamp 跳过）
  - T24~T26：未知 topic 跳过 / last_update==now_ms / applied+skipped 计数一致
  - T27~T30：snapshot（字段一致 / timestamp==last_update / clone 后修改原 model 不影响快照 / summary_json 可解析）
  - T31~T36：on_tick 端到端（MockDdsNode 广播接收 / take 消费不重复 / 周期到发布 true / 周期未到 false / published_count 递增 / 多 reader 各自接收）
  - T37~T38：new 失败（shutdown 节点 → Dds 错误透传 / 空 topics 列表合法）
  - T39~T40：publish 摘要 JSON 可解析回（timestamp/device_count 正确；第二次 publish 后 published_count==2）
- crate 总测试数：**40 tests**
