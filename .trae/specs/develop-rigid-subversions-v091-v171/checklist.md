# Checklist — 刚性子版本 v0.9.1 / v0.12.1 / v0.12.2 / v0.17.1

> **使用方式**：每完成一项检查后勾选。所有检查项必须通过方可提交。

---

## v0.9.1 — 横向隔离合规

- [x] **C1.1** `crates/kernel/mm/src/isolation/mod.rs` 存在并定义 `ComplianceResult` / `IsolationEvidence` / `BomImpact`
- [x] **C1.2** `crates/kernel/mm/src/isolation/compliance.rs` 实现 `verify_horizontal_isolation()` / `collect_isolation_evidence()`
- [x] **C1.3** `crates/kernel/mm/src/isolation/audit.rs` 实现 `generate_compliance_report()` / `writeback_bom()`
- [x] **C1.4** `crates/kernel/mm/src/lib.rs` 已注册 `pub mod isolation;`
- [x] **C1.5** `crates/kernel/mm/Cargo.toml` 添加 `heapless` 依赖，版本号 0.9.1
- [x] **C1.6** `configs/compliance/isolation-policy.toml` 存在并包含双分区策略
- [x] **C1.7** `docs/kernel/horizontal-isolation-compliance.md` 存在并包含 Go/No-Go 结论 + 签字栏
- [x] **C1.8** `verify_horizontal_isolation` 在 mock 证据下分别返回 Go / NoGo（单元测试覆盖）
- [x] **C1.9** `writeback_bom` 在 NoGo 下正确更新成本（单元测试覆盖）
- [x] **C1.10** 证据采集可复现（相同输入 → 相同输出，单元测试覆盖）
- [x] **C1.11** `cargo build -p eneros-mm` 通过
- [x] **C1.12** `cargo test -p eneros-mm` 全部通过（58 passed, 0 failed）

## v0.12.1 — 北斗授时

- [x] **C2.1** `crates/drivers/time/src/beidou/mod.rs` 存在并定义 `TimeStamp` / `FixQuality` / `SyncError` / `BeidouState`
- [x] **C2.2** `crates/drivers/time/src/beidou/nmea.rs` 实现 `parse_nmea()`，支持 `$GNZDA` + `$GPRMC`
- [x] **C2.3** `crates/drivers/time/src/beidou/pps.rs` 实现 `on_pps_pulse()` / `discipline_clock()`
- [x] **C2.4** `crates/drivers/time/src/lib.rs` 已注册 `pub mod beidou;`
- [x] **C2.5** `configs/time/beidou.toml` 存在并包含 UART 波特率、1PPS 引脚、闰秒表
- [x] **C2.6** `docs/drivers/beidou-time-sync-design.md` 存在并包含 1PPS+NMEA 配对原理
- [x] **C2.7** NMEA 正常报文解析正确（单元测试覆盖）
- [x] **C2.8** NMEA 异常报文（校验和错、截断、非法字段）返回 `Err(SyncError::ParseError)`，不 panic
- [x] **C2.9** 闰秒边界处理正确（单元测试覆盖）
- [x] **C2.10** PPS 配对与钟差计算正确（单元测试覆盖）
- [x] **C2.11** `cargo build -p eneros-time` 通过
- [x] **C2.12** `cargo test -p eneros-time` 全部通过（79 passed, 0 failed）

## v0.12.2 — 守时与时钟冗余

- [x] **C3.1** `crates/drivers/time/src/holdover/mod.rs` 存在并定义 `HoldoverStatus` / `ClockSource` / `HoldoverQuality` / `ClockPriority`
- [x] **C3.2** `crates/drivers/time/src/holdover/ocxo.rs` 实现 `extrapolate_time()`，定义 `OcxoModel`
- [x] **C3.3** `crates/drivers/time/src/redundancy.rs` 实现 `evaluate_sources()` / `switch_clock_source()`
- [x] **C3.4** `crates/drivers/time/src/lib.rs` 已注册 `pub mod holdover;` + `pub mod redundancy;`
- [x] **C3.5** `crates/drivers/time/Cargo.toml` 版本号 0.12.2
- [x] **C3.6** `configs/time/holdover.toml` 存在并包含 OCXO 漂移参数、切换阈值
- [x] **C3.7** `docs/drivers/holdover-redundancy-design.md` 存在并包含三源冗余架构
- [x] **C3.8** OCXO 24h 漂移推算 < 1ms（单元测试覆盖）
- [x] **C3.9** 健康度评分算法正确（单元测试覆盖）
- [x] **C3.10** 三源自动切换无时钟跳变（单元测试覆盖）
- [x] **C3.11** RTC-only 降级模式时标单调递增（单元测试覆盖）
- [x] **C3.12** `cargo build -p eneros-time` 通过
- [x] **C3.13** `cargo test -p eneros-time` 全部通过（117 passed, 0 failed）

## v0.17.1 — Edge Box 电源管理

- [x] **C4.1** `crates/drivers/power/Cargo.toml` 存在，包名 `eneros-power`，版本 0.17.1，no_std
- [x] **C4.2** `crates/drivers/power/src/lib.rs` 存在并定义 `PowerDownSequence` / `ShutdownStage` / `PowerEvent` / `PowerState`
- [x] **C4.3** `crates/drivers/power/src/detect.rs` 实现 `register_power_irq()`
- [x] **C4.4** `crates/drivers/power/src/sequence.rs` 实现 `advance_sequence()` / `emergency_checkpoint()`
- [x] **C4.5** 根 `Cargo.toml` workspace members 已添加 `"crates/drivers/power"`
- [x] **C4.6** `configs/power/sequence.toml` 存在并包含 ride-through 预算、各阶段超时
- [x] **C4.7** `docs/drivers/edge-box-power-design.md` 存在并包含关机序列状态机
- [x] **C4.8** 关机序列状态机所有转换路径正确（单元测试覆盖）
- [x] **C4.9** ride-through 预算计算正确（单元测试覆盖）
- [x] **C4.10** 主电恢复时取消关机序列（单元测试覆盖）
- [x] **C4.11** 普通任务取消关机被拒绝（单元测试覆盖）
- [x] **C4.12** `cargo build -p eneros-power` 通过
- [x] **C4.13** `cargo test -p eneros-power` 全部通过（27 passed, 0 failed）

## 集成校验（§2.4 强制）

### 目录结构校验

- [x] **C5.1** 新 crate（power）在 `crates/drivers/` 下，未直接放根目录（C1）
- [x] **C5.2** 根 `Cargo.toml` members 已添加新 crate 路径（C2）
- [x] **C5.3** 跨 crate path 引用使用正确相对路径（C3）
- [x] **C5.4** 新文档在 `docs/<topic>/` 子目录，未平面化（C4/C12）
- [x] **C5.5** 无根目录 crate（除 ci/）（C5）

### 构建校验

- [x] **C5.6** `cargo metadata --format-version 1 > /dev/null` 成功（C6）
- [x] **C5.7** `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（C7）
- [x] **C5.8** `cargo build -p eneros-mm --target aarch64-unknown-none` 通过（C8）
- [x] **C5.9** `cargo build -p eneros-time --target aarch64-unknown-none` 通过（C8）
- [x] **C5.10** `cargo build -p eneros-power --target aarch64-unknown-none` 通过（C8）
- [x] **C5.11** `cargo fmt --all -- --check` 通过（C9）
- [x] **C5.12** `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 无 warning（C10）
- [x] **C5.13** `cargo deny check advisories licenses bans sources` — 本地 cargo-deny 未安装（degraded 模式），CI 中自动安装并执行（C11）

### 文档与规范校验

- [x] **C5.14** 新文档在 `docs/<topic>/` 下（C12）
- [x] **C5.15** `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪（C13）
- [x] **C5.16** `.gitignore` 覆盖新产生的文件类型（C14）
- [ ] **C5.17** 提交信息遵循 Conventional Commits（C15）— 待提交时验证

### no_std 合规（§4.3）

- [x] **C5.18** 所有新增 Rust 代码 `#![cfg_attr(not(test), no_std)]`
- [x] **C5.19** 无 `use std::*`，使用 `alloc::*` / `core::*` / `heapless::*` / `spin::*`
- [x] **C5.20** aarch64 硬件相关代码用 `#[cfg(target_arch = "aarch64")]` 门控

### ADR 合规（§5.4）

- [x] **C5.21** v0.9.1 形成 Go/No-Go 书面结论（ADR-0003 合规闸门）
- [x] **C5.22** v0.12.1 仅北斗不依赖 GPS（自主可控）
- [x] **C5.23** v0.17.1 关机序列不可被普通任务取消（安全性）
