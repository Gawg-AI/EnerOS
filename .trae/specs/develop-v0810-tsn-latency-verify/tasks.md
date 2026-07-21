# Tasks

- [x] Task 1: 升级 crate 版本号 `crates/protocols/tsn-time/Cargo.toml`
  - [x] SubTask 1.1: `version = "0.80.0"` → `version = "0.81.0"`
  - [x] SubTask 1.2: `description` 更新为 "EnerOS v0.81.0 gPTP + TSN 802.1Qbv 调度 + 端到端时延探针（无真实网络 I/O）"

- [x] Task 2: 实现 `crates/protocols/tsn-time/src/driver_glue.rs` — TSN 驱动抽象层
  - [x] SubTask 2.1: `TsnError` 枚举（`SendFailed` / `RecvFailed` / `NotInitialized`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 2.2: `TsnDriver` trait（`send(&mut self, tc: TrafficClass, payload: &[u8]) -> Result<(), TsnError>` + `recv(&mut self) -> Result<alloc::vec::Vec<u8>, TsnError>`）
  - [x] SubTask 2.3: `MockTsnDriver { sent: Vec<(TrafficClass, Vec<u8>)>, recv_queue: Vec<Vec<u8>>, fail_send: bool, fail_recv: bool }` + `new()` + `push_recv(data: Vec<u8>)`
  - [x] SubTask 2.4: 实现 `TsnDriver` for `MockTsnDriver`（send 记录 + 失败短路；recv pop_front 或返回 `Err(RecvFailed)`，使用 `Vec::pop()` 简化）
  - [x] SubTask 2.5: `driver_send_closure<'a>(driver: &'a mut dyn TsnDriver, tc: TrafficClass, payload: &'a [u8]) -> impl FnMut() -> Result<(), ()> + 'a` 适配器函数（D26 偏差：闭包捕获 `&mut T` 调用 `&mut self` 方法，仅能实现 `FnMut`）

- [x] Task 3: 实现 `crates/protocols/tsn-time/src/latency_probe.rs` — 时延探针与统计
  - [x] SubTask 3.1: `DelayStats { min, max, mean, p99, p999, jitter, samples: u64 }` 结构体，派生 `Debug, Clone, Copy, PartialEq, Eq, Default`（Default 全零；字段类型 `u64` 而非 `Duration`，因 `clock_fn: fn() -> u64` 返回纳秒）
  - [x] SubTask 3.2: `LatencyProbe { sample_count: u32, results: Vec<u64>, clock_fn: fn() -> u64, sleep_fn: fn(Duration) }` 结构体（`results: Vec<u64>` 而非 `Vec<Duration>`，与 `clock_fn` 返回类型对齐）
  - [x] SubTask 3.3: `LatencyProbe::new(sample_count, clock_fn, sleep_fn) -> Self`（sample_count=0, results=Vec::new()）
  - [x] SubTask 3.4: `measure_round_trip(&mut self, send: &mut impl FnMut() -> Result<(), ()>) -> Result<Duration, ()>`（clock_fn 起止差值；send 失败立即返回 `Err(())`；D26 偏差：`&mut impl FnMut()` 允许同一闭包在 `run_burst` 多轮循环中被反复调用）
  - [x] SubTask 3.5: `run_burst(&mut self, count: u32, interval: Duration, mut send: impl FnMut() -> Result<(), ()>) -> DelayStats`（循环 + sleep_fn 调用，无论成败都 sleep）
  - [x] SubTask 3.6: `compute_stats(&self) -> DelayStats`（空 → Default；否则 sort + min/max/mean/p99/p999/jitter/samples）
  - [x] SubTask 3.7: `run(&mut self, duration: Duration, mut send: impl FnMut() -> Result<(), ()>) -> DelayStats`（基于 clock_fn deadline 循环，不调用 sleep_fn）
  - [x] SubTask 3.8: `measure_e2e(&mut self, samples: u32, mut send: impl FnMut() -> Result<(), ()>) -> DelayStats`（委托 `run_burst(samples, Duration::from_millis(1), send)`）
  - [x] SubTask 3.9: `measure_under_load(&mut self, samples: u32, interval: Duration, background_load: impl Fn(), mut send: impl FnMut() -> Result<(), ()>) -> DelayStats`（每轮先 background_load()，再 measure_round_trip，再 sleep_fn）

- [x] Task 4: 修改 `crates/protocols/tsn-time/src/lib.rs` — 模块声明 + 重新导出 + 测试 + 偏差表
  - [x] SubTask 4.1: 新增 `pub mod driver_glue;` 与 `pub mod latency_probe;`（按字母序插入既有 `pub mod` 列表）
  - [x] SubTask 4.2: 新增 `pub use driver_glue::{driver_send_closure, MockTsnDriver, TsnDriver, TsnError};`
  - [x] SubTask 4.3: 新增 `pub use latency_probe::{DelayStats, LatencyProbe};`
  - [x] SubTask 4.4: 更新顶部模块文档注释，追加 v0.81.0 扩展段落（TSN 驱动抽象 + 时延探针，无真实网络 I/O）
  - [x] SubTask 4.5: 追加 D20~D26 偏差声明段落到现有 D15~D19 之后（保留 v0.79.0 D1~D14 / v0.80.0 D15~D19 不变；新增 D26：FnMut 偏差）
  - [x] SubTask 4.6: 新增 T56~T84 测试（29 个测试，覆盖 DelayStats / LatencyProbe / TsnDriver / MockTsnDriver / driver_send_closure / run / run_burst / compute_stats / measure_e2e / measure_under_load）
  - [x] SubTask 4.7: 保留 v0.79.0 T1~T25 / v0.80.0 T26~T55 测试不变（Surgical Changes 原则）
  - [x] SubTask 4.8: 测试模块新增 `use core::sync::atomic::{AtomicU64, Ordering}` 与静态 `CLOCK_NS` / `SLEEP_COUNT` 计数器辅助测试

- [x] Task 5: 创建配置文件 `configs/latency_probe.toml`
  - [x] SubTask 5.1: TOML 模板含 `sample_count` / `interval_us` / `burst_count` / `duration_us` 字段，附注释说明用途

- [x] Task 6: 创建设计文档 `docs/protocols/tsn-determinism-report.md`
  - [x] SubTask 6.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
  - [x] SubTask 6.2: 2 Mermaid 图（时延测量 sequence diagram + compute_stats 流程图）
  - [x] SubTask 6.3: D20~D25 偏差声明表 + v0.79.0 D1~D14 / v0.80.0 D15~D19 历史回顾表

- [x] Task 7: 版本同步根目录文件
  - [x] SubTask 7.1: 根 `Cargo.toml` 顶层 `version = "0.80.0"` → `"0.81.0"`（workspace members 无需新增）
  - [x] SubTask 7.2: `Makefile` 版本号 `0.80.0` → `0.81.0`（header 注释 + VERSION 变量）
  - [x] SubTask 7.3: `.github/workflows/ci.yml` 版本号 `0.80.0` → `0.81.0`
  - [x] SubTask 7.4: `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-tsn-time` 类型列表至 v0.81.0（追加 `DelayStats / LatencyProbe / TsnDriver / MockTsnDriver / TsnError / driver_send_closure`）

- [x] Task 8: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.2: `cargo test -p eneros-tsn-time` 全部通过（T1~T84 = 84 tests + 1 doctest）
  - [x] SubTask 8.3: `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt -p eneros-tsn-time -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning
  - [x] SubTask 8.6: `cargo deny check advisories licenses bans sources` 通过
  - [x] SubTask 8.7: 回归 — v0.75.0~v0.80.0 现有测试仍全绿（eneros-agent-bus-dds 63 tests + 1 doctest + eneros-tsn-time T1~T55 仍通过，无回归）

# Task Dependencies

- Task 1（升级 Cargo.toml 版本）必须先完成 — 后续所有 Task 依赖 crate 已升级
- Task 2（driver_glue.rs）是核心 — Task 4 的导出与测试依赖之；Task 3 的 `driver_send_closure` 适配器依赖 `TsnDriver` trait；Task 6 设计文档需引用类型
- Task 3（latency_probe.rs）依赖 Task 2 完成（`measure_under_load` / 集成测试场景使用 `TsnDriver`）
- Task 4（lib.rs）依赖 Task 2/3 完成（需导出两个模块的类型）
- Task 5/6（配置 + 文档）可与 Task 2~3 并行
- Task 7（版本同步）依赖 Task 1~6 完成
- Task 8（构建校验）依赖所有前置任务完成
