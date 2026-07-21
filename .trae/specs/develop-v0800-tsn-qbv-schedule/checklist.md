# Checklist

## tas.rs — 新建
- [x] C1 `TrafficClass` 枚举（`#[repr(u8)]`，8 变体：Be(0) / BK(1) / EE(2) / CA(3) / VO(4) / VI(5) / NC(6) / ST(7)），派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`
- [x] C2 `TrafficClass::code(&self) -> u8` 返回变体编码
- [x] C3 `TrafficClass::from_code(u8) -> Option<Self>` 反编码（0~7 → Some，8+ → None）
- [x] C4 `Packet` 结构体（`ethertype: u16` / `dscp: u8` / `pcp: u8`），派生 `Debug, Clone, Copy, PartialEq, Eq`（D5）
- [x] C5 `Packet::is_ptp() -> bool`（ethertype == 0x88F7）
- [x] C6 `Packet::is_goose() -> bool`（ethertype == 0x88B8）
- [x] C7 `Packet::is_sv() -> bool`（ethertype == 0x88BA）
- [x] C8 `GateState { duration: Duration, gates: u8 }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C9 `GateControlList { entries: Vec<GateState>, cycle_count: u32 }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C10 `GateControlList::new(entries: Vec<GateState>) -> Self`（cycle_count = 0 初始）
- [x] C11 `GateControlList::increment_cycle(&mut self)`（cycle_count 自增 1）
- [x] C12 `TasScheduleEntry { duration_us: u64, gate_mask: u8 }` 结构体，派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C13 `TasConfig { cycle_us: u64, base_time_s: u64, schedule: Vec<TasScheduleEntry>, port_count: u8 }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C14 `TasConfig::default()` 返回 `cycle_us = 1_000_000` / `base_time_s = 0` / `schedule = Vec::new()` / `port_count = 1`
- [x] C15 `TasPort { port_id: u8, applied: bool }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C16 `TasPort::new(port_id: u8) -> Self`（applied = false 初始）
- [x] C17 `TasError` 枚举（`ScheduleGap { expected: Duration, actual: Duration }` / `TooShort(Duration)` / `NicApplyFailed` / `InvalidConfig`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C18 `NicApplier` trait（`apply(&mut self, iface: &str, config: &GateControlList) -> Result<(), TasError>`）
- [x] C19 `MockNicApplier { applied: Vec<(String, u32)>, fail: bool }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C20 `MockNicApplier::new() -> Self`（applied = Vec::new(), fail = false）
- [x] C21 `MockNicApplier` 实现 `NicApplier` trait（fail=false 追加 (iface.to_string(), entry_count)；fail=true 返回 `Err(NicApplyFailed)`）
- [x] C22 `TasScheduler { ports: Vec<TasPort>, base_time: PtpTime, cycle_time: Duration, config: GateControlList }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C23 `TasScheduler::new(config: &TasConfig) -> Self`（D7：使用 `PtpTime::new(config.base_time_s, 0)`）
- [x] C24 `TasScheduler::validate_schedule(&self) -> Result<(), TasError>`（D9：闭合 + 时长检查）
- [x] C25 `TasScheduler::classify_packet(&self, pkt: &Packet) -> TrafficClass`（D12：PTP/GOOSE/SV 优先，DSCP 分段）
- [x] C26 `TasScheduler::next_gate_window(&self, tc: TrafficClass) -> Duration`（D11：遍历找首匹配）
- [x] C27 `TasScheduler::apply_to_nic(&mut self, applier: &mut dyn NicApplier, iface: &str) -> Result<(), TasError>`（先 validate 再 apply，applied=true 标记）

## stream.rs — 新建
- [x] C28 `StreamId(pub u32)` newtype，派生 `Debug, Clone, Copy, PartialEq, Eq, Hash`
- [x] C29 `StreamId` 实现 `Display`（直接输出 u32 数字）
- [x] C30 `StreamId::new(u32) -> Self`
- [x] C31 `StreamFilter { stream_id: StreamId, gate_id: u8, priority: u8 }` 结构体，派生 `Debug, Clone, PartialEq, Eq`
- [x] C32 `StreamFilter::new(stream_id, gate_id, priority) -> Self`（D14：纯数据，无过滤逻辑）

## config_loader.rs — 新建
- [x] C33 `build_tas_config(cycle_us: u64, base_time_s: u64, entries: Vec<TasScheduleEntry>, port_count: u8) -> TasConfig` 函数（D15：无 TOML 解析）

## lib.rs — 模块声明 + 导出 + 测试 + 偏差表（修改）
- [x] C34 新增 `pub mod config_loader;` / `pub mod stream;` / `pub mod tas;`（alphabetical 顺序，与 v0.79.0 bmca/clock/gptp/port 一致）
- [x] C35 新增 `pub use config_loader::build_tas_config;`
- [x] C36 新增 `pub use stream::{StreamFilter, StreamId};`
- [x] C37 新增 `pub use tas::{GateControlList, GateState, MockNicApplier, NicApplier, Packet, TasConfig, TasError, TasPort, TasScheduleEntry, TasScheduler, TrafficClass};`
- [x] C38 顶部模块文档注释更新为 "v0.80.0 gPTP + TSN 802.1Qbv 调度"
- [x] C39 追加 D1~D19 偏差声明表（保留 v0.79.0 D1~D14 不变，新增 v0.80.0 偏差段落）
- [x] C40 T26 新增：`TrafficClass::code()` — 8 变体返回 0~7
- [x] C41 T27 新增：`TrafficClass::from_code()` — 0~7 返回 Some，8+ 返回 None
- [x] C42 T28 新增：`Packet::is_ptp() / is_goose() / is_sv()` ethertype 识别
- [x] C43 T29 新增：`Packet` 构造与字段访问
- [x] C44 T30 新增：`GateState::new()`（或结构体字面量）+ 字段访问
- [x] C45 T31 新增：`GateControlList::new()` cycle_count=0 初始
- [x] C46 T32 新增：`GateControlList::increment_cycle()` 自增
- [x] C47 T33 新增：`TasConfig::default()` 字段验证（cycle_us=1_000_000 / port_count=1 / schedule=[]）
- [x] C48 T34 新增：`TasPort::new()` applied=false
- [x] C49 T35 新增：`TasScheduler::new()` 字段验证（base_time = PtpTime::new(base_time_s, 0)）
- [x] C50 T36 新增：`validate_schedule()` OK（闭合 + 时长 >= 5µs）
- [x] C51 T37 新增：`validate_schedule()` ScheduleGap 错误（sum != cycle_time）
- [x] C52 T38 新增：`validate_schedule()` TooShort 错误（duration < 5µs）
- [x] C53 T39 新增：`classify_packet()` PTP ethertype → NC
- [x] C54 T40 新增：`classify_packet()` GOOSE ethertype → VO
- [x] C55 T41 新增：`classify_packet()` SV ethertype → VI
- [x] C56 T42 新增：`classify_packet()` DSCP 0-7 → BE
- [x] C57 T43 新增：`classify_packet()` DSCP 8-15 → BK
- [x] C58 T44 新增：`classify_packet()` DSCP 24-31 → EE
- [x] C59 T45 新增：`classify_packet()` DSCP 40-47 → CA
- [x] C60 T46 新增：`classify_packet()` DSCP 48 → BE（默认分支）
- [x] C61 T47 新增：`next_gate_window()` TC6 首窗口（0µs）
- [x] C62 T48 新增：`next_gate_window()` TC3 第二窗口（50µs）
- [x] C63 T49 新增：`next_gate_window()` TC 永未开放 → cycle_time
- [x] C64 T50 新增：`apply_to_nic()` Mock 成功（mock.applied.len() == 1）
- [x] C65 T51 新增：`apply_to_nic()` 调度非法时不下发（mock.applied.is_empty()）
- [x] C66 T52 新增：`StreamId` 构造 + Display 输出 "42"
- [x] C67 `build_tas_config()` 测试在 config_loader.rs 或 lib.rs 内（构造器组装验证）
- [x] C68 保留 v0.79.0 T1~T25 不变（Surgical Changes 原则）

## Cargo.toml — 版本升级
- [x] C69 包版本 `0.79.0` → `0.80.0`
- [x] C70 description 更新为 "EnerOS v0.80.0 gPTP + TSN 802.1Qbv 调度 — 时间感知整形（无真实网络 I/O）"

## 配置文件
- [x] C71 `configs/tas.toml` 存在
- [x] C72 含字段 `cycle_us` / `base_time_s` / `port_count` + `[[schedule]]` 数组（每条 `duration_us` / `gate_mask`）

## 设计文档
- [x] C73 `docs/protocols/tsn-qbv-design.md` 存在
- [x] C74 12 章节完整（版本目标 / 前置依赖 / 交付物 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
- [x] C75 2 Mermaid 图（802.1Qbv 门控周期甘特图 + classify_packet 决策流程图）
- [x] C76 D1~D19 偏差声明表
- [x] C77 文档在 `docs/protocols/` 下（非蓝图 `docs/phase2/`，D2）

## workspace members + 版本同步
- [x] C78 根 `Cargo.toml` 顶层 `version = "0.80.0"`
- [x] C79 workspace members 中 `"crates/protocols/tsn-time"` 已存在（v0.79.0 注册，无需新增）
- [x] C80 `Makefile` 版本号 `0.80.0`（header 注释 + VERSION 变量）
- [x] C81 `.github/workflows/ci.yml` 版本号 `0.80.0`
- [x] C82 `ci/src/gate.rs` clippy 段注释含 `eneros-tsn-time v0.80.0` 与新类型列表（TrafficClass / Packet / GateState / GateControlList / TasPort / TasConfig / TasScheduleEntry / TasError / TasScheduler / NicApplier / MockNicApplier / StreamId / StreamFilter / build_tas_config）
- [x] C83 `ci/src/gate.rs` test 段注释同上

## 构建校验（§2.4.2 C6~C11）
- [x] C84 `cargo metadata --format-version 1` 成功
- [x] C85 `cargo test -p eneros-tsn-time` 全部通过（T1~T52 = 52 tests + 1 doctest）
- [x] C86 `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C87 `cargo fmt -p eneros-tsn-time -- --check` 通过
- [x] C88 `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning
- [x] C89 `cargo deny check licenses bans sources` 通过
- [x] C90 回归 — v0.75.0~v0.78.0 现有测试仍全绿（eneros-agent-bus-dds 63 tests + 1 doctest 通过，无回归）

## no_std 合规
- [x] C91 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C92 无 `panic!` / `todo!` / `unimplemented!`
- [x] C93 子模块（tas.rs / stream.rs / config_loader.rs）不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] C94 无 `std::collections::HashMap`
- [x] C95 无 `Send + Sync` bound
- [x] C96 无 `log` crate 依赖
- [x] C97 无 `toml` / `serde` / `serde_json` crate 依赖（D15）
- [x] C98 无 `nix` / `socketcan` / `pcap` 等 Linux 特定 crate 依赖（D6）
- [x] C99 无 `unsafe` 块
- [x] C100 使用 `core::time::Duration`（no_std 可用）

## 目录规范
- [x] C101 复用 v0.79.0 的 `crates/protocols/tsn-time/`（不新建 crate，D1）
- [x] C102 文档在 `docs/protocols/` 下（D2）
- [x] C103 配置在 `configs/` 下（D3）
- [x] C104 无根目录 crate（除 `ci/`）
- [x] C105 无垃圾文件（target/ / *.elf / *.bin / IDE 缓存）

## Surgical Changes 验证（Karpathy 原则）
- [x] C106 v0.79.0 的 `clock.rs` 文件未修改
- [x] C107 v0.79.0 的 `port.rs` 文件未修改
- [x] C108 v0.79.0 的 `bmca.rs` 文件未修改
- [x] C109 v0.79.0 的 `gptp.rs` 文件未修改
- [x] C110 v0.79.0 的 T1~T25 测试保留不变
- [x] C111 v0.79.0 的 `pub use bmca::...` / `pub use clock::...` / `pub use gptp::...` / `pub use port::...` 导出保留不变
- [x] C112 仅在 `lib.rs` 末尾追加新模块声明与导出（不重排已有声明）

## 简化设计验证（Karpathy 原则）
- [x] C113 无真实 netlink/taprio 下发代码（D6：通过 `NicApplier` trait + `MockNicApplier` 抽象）
- [x] C114 无 `Packet` 真实抓包逻辑（D5：最小数据集，由上层构造）
- [x] C115 无 `PtpTime::from_unix()` 修改（D7：使用现有 `PtpTime::new()`）
- [x] C116 无 `toml` 解析（D15：纯 Rust 构造器 `build_tas_config`）
- [x] C117 无真实 802.1Qci per-stream 过滤逻辑（D14：仅数据类型）
- [x] C118 无 `TasPort.gate_states` 冗余字段（D10：调度仅在 `GateControlList`）
- [x] C119 无 `Send + Sync` bound（D18：沿用 v0.79.0 单线程先例）

## 破坏性变更
- [x] C120 无破坏性变更（纯扩展；v0.79.0 类型签名不变；v0.75.0~v0.78.0 类型签名不变；默认 feature 不变）
