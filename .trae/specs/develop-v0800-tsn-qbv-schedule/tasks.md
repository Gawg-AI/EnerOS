# Tasks

- [x] Task 1: 升级 crate 版本号 `crates/protocols/tsn-time/Cargo.toml`
  - [x] SubTask 1.1: `version = "0.79.0"` → `version = "0.80.0"`
  - [x] SubTask 1.2: `description` 更新为 "EnerOS v0.80.0 gPTP + TSN 802.1Qbv 调度 — 时间感知整形（无真实网络 I/O）"

- [x] Task 2: 实现 `crates/protocols/tsn-time/src/tas.rs` — TAS 核心类型与调度算法
  - [x] SubTask 2.1: `TrafficClass` 枚举（`#[repr(u8)]`，8 变体 Be/BK/EE/CA/VO/VI/NC/ST）+ `code() -> u8` + `from_code(u8) -> Option<Self>` + 派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`
  - [x] SubTask 2.2: `Packet` 结构体（`ethertype: u16` / `dscp: u8` / `pcp: u8`）+ `is_ptp()` / `is_goose()` / `is_sv()` 方法（D5：最小数据集，无真实抓包）
  - [x] SubTask 2.3: `GateState { duration: Duration, gates: u8 }` + `GateControlList { entries: Vec<GateState>, cycle_count: u32 }` + `new()` + `increment_cycle()`
  - [x] SubTask 2.4: `TasScheduleEntry { duration_us: u64, gate_mask: u8 }` + `TasConfig { cycle_us, base_time_s, schedule, port_count }` + `Default`（cycle_us=1_000_000, base_time_s=0, schedule=Vec::new(), port_count=1）
  - [x] SubTask 2.5: `TasPort { port_id: u8, applied: bool }` + `new(port_id)`（applied=false 初始）
  - [x] SubTask 2.6: `TasError` 枚举（`ScheduleGap { expected, actual }` / `TooShort(Duration)` / `NicApplyFailed` / `InvalidConfig`），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 2.7: `NicApplier` trait（`apply(&mut self, iface: &str, config: &GateControlList) -> Result<(), TasError>`）
  - [x] SubTask 2.8: `MockNicApplier { applied: Vec<(String, u32)>, fail: bool }` + `new()` + 实现 `NicApplier` trait（D6：无真实 netlink/taprio）
  - [x] SubTask 2.9: `TasScheduler { ports: Vec<TasPort>, base_time: PtpTime, cycle_time: Duration, config: GateControlList }`
  - [x] SubTask 2.10: `TasScheduler::new(config: &TasConfig) -> Self`（D7：使用 `PtpTime::new(config.base_time_s, 0)`，不修改 v0.79.0 clock.rs）
  - [x] SubTask 2.11: `TasScheduler::validate_schedule(&self) -> Result<(), TasError>`（D9：总 duration == cycle_time 且每条 >= 5µs）
  - [x] SubTask 2.12: `TasScheduler::classify_packet(&self, pkt: &Packet) -> TrafficClass`（D12：PTP/GOOSE/SV ethertype 优先，否则 DSCP 分段）
  - [x] SubTask 2.13: `TasScheduler::next_gate_window(&self, tc: TrafficClass) -> Duration`（D11：遍历 GCL 找首匹配，无匹配返回 cycle_time）
  - [x] SubTask 2.14: `TasScheduler::apply_to_nic(&mut self, applier: &mut dyn NicApplier, iface: &str) -> Result<(), TasError>`（先 validate 再 apply）

- [x] Task 3: 实现 `crates/protocols/tsn-time/src/stream.rs` — Stream 过滤数据类型（最小骨架）
  - [x] SubTask 3.1: `StreamId(pub u32)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, Hash` + 实现 `Display` + `new(u32)`
  - [x] SubTask 3.2: `StreamFilter { stream_id: StreamId, gate_id: u8, priority: u8 }` 结构体，派生 `Debug, Clone, PartialEq, Eq` + `new(stream_id, gate_id, priority)`（D14：无真实 802.1Qci 过滤逻辑）

- [x] Task 4: 实现 `crates/protocols/tsn-time/src/config_loader.rs` — 配置构造器
  - [x] SubTask 4.1: `build_tas_config(cycle_us: u64, base_time_s: u64, entries: Vec<TasScheduleEntry>, port_count: u8) -> TasConfig` 函数（D15：纯 Rust 构造，无 TOML 解析）

- [x] Task 5: 修改 `crates/protocols/tsn-time/src/lib.rs` — 模块声明 + 重新导出 + 测试 + 偏差表
  - [x] SubTask 5.1: 新增 `pub mod tas;` / `pub mod stream;` / `pub mod config_loader;`（alphabetical 顺序）
  - [x] SubTask 5.2: 新增 `pub use tas::{TrafficClass, Packet, GateState, GateControlList, TasPort, TasConfig, TasScheduleEntry, TasError, TasScheduler, NicApplier, MockNicApplier};`
  - [x] SubTask 5.3: 新增 `pub use stream::{StreamId, StreamFilter};`
  - [x] SubTask 5.4: 新增 `pub use config_loader::build_tas_config;`
  - [x] SubTask 5.5: 更新顶部模块文档注释，描述 v0.80.0 扩展（gPTP + TSN 802.1Qbv）
  - [x] SubTask 5.6: 追加 D1~D19 偏差声明表到现有 D1~D14 之后（保留 v0.79.0 偏差表，新增 v0.80.0 偏差段落）
  - [x] SubTask 5.7: 新增 T26~T52 测试（27 个测试，覆盖 TrafficClass / Packet / GateState / GCL / TasConfig / TasPort / TasScheduler / StreamFilter / apply_to_nic / build_tas_config）
  - [x] SubTask 5.8: 保留 v0.79.0 T1~T25 测试不变（Surgical Changes 原则）

- [x] Task 6: 创建配置文件 `configs/tas.toml`
  - [x] SubTask 6.1: TOML 模板含 `cycle_us` / `base_time_s` / `port_count` 字段 + `[[schedule]]` 数组（每条 `duration_us` / `gate_mask`）

- [x] Task 7: 创建设计文档 `docs/protocols/tsn-qbv-design.md`
  - [x] SubTask 7.1: 12 章节完整（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
  - [x] SubTask 7.2: 2 Mermaid 图（802.1Qbv 门控周期甘特图 + classify_packet 决策流程图）
  - [x] SubTask 7.3: D1~D19 偏差声明表

- [x] Task 8: 版本同步根目录文件
  - [x] SubTask 8.1: 根 `Cargo.toml` 顶层 `version = "0.79.0"` → `"0.80.0"`（workspace members `"crates/protocols/tsn-time"` 已存在，无需新增）
  - [x] SubTask 8.2: `Makefile` 版本号 `0.79.0` → `0.80.0`（header 注释 + VERSION 变量）
  - [x] SubTask 8.3: `.github/workflows/ci.yml` 版本号 `0.79.0` → `0.80.0`
  - [x] SubTask 8.4: `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-tsn-time` 类型列表至 v0.80.0（追加 `TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config`）

- [x] Task 9: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 9.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 9.2: `cargo test -p eneros-tsn-time` 全部通过（T1~T52 = 52 tests + 1 doctest）
  - [x] SubTask 9.3: `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 9.4: `cargo fmt -p eneros-tsn-time -- --check` 通过
  - [x] SubTask 9.5: `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning
  - [x] SubTask 9.6: `cargo deny check licenses bans sources` 通过
  - [x] SubTask 9.7: 回归 — v0.75.0~v0.79.0 现有测试仍全绿（eneros-agent-bus-dds 63 tests + 1 doctest + eneros-tsn-time T1~T25 仍通过，无回归）

# Task Dependencies

- Task 1（升级 Cargo.toml 版本）必须先完成 — 后续所有 Task 依赖 crate 已升级
- Task 2（tas.rs）是核心 — Task 5 的导出与测试依赖之；Task 7 的设计文档需引用 tas.rs 类型
- Task 3（stream.rs）独立 — 可与 Task 2 并行
- Task 4（config_loader.rs）依赖 Task 2 完成（使用 `TasConfig` / `TasScheduleEntry` 类型）
- Task 5（lib.rs）依赖 Task 2/3/4 完成（需导出三个模块的类型）
- Task 6/7（配置 + 文档）可与 Task 2~5 并行
- Task 8（版本同步）依赖 Task 1~7 完成
- Task 9（构建校验）依赖所有前置任务完成
