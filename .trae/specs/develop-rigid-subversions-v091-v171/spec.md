# 刚性子版本 v0.9.1 / v0.12.1 / v0.12.2 / v0.17.1 开发规范

> **覆盖版本**：v0.9.1（横向隔离合规）、v0.12.1（北斗授时）、v0.12.2（守时冗余）、v0.17.1（Edge Box 电源管理）
> **蓝图依据**：`蓝图/phase0.md` 各版本章节 + `蓝图/appendix.md` + ADR-0003（合规闸门）
> **开发模式**：4 个刚性子版本集中回填，v0.9.1 / v0.12.1 / v0.17.1 可并行，v0.12.2 依赖 v0.12.1

---

## Why

Phase 0 主路径已推进至 v0.18.0，但 4 个刚性子版本（v0.9.1 / v0.12.1 / v0.12.2 / v0.17.1）尚未实现。这 4 个版本是 Phase 0 出口的硬性门槛：

- **v0.9.1** 是 ADR-0003 合规 Go/No-Go 闸门，未完成则 Phase 0 不得出口进入 Phase 1
- **v0.12.1 + v0.12.2** 构成"主授时 + 守时"冗余对，是 SOE/录波/TSN 时间基准的前置
- **v0.17.1** 是 v0.24.0 文件系统与 Phase 1 checkpoint 的掉电安全底座

集中回填这 4 个版本以解除 Phase 0 出口阻塞。

---

## What Changes

### v0.9.1 — 横向隔离合规路径
- **新增** `crates/kernel/mm/src/isolation/mod.rs`：隔离合规模块入口
- **新增** `crates/kernel/mm/src/isolation/compliance.rs`：合规路径验证逻辑（Go/No-Go 判定）
- **新增** `crates/kernel/mm/src/isolation/audit.rs`：隔离证据采集与报告生成
- **新增** `configs/compliance/isolation-policy.toml`：双分区策略配置
- **新增** `docs/kernel/horizontal-isolation-compliance.md`：合规路径书面结论
- **修改** `crates/kernel/mm/src/lib.rs`：注册 `pub mod isolation;`
- **修改** `crates/kernel/mm/Cargo.toml`：添加 `heapless` 依赖（固定容量容器）

### v0.12.1 — 北斗授时
- **新增** `crates/drivers/time/src/beidou/mod.rs`：北斗驱动入口
- **新增** `crates/drivers/time/src/beidou/nmea.rs`：NMEA 0183 报文解析
- **新增** `crates/drivers/time/src/beidou/pps.rs`：1PPS 中断处理与时钟 disciplining
- **新增** `configs/time/beidou.toml`：串口波特率、1PPS 引脚、闰秒表
- **新增** `docs/drivers/beidou-time-sync-design.md`：北斗授时设计
- **修改** `crates/drivers/time/src/lib.rs`：注册 `pub mod beidou;`

### v0.12.2 — 守时与时钟冗余
- **新增** `crates/drivers/time/src/holdover/mod.rs`：守时状态机
- **新增** `crates/drivers/time/src/holdover/ocxo.rs`：OCXO 频率补偿模型
- **新增** `crates/drivers/time/src/redundancy.rs`：三源故障切换（北斗/OCXO/RTC）
- **新增** `configs/time/holdover.toml`：OCXO 漂移参数、切换阈值
- **新增** `docs/drivers/holdover-redundancy-design.md`：守时与冗余设计
- **修改** `crates/drivers/time/src/lib.rs`：注册 `pub mod holdover;` + `pub mod redundancy;`

### v0.17.1 — Edge Box 电源管理
- **新增** `crates/drivers/power/Cargo.toml`：新 crate 配置
- **新增** `crates/drivers/power/src/lib.rs`：电源管理入口
- **新增** `crates/drivers/power/src/detect.rs`：掉电检测与中断处理
- **新增** `crates/drivers/power/src/sequence.rs`：关机序列状态机
- **新增** `configs/power/sequence.toml`：ride-through 预算、各阶段超时
- **新增** `docs/drivers/edge-box-power-design.md`：电源管理设计
- **修改** 根 `Cargo.toml`：workspace members 添加 `crates/drivers/power`

### 集成变更（4 版本共用）
- **修改** `Cargo.toml`：workspace 版本号提升策略（各子版本独立版本号，workspace 维持 0.18.0）
- **修改** `Makefile`：VERSION 注释更新
- **修改** `.github/workflows/ci.yml`：交叉编译覆盖新 power crate
- **修改** `ci/src/gate.rs`：注释更新覆盖 4 个子版本

---

## Impact

- **Affected specs**：
  - `develop-v090-partition-isolation`（v0.9.1 基于其 Partition 结构）
  - `develop-v120-rtc-system-clock`（v0.12.1/v0.12.2 基于其 TimeStamp/MonotonicClock）
  - `develop-v170-multi-core-coherence`（v0.17.1 基于其内存一致性）
- **Affected code**：
  - `crates/kernel/mm/`：新增 isolation 子模块
  - `crates/drivers/time/`：新增 beidou + holdover + redundancy 子模块
  - `crates/drivers/power/`：全新 crate
  - `configs/`：新增 compliance/ time/ power/ 子目录
  - `docs/kernel/` + `docs/drivers/`：新增 4 篇设计文档
- **Breaking changes**：无（纯新增，不修改已有 API）

---

## ADDED Requirements

### Requirement: 横向隔离合规验证（v0.9.1）

系统 SHALL 提供基于 36 号文的双分区横向隔离合规验证能力，采集 v0.9.0 隔离证据并形成 Go/No-Go 书面结论。

#### Scenario: 双分区可接受（Go）
- **WHEN** 物理内存隔离、capability 强制、单向数据流三项以上满足
- **THEN** 返回 `ComplianceResult::Go`，生成包含证据链的合规报告

#### Scenario: 必须加物理隔离装置（No-Go）
- **WHEN** 隔离证据不足（如物理内存隔离失败）
- **THEN** 返回 `ComplianceResult::NoGo`，输出 BOM 影响与原因

#### Scenario: 证据采集可复现
- **WHEN** 监管复核时重新调用 `collect_isolation_evidence()`
- **THEN** 生成与首次一致的证据报告（相同输入 → 相同输出）

### Requirement: 北斗 GNSS 授时（v0.12.1）

系统 SHALL 集成北斗 GNSS 接收模块，通过 1PPS + NMEA 报文配对授时，将系统时钟同步至北斗 BDT，同步精度 < 100ns。

#### Scenario: 正常授时同步
- **WHEN** 北斗模块输出 1PPS 与 $GNZDA 报文，且配对成功
- **THEN** 系统单调时钟被 discipline，`beidou_sync()` 返回 `TimeStamp`

#### Scenario: NMEA 异常报文不 panic
- **WHEN** 收到校验和错、截断、非法字段报文
- **THEN** `parse_nmea()` 返回 `Err(SyncError::ParseError)`，不 panic

#### Scenario: 1PPS 超时
- **WHEN** 1PPS 中断超过预期周期未到达
- **THEN** 返回 `Err(SyncError::PpsTimeout)`，由 v0.12.2 守时兜底

### Requirement: 守时与时钟冗余（v0.12.2）

系统 SHALL 在北斗信号丢失时通过 OCXO + RTC 守时，24 小时守时漂移 < 1ms，并在北斗/OCXO/RTC 三源间自动故障切换。

#### Scenario: 北斗失锁切换至 OCXO
- **WHEN** 北斗信号丢失且 OCXO 健康
- **THEN** 自动切换至 OCXO 守时，时钟不回退，输出 `HoldoverQuality::Good`

#### Scenario: 24h 守时精度
- **WHEN** OCXO 守时 24 小时
- **THEN** 漂移 < 1ms，标记 `HoldoverQuality::Good`（< 1ms/24h）

#### Scenario: 三源全降级
- **WHEN** OCXO 也不健康，降级至 RTC
- **THEN** 时标仍单调递增，不崩溃，输出 `HoldoverQuality::Degraded`

#### Scenario: 主电恢复时钟不跳变
- **WHEN** 北斗信号恢复
- **THEN** 平滑回切至北斗，不产生时钟跳变

### Requirement: Edge Box 电源管理（v0.17.1）

系统 SHALL 实现掉电检测、UPS/超级电容 ride-through、紧急 checkpoint 刷盘、优雅关机序列，保证突发掉电时数据完整性。

#### Scenario: 掉电检测
- **WHEN** 主电源消失
- **THEN** 在 < 10ms 内 CPU 收到 GPIO 中断，进入 `ShutdownStage::Detect`

#### Scenario: ride-through 内完成 checkpoint
- **WHEN** ride-through 预算 ≥ 100ms 且 checkpoint 完成
- **THEN** 进入 `GracefulShutdown`，安全断电

#### Scenario: ride-through 超时硬断电
- **WHEN** ride-through 超时且 checkpoint 未完成
- **THEN** 进入 `HardOff`，标记未完成状态，重启后可识别

#### Scenario: 主电恢复取消关机
- **WHEN** 关机序列执行中主电恢复
- **THEN** 取消关机序列，恢复正常运行

#### Scenario: 关机序列不可被普通任务取消
- **WHEN** 普通任务尝试取消关机序列
- **THEN** 拒绝取消，关机序列继续执行

---

## 设计决策

### D1: heapless 替代 alloc::string::String 的容量语法
蓝图接口中 `String<256>` 语法非标准 Rust。采用 `heapless::String<256>` + `heapless::Vec<T, N>` 实现固定容量 no_std 容器，避免动态分配不确定性。

### D2: v0.9.1 不移动现有 partition.rs
蓝图路径 `crates/kernel/mm/src/isolation/compliance.rs` 暗示 isolation 子目录。但现有 `partition.rs` 位于 `mm/src/` 顶层且已被 v0.9.0 测试覆盖。遵循"外科手术式变更"原则，仅新建 `isolation/` 子目录存放 compliance/audit，不移动 partition.rs。

### D3: v0.12.1/v0.12.2 作为 time crate 子模块
北斗与守时模块作为 `crates/drivers/time/src/` 下的子目录（beidou/、holdover/），复用 time crate 的 TimeStamp/MonotonicClock，不新建独立 crate。

### D4: v0.17.1 power 作为独立 crate
电源管理是独立子系统，且未来可能扩展（UPS 监控、电池管理），作为 `crates/drivers/power/` 独立 crate，遵循 §2.3.1 crate 分组规则。

### D5: 硬件相关代码 cfg-gated
所有 aarch64 MMIO/中断/寄存器访问代码用 `#[cfg(target_arch = "aarch64")]` 门控，host 测试侧提供 mock 实现，保证 `cargo test --workspace` 通过。

### D6: 版本号策略
4 个子版本作为独立交付，各 crate 的 `Cargo.toml` 版本号单独设置（mm 0.9.1、time 0.12.2、power 0.17.1），workspace 版本号维持 0.18.0（因主路径已超前）。

### D7: 合规结论需人工签字
v0.9.1 的 Go/No-Go 最终结论依赖监管沟通与架构负责人签字，代码只提供证据采集与报告生成，结论判定为半自动（AI 可编码性=2）。
