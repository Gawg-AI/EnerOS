# Checklist

## clock.rs — 新建
- [x] C1 `ClockIdentity(pub [u8; 8])` newtype（D13），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C2 `ClockIdentity` 实现 `Display`（冒号分隔十六进制，如 `01:23:45:67:89:AB:CD:EF`）
- [x] C3 `MacAddr(pub [u8; 6])` newtype（D14），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`
- [x] C4 `MacAddr` 实现 `Display`（冒号分隔十六进制，如 `01:23:45:67:89:AB`）
- [x] C5 `PtpTime { seconds: u64, nanos: u32 }` 结构体，派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C6 `PtpTime::new(seconds: u64, nanos: u32) -> Self` 构造函数
- [x] C7 `PtpTime::to_ns(&self) -> i128`（`seconds * 1_000_000_000 + nanos`）
- [x] C8 `PtpTime::add_ns(&mut self, ns: i64)`（正负均可，处理 nanos 进位/借位到 seconds）
- [x] C9 `PtpTime::diff_ns(&self, other: &PtpTime) -> i64`（`self.to_ns() - other.to_ns()` 截断为 i64）
- [x] C10 无 `use std::*`（仅 `core::*`）
- [x] C11 不调用 `PtpTime::now()`（D7）

## port.rs — 新建
- [x] C12 `PortRole` 枚举（`Master` / `Slave` / `Passive` / `Disabled`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C13 `PortRole` 实现 `Display`
- [x] C14 `PortState` 枚举（`Initializing` / `Listening` / `Master` / `Slave` / `Passive`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C15 `PortState` 实现 `Display`
- [x] C16 `Port` 结构体（`port_id: u16` / `role: PortRole` / `state: PortState` / `mac: MacAddr` / `hw_timestamping: bool`），派生 `Debug, Clone, PartialEq, Eq`
- [x] C17 `Port::new(port_id: u16, mac: MacAddr, hw_timestamping: bool) -> Self`（`role = Disabled`、`state = Initializing`）

## bmca.rs — 新建
- [x] C18 `AnnounceMessage` 结构体（8 字段：`grandmaster_identity` / `priority1` / `clock_class` / `accuracy` / `priority2` / `steps_removed` / `source_port_id` / `source_mac`），派生 `Debug, Clone, PartialEq, Eq`
- [x] C19 `BmcaResult` 枚举（`ElectedAsMaster` / `FollowMaster(ClockIdentity)`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C20 `BmcaResult` 实现 `Display`
- [x] C21 `compare_priority(a: &AnnounceMessage, b: &AnnounceMessage) -> core::cmp::Ordering` 按 BMCA 优先级顺序比较（`priority1` → `clock_class` → `accuracy` → `priority2` → `grandmaster_identity`，数值小者优先）

## gptp.rs — 新建
- [x] C22 `SyncMessage` 结构体（3 字段：`origin_timestamp: PtpTime` / `sequence_id: u16` / `steps_removed: u16`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C23 `FollowUpMessage` 结构体（2 字段：`sequence_id: u16` / `precise_origin_timestamp: PtpTime`），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C24 `GptpConfig` 结构体（`priority1: u8` / `priority2: u8` / `ports: Vec<Port>`），派生 `Debug, Clone`
- [x] C25 `GptpConfig::default()` 返回 `priority1 = 128` / `priority2 = 0` / `ports = Vec::new()`
- [x] C26 `GptpClock` 结构体（13 基础字段 + `last_sync_seq_id` / `last_sync_rx_ts` / `last_sync_delay_ns` / `last_sync_origin_ts` 用于 FollowUp 配对），派生 `Debug, Clone`
- [x] C27 `GptpClock::new(identity: ClockIdentity, initial_time: PtpTime, config: &GptpConfig) -> Self`（D7：`initial_time` 参数注入）
- [x] C28 `GptpClock::run_bmca(&mut self, announces: &[AnnounceMessage]) -> BmcaResult`
- [x] C29 `GptpClock::handle_sync(&mut self, sync: &SyncMessage, rx_ts: PtpTime, delay_ns: i64)`（D9：`delay_ns` 参数修复蓝图 bug）
- [x] C30 `GptpClock::handle_sync()` 低通滤波：`self.offset = (self.offset * 7 + new_offset) / 8`
- [x] C31 `GptpClock::handle_follow_up(&mut self, fu: &FollowUpMessage)`（按 `sequence_id` 匹配最近 Sync，重新计算 offset）
- [x] C32 `GptpClock::adjust_clock(&mut self, offset: i64)`（D6：小偏移仅存 `frequency_offset`，大偏移跳跃并记录 `last_jump_ns`）
- [x] C33 `GptpClock::compute_offset(&self) -> i64`（返回 `self.offset`）
- [x] C34 `GptpClock::current_time(&self) -> PtpTime`（返回 `current_time + offset`）
- [x] C35 `GptpClock::to_announce(&self) -> AnnounceMessage`（构造反映本时钟状态的 Announce）

## lib.rs — 模块声明 + 导出 + 测试
- [x] C36 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C37 5 个 `pub mod` 声明（`bmca` / `clock` / `gptp` / `port`，alphabetical 顺序；tests 为 `#[cfg(test)] mod tests`）
- [x] C38 `pub use bmca::{compare_priority, AnnounceMessage, BmcaResult};`
- [x] C39 `pub use clock::{ClockIdentity, MacAddr, PtpTime};`
- [x] C40 `pub use gptp::{FollowUpMessage, GptpClock, GptpConfig, SyncMessage};`
- [x] C41 `pub use port::{Port, PortRole, PortState};`
- [x] C42 顶部模块文档注释描述 v0.79.0 gPTP 时间同步层
- [x] C43 完整 D1~D14 偏差声明表
- [x] C44 T1 新增：`ClockIdentity` 构造 + Display 输出
- [x] C45 T2 新增：`MacAddr` 构造 + Display 输出
- [x] C46 T3 新增：`PtpTime::new()` + `to_ns()` 正确换算
- [x] C47 T4 新增：`PtpTime::add_ns()` 正向（500ms 进位到 seconds）
- [x] C48 T5 新增：`PtpTime::add_ns()` 负向（-500ms 借位到 seconds=0）
- [x] C49 T6 新增：`PtpTime::diff_ns()` 正向（self > other）
- [x] C50 T7 新增：`PtpTime::diff_ns()` 负向（self < other）
- [x] C51 T8 新增：`PortRole` 4 变体 Display 输出非空
- [x] C52 T9 新增：`PortState` 5 变体 Display 输出非空
- [x] C53 T10 新增：`Port::new()` 字段访问（`role == Disabled`、`state == Initializing`）
- [x] C54 T11 新增：`GptpConfig::default()` 字段验证（`priority1 = 128` / `priority2 = 0` / `ports = []`）
- [x] C55 T12 新增：`GptpClock::new()` 初始状态（`steps_removed = 0` / `offset = 0` / `grandmaster = identity` / `frequency_offset = 0` / `last_jump_ns = None`）
- [x] C56 T13 新增：`GptpClock::current_time()` 返回 `current_time + offset`（offset=0 时等于 initial_time）
- [x] C57 T14 新增：`GptpClock::compute_offset()` 初始返回 0
- [x] C58 T15 新增：`adjust_clock(500_000)` 小偏移 → `frequency_offset = 5_000`、`current_time` 不变、`last_jump_ns = None`
- [x] C59 T16 新增：`adjust_clock(5_000_000)` 大偏移 → `current_time` 增加 5ms、`last_jump_ns = Some(5_000_000)`
- [x] C60 T17 新增：`to_announce()` 构造的 Announce 字段反映 self 状态
- [x] C61 T18 新增：`AnnounceMessage` 构造与字段访问
- [x] C62 T19 新增：`compare_priority` — `priority1` 小者优先
- [x] C63 T20 新增：`compare_priority` — `priority1` 平局，`clock_class` 小者优先
- [x] C64 T21 新增：`compare_priority` — 全平局，`grandmaster_identity` 字节数组小者优先
- [x] C65 T22 新增：`run_bmca(&[])` 空候选列表 → `ElectedAsMaster`
- [x] C66 T23 新增：`run_bmca` 远端 `priority1 = 100`（自身 200） → `FollowMaster(remote_identity)` + `steps_removed` 增 1
- [x] C67 T24 新增：`run_bmca` 远端 `priority1 = 255`（自身 128） → `ElectedAsMaster`
- [x] C68 T25 新增：`handle_sync` 偏移计算（`origin=0,0` / `rx=1s,0` / `delay=100ms` → `new_offset = 900ms`，滤波后 `offset = (0*7 + 900_000_000) / 8 = 112_500_000`）

## Cargo.toml — 新 crate
- [x] C69 包名 `eneros-tsn-time`，version `0.79.0`，edition `2021`
- [x] C70 `#![cfg_attr(not(test), no_std)]`（在 lib.rs 中）
- [x] C71 无外部依赖（`[dependencies]` 为空或不存在）

## 配置文件
- [x] C72 `configs/gptp.toml` 存在
- [x] C73 含字段 `priority1 = 128` / `priority2 = 0` / `sync_interval_ms = 125` / `[[ports]]` 数组

## 设计文档
- [x] C74 `docs/protocols/gptp-sync-design.md` 存在
- [x] C75 12 章节完整（版本目标 / 前置依赖 / 交付物 / 数据结构 / 接口 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险 / 偏差声明）
- [x] C76 2 Mermaid 图（gPTP 主从同步时序图 + BMCA 选举决策流程图）
- [x] C77 D1~D14 偏差声明表
- [x] C78 文档在 `docs/protocols/` 下（非蓝图 `docs/phase2/`，D2）

## workspace members + 版本同步
- [x] C79 根 `Cargo.toml` `[workspace] members` 含 `"crates/protocols/tsn-time"`
- [x] C80 根 `Cargo.toml` 顶层 `version = "0.79.0"`
- [x] C81 `Makefile` 版本号 `0.79.0`（header 注释 + VERSION 变量）
- [x] C82 `.github/workflows/ci.yml` 版本号 `0.79.0`
- [x] C83 `ci/src/gate.rs` clippy 段注释含 `eneros-tsn-time v0.79.0` 与类型列表
- [x] C84 `ci/src/gate.rs` test 段注释同上

## 构建校验（§2.4.2 C6~C11）
- [x] C85 `cargo metadata --format-version 1` 成功
- [x] C86 `cargo test -p eneros-tsn-time` 全部通过（T1~T25 + 1 doctest）
- [x] C87 `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C88 `cargo fmt -p eneros-tsn-time -- --check` 通过
- [x] C89 `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning
- [x] C90 `cargo deny check licenses bans sources` 通过
- [x] C91 回归 — v0.75.0~v0.78.0 现有测试仍全绿（63 tests + 1 doctest 通过，无回归）

## no_std 合规
- [x] C92 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C93 无 `panic!` / `todo!` / `unimplemented!`
- [x] C94 子模块（clock.rs / port.rs / bmca.rs / gptp.rs）不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] C95 无 `std::collections::HashMap`
- [x] C96 无 `Send + Sync` bound
- [x] C97 无 `log` crate 依赖（D6）
- [x] C98 无 `uuid` crate 依赖（D13：ClockIdentity(pub [u8; 8])）
- [x] C99 无 `serde` / `serde_json` crate 依赖
- [x] C100 无 `PtpTime::now()` 全局函数（D7）
- [x] C101 无 `warn!` / `info!` / `error!` 宏调用（D6）
- [x] C102 无 `unsafe` 块

## 目录规范
- [x] C103 新 crate 在 `crates/protocols/tsn-time/`（D1）
- [x] C104 文档在 `docs/protocols/` 下（D2）
- [x] C105 配置在 `configs/` 下（D3）
- [x] C106 无根目录 crate（除 `ci/`）
- [x] C107 无垃圾文件（target/ / *.elf / *.bin / IDE 缓存）

## 简化设计验证（Karpathy 原则）
- [x] C108 无真实网络 I/O 代码（D5：消息通过参数注入）
- [x] C109 无 `log` crate 依赖（D6：用 `last_jump_ns` 字段替代 `warn!`）
- [x] C110 无 `PtpTime::now()` 系统时钟访问（D7：`initial_time` 参数注入）
- [x] C111 无实际 `SO_TIMESTAMPING` socket 集成（D8：仅 `hw_timestamping: bool` 标志位）
- [x] C112 修复蓝图 `handle_sync` bug（D9：`delay_ns` 参数化，使偏移计算物理意义正确）
- [x] C113 无性能基准测试代码（D10）
- [x] C114 无 24h 漂移测试（D11）
- [x] C115 无双机集成测试（D12）
- [x] C116 `ClockIdentity(pub [u8; 8])` newtype（D13，固定 8 字节数组）
- [x] C117 `MacAddr(pub [u8; 6])` newtype（D14，固定 6 字节数组）

## 破坏性变更
- [x] C118 无破坏性变更（纯新增 crate；v0.75.0~v0.78.0 类型签名不变；默认 feature 不变）
