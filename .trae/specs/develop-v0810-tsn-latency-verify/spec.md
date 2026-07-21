# v0.81.0 — TSN 网络驱动与确定性时延验证 Spec

## Why

EnerOS Phase 2（P2-B 收尾）需要在 v0.79.0 gPTP 时间同步 + v0.80.0 TAS 门控调度之上，建立端到端时延测量能力，验证 Agent 关键流量（TC3 命令、TC4 GOOSE、TC5 SV）获得确定性保证。本版本是 VPP < 30s 响应的网络底座达标判定版本。

本版本交付**纯 Rust 类型与算法骨架**（无真实网络 I/O、无硬件时间戳集成），通过 closure 注入 + Mock 驱动的方式验证 `DelayStats` 统计计算、`LatencyProbe` 多场景测量、`TsnDriver` 抽象接口的正确性。真实 TSN 硬件集成与 p99 < 2ms 性能量化验收延后到具备硬件测试仪表的集成测试阶段。

## What Changes

- **扩展 crate**：`crates/protocols/tsn-time/`（新增 2 个源文件：`driver_glue.rs` / `latency_probe.rs`）
- **新增类型**：
  - `DelayStats`（min/max/mean/p99/p999/jitter/samples，派生 `Debug, Clone, PartialEq, Eq, Default`）
  - `LatencyProbe`（sample_count + results + clock_fn + sleep_fn，闭包注入避免 `Instant::now()` 依赖）
  - `TsnDriver` trait（send/recv 抽象，对应蓝图 `TsnDriver` 接口）
  - `MockTsnDriver`（记录发送队列 + 接收队列，D8：无真实 netlink/socket）
  - `TsnError`（`SendFailed` / `RecvFailed` / `NotInitialized`）
  - `driver_send_closure` 辅助函数（将 `TsnDriver` 包装为 `LatencyProbe` 用的 send 闭包）
- **新增配置**：`configs/latency_probe.toml`（探针配置模板：sample_count / interval_us / burst_count）
- **新增文档**：`docs/protocols/tsn-determinism-report.md`（12 章节 + 2 Mermaid 图 + D1~D16 偏差声明表）
- **版本同步**：根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` / `ci/src/gate.rs` 由 `0.80.0` → `0.81.0`
- **workspace members**：根 `Cargo.toml` 无需新增（`crates/protocols/tsn-time` 已存在）

## Impact

- **Affected specs**：无（纯扩展既有 crate；v0.79.0/v0.80.0 类型签名不变）
- **Affected code**：
  - 新增：`crates/protocols/tsn-time/src/driver_glue.rs`
  - 新增：`crates/protocols/tsn-time/src/latency_probe.rs`
  - 新增：`configs/latency_probe.toml`
  - 新增：`docs/protocols/tsn-determinism-report.md`
  - 修改：`crates/protocols/tsn-time/Cargo.toml`（version 0.80.0 → 0.81.0，description 更新）
  - 修改：`crates/protocols/tsn-time/src/lib.rs`（新增 `pub mod` + `pub use` + 测试 T56~T84 + 偏差表 D1~D16，保留 v0.79.0/v0.80.0 内容）
  - 修改：根 `Cargo.toml`（workspace version）
  - 修改：`Makefile`（version）
  - 修改：`.github/workflows/ci.yml`（version）
  - 修改：`ci/src/gate.rs`（clippy/test 段注释新增 `eneros-tsn-time v0.81.0` 与类型列表）
- **Surgical Changes 原则**：v0.79.0 `clock.rs` / `port.rs` / `bmca.rs` / `gptp.rs` 与 v0.80.0 `tas.rs` / `stream.rs` / `config_loader.rs` 完全不动；仅在 `lib.rs` 末尾追加模块声明、导出、测试、偏差表
- **后续解锁**：v0.82.0+（Agent 使用确定性网络进行联邦协同）

## ADDED Requirements

### Requirement: DelayStats 统计结果数据结构

系统 SHALL 提供 `DelayStats` 结构体（位于 `latency_probe` 模块），包含 7 个字段：`min: Duration` / `max: Duration` / `mean: Duration` / `p99: Duration` / `p999: Duration` / `jitter: Duration` / `samples: u64`，并派生 `Debug, Clone, PartialEq, Eq, Default`。`Default` 实现 SHALL 返回所有 `Duration` 字段为 `Duration::ZERO`、`samples` 为 `0` 的实例。

#### Scenario: Default 实现
- **WHEN** 调用 `DelayStats::default()`
- **THEN** `min == max == mean == p99 == p999 == jitter == Duration::ZERO`，`samples == 0`

#### Scenario: 字段可读可写
- **WHEN** 构造 `DelayStats { min: Duration::from_micros(100), max: Duration::from_micros(500), mean: Duration::from_micros(300), p99: Duration::from_micros(450), p999: Duration::from_micros(480), jitter: Duration::from_micros(400), samples: 100 }`
- **THEN** 所有字段可读访问且值与构造一致

### Requirement: TsnDriver trait 与 MockTsnDriver

系统 SHALL 提供 `TsnDriver` trait（位于 `driver_glue` 模块），抽象 TSN 网络驱动的数据面操作：
- `fn send(&mut self, tc: TrafficClass, payload: &[u8]) -> Result<(), TsnError>` — 按 TrafficClass 发送 payload
- `fn recv(&mut self) -> Result<alloc::vec::Vec<u8>, TsnError>` — 接收下一个数据包

系统 SHALL 提供 `TsnError` 枚举（`SendFailed` / `RecvFailed` / `NotInitialized`），派生 `Debug, Clone, Copy, PartialEq, Eq`。

系统 SHALL 提供 `MockTsnDriver` 实现 `TsnDriver` trait，含字段：`sent: Vec<(TrafficClass, Vec<u8>)>` / `recv_queue: Vec<Vec<u8>>` / `fail_send: bool` / `fail_recv: bool`，并提供 `new()` / `push_recv(data: Vec<u8>)` 方法。

#### Scenario: MockTsnDriver send 记录
- **WHEN** 调用 `mock.send(TrafficClass::CA, &[0x01, 0x02])`
- **THEN** `mock.sent` 长度为 1，包含 `(TrafficClass::CA, vec![0x01, 0x02])`，返回 `Ok(())`

#### Scenario: MockTsnDriver recv 返回队列数据
- **WHEN** 先 `mock.push_recv(vec![0xAA, 0xBB])`，再 `mock.recv()`
- **THEN** 返回 `Ok(vec![0xAA, 0xBB])`，队列长度减 1

#### Scenario: MockTsnDriver send 失败
- **WHEN** `mock.fail_send = true`，调用 `mock.send(TrafficClass::Be, &[])`
- **THEN** 返回 `Err(TsnError::SendFailed)`，`mock.sent` 仍为空

#### Scenario: MockTsnDriver recv 空队列
- **WHEN** 队列为空，调用 `mock.recv()`
- **THEN** 返回 `Err(TsnError::RecvFailed)`

### Requirement: driver_send_closure 适配器

系统 SHALL 提供 `driver_send_closure<'a>(driver: &'a mut dyn TsnDriver, tc: TrafficClass, payload: &'a [u8]) -> impl Fn() -> Result<(), ()> + 'a` 函数，将 `TsnDriver::send` 包装为 `LatencyProbe` 所需的 `Fn() -> Result<(), ()>` 闭包。

#### Scenario: 闭包成功路径
- **WHEN** 构造 `driver_send_closure(&mut mock, TrafficClass::CA, &[0x01])`，调用闭包
- **THEN** 返回 `Ok(())`，`mock.sent` 包含对应记录

#### Scenario: 闭包失败路径
- **WHEN** `mock.fail_send = true`，构造闭包并调用
- **THEN** 返回 `Err(())`，`mock.sent` 仍为空

### Requirement: LatencyProbe 时延探针

系统 SHALL 提供 `LatencyProbe` 结构体（位于 `latency_probe` 模块），含字段：
- `sample_count: u32` — 已采集样本数
- `results: Vec<Duration>` — 采集结果列表
- `clock_fn: fn() -> u64` — 时钟注入（返回纳秒，避免 `Instant::now()` 依赖，D6）
- `sleep_fn: fn(Duration)` — 睡眠注入（避免 `eneros_time::delay()` 依赖，D7）

`LatencyProbe::new(clock_fn: fn() -> u64, sleep_fn: fn(Duration)) -> Self` SHALL 初始化 `sample_count = 0` / `results = Vec::new()`。

#### Scenario: 构造函数初始化
- **WHEN** 调用 `LatencyProbe::new(test_clock, test_sleep)`
- **THEN** `sample_count == 0`、`results.is_empty()` 为 true

### Requirement: measure_round_trip 单次往返测量

`LatencyProbe::measure_round_trip(&mut self, send: impl Fn() -> Result<(), ()>) -> Result<Duration, ()>` SHALL：
1. 调用 `(self.clock_fn)()` 记录起始纳秒 `start_ns`
2. 调用 `send()`，若返回 `Err(())` 则直接向上传递错误（不增加 `sample_count`）
3. 调用 `(self.clock_fn)()` 记录结束纳秒 `end_ns`
4. 返回 `Ok(Duration::from_nanos(end_ns.saturating_sub(start_ns)))`

#### Scenario: 成功测量
- **WHEN** `clock_fn` 第一次返回 `1_000_000`、第二次返回 `1_500_000`（500µs 间隔），`send` 闭包返回 `Ok(())`
- **THEN** 返回 `Ok(Duration::from_nanos(500_000))`，`sample_count` 未变化（由调用者决定何时计入）

#### Scenario: send 失败
- **WHEN** `send` 闭包返回 `Err(())`
- **THEN** 返回 `Err(())`，未调用第二次 `clock_fn`（提前返回）

### Requirement: run_burst 突发测量

`LatencyProbe::run_burst(&mut self, count: u32, interval: Duration, send: impl Fn() -> Result<(), ()>) -> DelayStats` SHALL：
1. 循环 `count` 次：
   - 调用 `measure_round_trip(&send)`，若 `Ok(d)` 则 `self.results.push(d)` 且 `self.sample_count += 1`
   - 调用 `(self.sleep_fn)(interval)`（即使在 send 失败时也调用，确保采样间隔稳定）
2. 返回 `self.compute_stats()`

#### Scenario: 零次采样
- **WHEN** 调用 `run_burst(0, Duration::from_millis(1), || Ok(()))`
- **THEN** 返回 `DelayStats::default()`，`sample_count == 0`，`results.is_empty()` 为 true

#### Scenario: 成功突发采样
- **WHEN** `clock_fn` 每次返回递增 1_000_000 ns（1ms），`send` 总是 `Ok(())`，调用 `run_burst(5, Duration::from_millis(1), ...)`
- **THEN** `sample_count == 5`，`results.len() == 5`，`sleep_fn` 被调用 5 次

#### Scenario: 部分 send 失败
- **WHEN** `send` 在第 2 次、第 4 次返回 `Err(())`，调用 `run_burst(5, ...)`
- **THEN** `sample_count == 3`，`results.len() == 3`，`sleep_fn` 被调用 5 次

### Requirement: compute_stats 统计计算

`LatencyProbe::compute_stats(&self) -> DelayStats` SHALL：
1. 若 `self.results` 为空，返回 `DelayStats::default()`
2. 否则克隆并 `sort()` results，计算：
   - `min = sorted[0]`
   - `max = sorted[n-1]`（n 为长度）
   - `mean = sorted.iter().sum::<Duration>() / n as u32`
   - `p99 = sorted[((n as f64 * 0.99) as usize).min(n - 1)]`
   - `p999 = sorted[((n as f64 * 0.999) as usize).min(n - 1)]`
   - `jitter = max - min`
   - `samples = n as u64`

#### Scenario: 空结果集
- **WHEN** `results` 为空
- **THEN** 返回 `DelayStats::default()`

#### Scenario: 单样本
- **WHEN** `results = vec![Duration::from_micros(200)]`
- **THEN** `min == max == mean == p99 == p999 == Duration::from_micros(200)`，`jitter == Duration::ZERO`，`samples == 1`

#### Scenario: 多样本统计
- **WHEN** `results = vec![100µs, 200µs, 300µs, 400µs, 500µs]`（5 样本）
- **THEN** `min == 100µs`、`max == 500µs`、`mean == 300µs`、`jitter == 400µs`、`samples == 5`
- **AND** `p99 == sorted[(5 * 0.99) as usize].min(4) == sorted[4] == 500µs`
- **AND** `p999 == sorted[(5 * 0.999) as usize].min(4) == sorted[4] == 500µs`

### Requirement: run 持续时长测量

`LatencyProbe::run(&mut self, duration: Duration, send: impl Fn() -> Result<(), ()>) -> DelayStats` SHALL：
1. 读取 `start_ns = (self.clock_fn)()`
2. 计算 `deadline = start_ns + duration.as_nanos() as u64`
3. 当 `(self.clock_fn)() < deadline` 时循环：
   - 调用 `measure_round_trip(&send)`，若 `Ok(d)` 则 push + sample_count++
   - 不调用 `sleep_fn`（与 `run_burst` 区别：run 是连续测量）
4. 返回 `self.compute_stats()`

#### Scenario: 时长到期停止
- **WHEN** `clock_fn` 第 1 次返回 0（作为 start_ns），第 2~5 次返回 100、200、300、400 ns（< deadline=1_000_000），第 6 次返回 1_500_000 ns（>= deadline），`send` 总返回 `Ok(())`
- **THEN** `results.len() == 4`（4 次循环测量），`sample_count == 4`

### Requirement: measure_e2e 端到端测量便捷方法

`LatencyProbe::measure_e2e(&mut self, samples: u32, send: impl Fn() -> Result<(), ()>) -> DelayStats` SHALL 等价于 `self.run_burst(samples, Duration::from_millis(1), send)`（默认 1ms 间隔，D16：蓝图 `topic` / `payload_size` 参数简化为闭包注入）。

#### Scenario: measure_e2e 委托 run_burst
- **WHEN** 调用 `measure_e2e(3, || Ok(()))`
- **THEN** 等价于 `run_burst(3, Duration::from_millis(1), || Ok(()))`，`sample_count == 3`

### Requirement: measure_under_load 负载下测量

`LatencyProbe::measure_under_load(&mut self, samples: u32, interval: Duration, background_load: impl Fn(), send: impl Fn() -> Result<(), ()>) -> DelayStats` SHALL：
1. 循环 `samples` 次：
   - 调用 `background_load()` 注入背景流量
   - 调用 `measure_round_trip(&send)`，若 `Ok(d)` 则 push + sample_count++
   - 调用 `(self.sleep_fn)(interval)`
2. 返回 `self.compute_stats()`

#### Scenario: 背景负载被调用
- **WHEN** 调用 `measure_under_load(3, Duration::from_millis(1), || load_count += 1, || Ok(()))`
- **THEN** `load_count == 3`，`sample_count == 3`

## MODIFIED Requirements

### Requirement: workspace members

根 `Cargo.toml` 的 `[workspace] members` 列表无需新增（`"crates/protocols/tsn-time"` 在 v0.79.0 已加入），保持不变。

### Requirement: 版本号同步

根 `Cargo.toml` / `Makefile` / `.github/workflows/ci.yml` 的版本号 SHALL 由 `0.80.0` 更新为 `0.81.0`。`crates/protocols/tsn-time/Cargo.toml` 的 `version` 与 `description` SHALL 更新为 `0.81.0` 与对应的 v0.81.0 描述（"EnerOS v0.81.0 gPTP + TSN 802.1Qbv 调度 + 端到端时延探针（无真实网络 I/O）"）。

`ci/src/gate.rs` 的 clippy 段与 test 段注释 SHALL 更新为 `eneros-tsn-time v0.81.0` 并列出新增类型（`DelayStats` / `LatencyProbe` / `TsnDriver` / `MockTsnDriver` / `TsnError` / `driver_send_closure`）。

### Requirement: tsn-time crate 模块声明

`crates/protocols/tsn-time/src/lib.rs` SHALL 在既有 `pub mod` 列表（按字母序：`bmca` / `clock` / `config_loader` / `gptp` / `port` / `stream` / `tas`）之后追加 `pub mod driver_glue;` 与 `pub mod latency_probe;`（保持字母序：`bmca` / `clock` / `config_loader` / `driver_glue` / `gptp` / `latency_probe` / `port` / `stream` / `tas`）。

`pub use` 导出 SHALL 在既有导出之后追加：
```rust
pub use driver_glue::{driver_send_closure, MockTsnDriver, TsnDriver, TsnError};
pub use latency_probe::{DelayStats, LatencyProbe};
```

模块顶部文档注释 SHALL 追加 v0.81.0 扩展段落（描述 TSN 驱动抽象 + 时延探针）。偏差声明 SHALL 追加 D15~D16 段落（保留 v0.79.0 D1~D14 / v0.80.0 D15~D19 / v0.81.0 D20~D25）。

> **注**：原 v0.80.0 D15~D19 编号保留不动；v0.81.0 新偏差从 D20 开始。

## REMOVED Requirements

### Requirement: 真实网络 I/O 与 DDS 发送/接收
**Reason**：CI 环境无法验证真实 TSN 网卡 + DDS 总线 + 硬件时间戳（需 TSN 交换机、Intel i210/i225 网卡、专用测试仪表）。蓝图 `LatencyProbe { sender: DdsWriter, receiver: DdsReader }` 在 no_std 协议层不可实现。
**Migration**：通过 closure 注入（`send: impl Fn() -> Result<(), ()>`）抽象发送动作；通过 `TsnDriver` trait + `MockTsnDriver` 提供可测试的驱动抽象；真实 DDS 集成延后到 v0.82.0+ Agent 使用阶段。

### Requirement: `eneros_time::Instant` 与 `eneros_time::delay()` 依赖
**Reason**：蓝图代码使用 `eneros_time::Instant::now()` 与 `start.elapsed()`、`eneros_time::delay(interval)`，但 `eneros-time` crate（v0.12.0）实际 API 为 `get_monotonic_ns() -> u64` 与 `sleep_until(deadline_ns: u64)`，无 `Instant` 类型与 `delay()` 函数。直接依赖 `eneros-time` 会在协议层 crate 引入 HAL 间接依赖，违反分层。
**Migration**：`LatencyProbe` 通过 `clock_fn: fn() -> u64` 与 `sleep_fn: fn(Duration)` 字段注入时间源与睡眠函数。测试用静态 `AtomicU64` 计数器模拟时钟，用空函数模拟睡眠。上层 Agent Runtime 可注入 `eneros_time::get_monotonic_ns` 与 `|d| eneros_time::sleep_until(get_monotonic_ns() + d.as_nanos() as u64)` 作为生产实现。

### Requirement: 性能基准测试与 p99 < 2ms 验收
**Reason**：CI 无法稳定验证 TC3 p99 < 2ms / p999 < 5ms / 抖动 < 1ms（需真实 TSN 硬件 + 测试仪表 + 网络拓扑）。
**Migration**：仅保留算法正确性单元测试（D10）。统计计算正确性通过已知输入验证（如 `vec![100µs, 200µs, ..., 500µs]` 期望 `min=100µs / max=500µs / mean=300µs / jitter=400µs`）。性能基准与硬件验收延后到 v0.82.0+ 集成测试阶段。

### Requirement: `tests/e2e_latency.rs` 与 `tests/jitter.rs` 集成测试文件
**Reason**：项目规则 §2.3 沿用 v0.75.0~v0.80.0 既有约定，单元测试内嵌 `src/lib.rs` 的 `#[cfg(test)] mod tests`（D4）。
**Migration**：v0.81.0 新增 T56~T84 共 29 个测试覆盖 `DelayStats` / `LatencyProbe` / `TsnDriver` / `driver_send_closure` 的全部场景，内嵌 `lib.rs` 末尾（在 v0.80.0 T55 之后，`sample_announce` helper 之前）。

### Requirement: 故障注入测试（网络拥塞下关键 TC 不受影响）
**Reason**：CI 无真实网络拥塞模拟环境，无法验证 TC3 在背景流量下不受影响。
**Migration**：通过 `measure_under_load` + `background_load: impl Fn()` 闭包注入模拟背景负载，验证算法逻辑（背景负载闭包被正确调用 N 次）。真实拥塞注入测试延后到硬件集成阶段。

---

## 偏差声明（D20~D25）

> v0.79.0 D1~D14 / v0.80.0 D15~D19 保留不变；以下为 v0.81.0 新增偏差。

| 偏差 | 说明 |
|------|------|
| **D20** | `driver_glue.rs` 与 `latency_probe.rs` 位于既有 crate `crates/protocols/tsn-time/`（项目规则 §2.3.1，非蓝图 `crates/tsn_time/`，沿用 D1） |
| **D21** | 文档位于 `docs/protocols/tsn-determinism-report.md`（项目规则 §2.3.3，非蓝图 `docs/phase2/tsn_determinism_report.md`，沿用 D2） |
| **D22** | 配置位于 `configs/latency_probe.toml`（项目规则 §2.3，非蓝图 `config/`，沿用 D3） |
| **D23** | 测试内嵌 `src/lib.rs` T56~T84（沿用 D4，非蓝图 `tests/e2e_latency.rs` / `tests/jitter.rs`） |
| **D24** | 无 `eneros_time::Instant` / `eneros_time::delay()` 依赖 — `LatencyProbe` 通过 `clock_fn: fn() -> u64` + `sleep_fn: fn(Duration)` 字段注入（蓝图 API 不存在；避免协议层 crate 引入 HAL 间接依赖） |
| **D25** | 无真实 DDS / TSN 硬件 I/O — `TsnDriver` trait + `MockTsnDriver` 抽象数据面（沿用 v0.80.0 `NicApplier` 模式）；`measure_round_trip` / `run` / `run_burst` / `measure_e2e` / `measure_under_load` 通过 `send: impl Fn() -> Result<(), ()>` 闭包注入发送动作；`driver_send_closure` 适配器桥接 `TsnDriver` → send 闭包 |

## no_std 合规

本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
仅使用 `alloc::*` 与 `core::*`，无外部依赖（无 `log` / `uuid` / `serde` / `smoltcp` / `eneros-time` / `eneros-hal` 等）。
不调用 `panic!` / `todo!` / `unimplemented!`，不含 `unsafe` 块。

**关键 no_std 设计**：
- `DelayStats` 使用 `core::time::Duration`（实现 `Ord` / `Sum` / `Default` / `Sub` / `Div<u32>`，no_std 可用）
- `LatencyProbe.clock_fn` / `sleep_fn` 为 `fn() -> u64` / `fn(Duration)` 函数指针（非 `Box<dyn Fn>`，避免 `alloc::boxed::Box` 与动态分发开销，且测试中可传递静态函数）
- `compute_stats` 使用 `f64` 中间计算（`core::ops` 提供，no_std 可用），最终索引转 `usize`
- `Vec::sort()` 使用 `Ord` trait（`alloc` 提供）
