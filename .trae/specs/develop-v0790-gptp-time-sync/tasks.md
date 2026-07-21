# Tasks

- [x] Task 1: 新建 crate 骨架 — `crates/protocols/tsn-time/`
  - [x] SubTask 1.1: 创建 `crates/protocols/tsn-time/Cargo.toml`（包名 `eneros-tsn-time`，version `0.79.0`，edition `2021`，无外部依赖）
  - [x] SubTask 1.2: 创建 `crates/protocols/tsn-time/src/lib.rs` 骨架（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 4 个 `pub mod` 声明：`bmca` / `clock` / `gptp` / `port` + `#[cfg(test)] mod tests` + 顶部模块文档注释 + D1~D14 偏差声明表）

- [x] Task 2: 实现 `clock.rs` — `ClockIdentity` / `MacAddr` / `PtpTime`
  - [x] SubTask 2.1: `ClockIdentity(pub [u8; 8])` newtype（D13），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`；实现 `Display`（冒号分隔十六进制）+ `new()`
  - [x] SubTask 2.2: `MacAddr(pub [u8; 6])` newtype（D14），派生 `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`；实现 `Display` + `new()`
  - [x] SubTask 2.3: `PtpTime { seconds: u64, nanos: u32 }` 结构体 + `new()` / `to_ns() -> i128` / `add_ns(i64)` / `diff_ns(&PtpTime) -> i64`

- [x] Task 3: 实现 `port.rs` — `PortRole` / `PortState` / `Port`
  - [x] SubTask 3.1: `PortRole` 枚举（`Master` / `Slave` / `Passive` / `Disabled`）+ `Display`
  - [x] SubTask 3.2: `PortState` 枚举（`Initializing` / `Listening` / `Master` / `Slave` / `Passive`）+ `Display`
  - [x] SubTask 3.3: `Port` 结构体 + `Port::new()`（`role = Disabled`、`state = Initializing`）

- [x] Task 4: 实现 `bmca.rs` — `AnnounceMessage` / `BmcaResult` / `compare_priority`
  - [x] SubTask 4.1: `AnnounceMessage` 结构体（8 字段）派生 `Debug, Clone, PartialEq, Eq`
  - [x] SubTask 4.2: `BmcaResult` 枚举（`ElectedAsMaster` / `FollowMaster(ClockIdentity)`）+ `Display`
  - [x] SubTask 4.3: `compare_priority(a, b) -> core::cmp::Ordering`（BMCA 优先级链式比较 using `.then_with()`）

- [x] Task 5: 实现 `gptp.rs` — `SyncMessage` / `FollowUpMessage` / `GptpConfig` / `GptpClock`
  - [x] SubTask 5.1: `SyncMessage` 结构体（3 字段）派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 5.2: `FollowUpMessage` 结构体（2 字段）派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 5.3: `GptpConfig` 结构体 + `Default`（`priority1 = 128`、`priority2 = 0`、`ports = Vec::new()`）
  - [x] SubTask 5.4: `GptpClock` 结构体（13 基础字段 + `last_sync_seq_id` / `last_sync_rx_ts` / `last_sync_delay_ns` / `last_sync_origin_ts` 用于 FollowUp 配对）
  - [x] SubTask 5.5: `GptpClock::new(identity, initial_time, config)`（D7：参数注入）
  - [x] SubTask 5.6: `GptpClock::run_bmca(&mut self, announces) -> BmcaResult`
  - [x] SubTask 5.7: `GptpClock::handle_sync(&mut self, sync, rx_ts, delay_ns)`（D9：修复蓝图 bug）
  - [x] SubTask 5.8: `GptpClock::handle_follow_up(&mut self, fu)`（按 `sequence_id` 匹配重算 offset）
  - [x] SubTask 5.9: `GptpClock::adjust_clock(&mut self, offset)`（D6：小偏移频率微调，大偏移跳跃 + `last_jump_ns`）
  - [x] SubTask 5.10: `GptpClock::compute_offset() -> i64`
  - [x] SubTask 5.11: `GptpClock::current_time() -> PtpTime`
  - [x] SubTask 5.12: `GptpClock::to_announce() -> AnnounceMessage`

- [x] Task 6: 修改 `lib.rs` — 重新导出 + 偏差表 + 测试
  - [x] SubTask 6.1: `pub use bmca::{compare_priority, AnnounceMessage, BmcaResult};`
  - [x] SubTask 6.2: `pub use clock::{ClockIdentity, MacAddr, PtpTime};`
  - [x] SubTask 6.3: `pub use gptp::{FollowUpMessage, GptpClock, GptpConfig, SyncMessage};`
  - [x] SubTask 6.4: `pub use port::{Port, PortRole, PortState};`
  - [x] SubTask 6.5: T1~T25 测试全部新增并通过
  - [x] SubTask 6.6: 顶部模块文档注释描述 v0.79.0 gPTP 时间同步层
  - [x] SubTask 6.7: 完整 D1~D14 偏差声明表

- [x] Task 7: 创建配置文件 `configs/gptp.toml`
  - [x] SubTask 7.1: TOML 模板含 `priority1` / `priority2` / `sync_interval_ms` / `[[ports]]` 数组

- [x] Task 8: 创建设计文档 `docs/protocols/gptp-sync-design.md`
  - [x] SubTask 8.1: 12 章节完整
  - [x] SubTask 8.2: 2 Mermaid 图（gPTP 主从同步时序图 + BMCA 选举决策流程图）
  - [x] SubTask 8.3: D1~D14 偏差声明表

- [x] Task 9: workspace members 与版本同步
  - [x] SubTask 9.1: 根 `Cargo.toml` 在 `[workspace] members` 添加 `"crates/protocols/tsn-time"`
  - [x] SubTask 9.2: 根 `Cargo.toml` 顶层 `version` 从 `0.78.0` → `0.79.0`
  - [x] SubTask 9.3: `Makefile` 版本号 `0.78.0` → `0.79.0`（header 注释 + VERSION 变量）
  - [x] SubTask 9.4: `.github/workflows/ci.yml` 版本号 `0.78.0` → `0.79.0`
  - [x] SubTask 9.5: `ci/src/gate.rs` clippy 段 + test 段注释新增 `eneros-tsn-time v0.79.0` 含类型列表

- [x] Task 10: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 10.1: `cargo metadata --format-version 1` 成功
  - [x] SubTask 10.2: `cargo test -p eneros-tsn-time` 全部通过（T1~T25 + 1 doctest）
  - [x] SubTask 10.3: `cargo build -p eneros-tsn-time --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 10.4: `cargo fmt -p eneros-tsn-time -- --check` 通过
  - [x] SubTask 10.5: `cargo clippy -p eneros-tsn-time --all-targets -- -D warnings` 无 warning
  - [x] SubTask 10.6: `cargo deny check licenses bans sources` 通过
  - [x] SubTask 10.7: 回归 — v0.75.0~v0.78.0 现有 T1~T63 测试仍全绿（无回归）

# Task Dependencies

- Task 1（crate 骨架）必须先完成 — 后续所有 Task 依赖 crate 存在
- Task 2（clock.rs）必须先完成 — Task 3 的 `Port.mac: MacAddr` 依赖之；Task 4 的 `AnnounceMessage.grandmaster_identity: ClockIdentity` 与 `source_mac: MacAddr` 依赖之；Task 5 的 `SyncMessage.origin_timestamp: PtpTime` 依赖之
- Task 3（port.rs）必须先完成 — Task 5 的 `GptpConfig.ports: Vec<Port>` 与 `GptpClock.ports: Vec<Port>` 依赖之
- Task 4（bmca.rs）必须先完成 — Task 5 的 `GptpClock::run_bmca()` 返回 `BmcaResult` 依赖之；`to_announce()` 返回 `AnnounceMessage` 依赖之
- Task 5（gptp.rs）依赖 Task 2/3/4 完成
- Task 6（lib.rs）依赖 Task 2/3/4/5 完成
- Task 7/8（配置 + 文档）可与 Task 2~5 并行
- Task 9（版本同步）依赖 Task 1~8 完成
- Task 10（构建校验）依赖所有前置任务完成
