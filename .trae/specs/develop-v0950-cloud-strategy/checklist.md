# Checklist — v0.95.0 Cloud Coordinator 基础：策略下发

> Spec：`spec.md`（develop-v0950-cloud-strategy）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新 crate 位于 `crates/agents/cloud-coordinator/`，未直接放根目录（agents 子系统归属正确）
- [x] C2: 根 `Cargo.toml` workspace members 已追加 `"crates/agents/cloud-coordinator"`，`cargo metadata` 可解析
- [x] C3: 新 crate `Cargo.toml` path 引用为正确相对路径（`../energy-market-agent` / `../coordinator`）
- [x] C4: 新文档 `cloud-strategy-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-cloud-coordinator` 40 通过
- [x] C8: `cargo build -p eneros-cloud-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-cloud-coordinator --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）（advisories 因 github.com 网络不可达跳过 DB 更新，零新增依赖，供应链面同 v0.94.0）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`）

## D. 数据结构（C16~C23）

- [x] C16: `Strategy` 6 字段（`strategy_id: u64` / `version: u32` / `targets: Vec<u64>` / `content: StrategyContent` / `deadline: u64` / `priority: Priority`）派生 Debug/Clone/PartialEq
- [x] C17: `StrategyContent` 4 变体（`OptimizationWeights(BTreeMap<Objective, f32>)` / `PriceForecast(Vec<PricePoint>)` / `DrResponse(DrSignal)` / `ModelUpdate(ModelRef)`）派生 Debug/Clone/PartialEq
- [x] C18: `ModelRef { model_id: u64, version: u32 }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default
- [x] C19: `EdgeAck { strategy_id: u64, edge_id: u64, accepted: bool, reason: Option<RejectReason> }` 派生 Debug/Clone/Copy/PartialEq
- [x] C20: `RejectReason { SafetyWeightTooLow, ExceedsCapacity }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C21: `LocalState { edge_id: u64, max_capacity_mw: f32 }` 派生 Debug/Clone/Copy/PartialEq/Default
- [x] C22: `Objective` / `PricePoint` / `DrSignal` 复用 `eneros-energy-market-agent`，`Priority` 复用 `eneros-coordinator`（不重复定义）
- [x] C23: 常量 `SAFETY_WEIGHT_MIN: f32 = 0.5` / `DEFAULT_ACK_TIMEOUT_MS: u64 = 10_000` / `DEFAULT_MAX_RETRIES: u32 = 3` 存在且为命名常量（无硬编码散落）

## E. 边缘安全校验 validate_strategy（C24~C31）

- [x] C24: 函数签名 `pub fn validate_strategy(strategy: &Strategy, local_state: &LocalState) -> Result<(), RejectReason>`
- [x] C25: `OptimizationWeights` safety weight ≥ 0.5 → `Ok(())`
- [x] C26: safety weight < 0.5 → `Err(SafetyWeightTooLow)`
- [x] C27: safety weight 缺失或 NaN → 按 0.0 → `Err(SafetyWeightTooLow)`（D10/D12 安全侧默认拒绝）
- [x] C28: `DrResponse` `target_mw.abs() ≤ max_capacity_mw` 且均有限 → `Ok(())`
- [x] C29: `DrResponse` `target_mw` 非有限或 `abs() > max_capacity_mw` → `Err(ExceedsCapacity)`
- [x] C30: `max_capacity_mw` 非有限或 ≤ 0 → 一切 DR 策略 `Err(ExceedsCapacity)`（D12）
- [x] C31: `PriceForecast` / `ModelUpdate` 恒 `Ok(())`

## F. 云边通道（C32~C38）

- [x] C32: `CloudError { BroadcastFailed }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C33: `CloudChannel` 为 sync trait：`broadcast(&mut self, &Strategy) -> Result<(), CloudError>` + `collect_acks(&mut self, u64, u64) -> Vec<EdgeAck>`，无 async、无 Send+Sync 约束（D3/D8）
- [x] C34: `MockCloudChannel` 配置 `fail_times = N` → 前 N 次 broadcast `Err(BroadcastFailed)`，第 N+1 次起 `Ok`
- [x] C35: Mock broadcast 成功时策略克隆入 `sent` 已发记录
- [x] C36: Mock collect_acks 从预置 acks 过滤 `strategy_id` 返回（不匹配 id 不返回）
- [x] C37: Mock 无预置 acks → collect_acks 返回空 Vec
- [x] C38: Mock 预置 acks 不被 collect 消耗语义明确（重复 collect 行为确定）

## G. 策略发布器 StrategyPublisher（C39~C50）

- [x] C39: 字段全 pub：`channel: Box<dyn CloudChannel>` / `max_retries: u32` / `published_count` / `retry_count` / `ack_count` / `reject_count` / `pending: Vec<Strategy>`（D9）
- [x] C40: `new(channel)` → max_retries = 3（DEFAULT_MAX_RETRIES），4 计数器全零，pending 空
- [x] C41: publish 首次成功 → `Ok`、`published_count == 1`、`retry_count == 0`、pending 空
- [x] C42: channel 前 1 次失败 → 第 2 次成功：`Ok`、`published_count == 1`、`retry_count == 1`（每次失败 retry_count += 1）
- [x] C43: channel 恒失败（max_retries=3）→ `Err(BroadcastFailed)`、`retry_count == 3`、策略克隆入 pending（len=1）
- [x] C44: 失败后 channel 恢复 → `republish_pending()` 返回成功补发数、pending 清空、`published_count` 相应增加
- [x] C45: 补发部分失败 → 成功条移除、失败条保留 pending，返回成功数
- [x] C46: 补发每条仍限 max_retries（重试计数正确累加）
- [x] C47: `collect_acks` 委托 channel 并透传 timeout_ms
- [x] C48: collect_acks 返回 3 条（2 accepted / 1 rejected）→ `ack_count += 2`、`reject_count += 1`
- [x] C49: 空 ack → 计数器不变
- [x] C50: 断网补发集成场景：publish 失败入 pending → 恢复后 republish → 再 collect_acks 全链路计数一致

## H. crate 集成（C51~C54）

- [x] C51: `lib.rs` 含 `pub mod strategy; pub mod channel; pub mod publisher;`
- [x] C52: `lib.rs` 重导出 11 项：Strategy / StrategyContent / ModelRef / EdgeAck / RejectReason / LocalState / validate_strategy / CloudChannel / MockCloudChannel / CloudError / StrategyPublisher（+ 3 常量）
- [x] C53: `lib.rs` crate 文档含 v0.95.0 说明 + D1~D12 偏差简表
- [x] C54: crate `Cargo.toml` description 含 v0.95.0；依赖仅 2 个既有 path crate，无新第三方依赖

## I. 配置文件（C55~C57）

- [x] C55: `configs/cloud_coordinator.toml` 存在，`[cloud_coordinator]` 段含 `ack_timeout_ms = 10000` / `max_retries = 3` / `safety_weight_min = 0.5` / `endpoint` 占位
- [x] C56: 中文注释覆盖 6 点：下发延迟 <1s / 策略非强制边缘可拒绝 / 断网重连补发 / 策略版本化 / NaN 防御 / 新策略类型可扩展
- [x] C57: 配置值与代码常量一致（10000 / 3 / 0.5）

## J. 设计文档（C58~C61）

- [x] C58: `docs/agents/cloud-strategy-design.md` 存在，12 章节齐全
- [x] C59: 含 2 个 Mermaid 图（云边策略下发数据流图 + publish/validate/ack 决策流程图含重试/拒绝/补发分支）
- [x] C60: 含 D1~D12 偏差表，与 spec 偏差声明一致
- [x] C61: 接口契约与实现一致（函数签名、字段、错误变体、计数器语义）

## K. 版本同步（C62~C65）

- [x] C62: 根 `Cargo.toml` `[workspace.package] version = "0.95.0"`
- [x] C63: `Makefile` 版本注释同步 0.95.0
- [x] C64: `.github/workflows/ci.yml` 版本注释同步 0.95.0
- [x] C65: `ci/src/gate.rs` 注释追加 v0.95.0 类型清单（11 项）

## L. 测试覆盖（C66~C71）

- [x] C66: 内嵌 40 个单元测试（T1~T40）全部实现并通过
- [x] C67: 测试分布：数据结构 T1~T6 / validate T7~T14 / channel T15~T22 / publisher T23~T36 / 集成+NaN T37~T40
- [x] C68: 含 publish + validate + ack 全链路集成测试
- [x] C69: 含断网补发故障注入测试（Mock fail_times + republish_pending）
- [x] C70: 含 NaN 风暴防御测试（weight NaN / DR target NaN / capacity NaN 与 ≤0）
- [x] C71: 回归零破坏：eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

## M. 蓝图达成（C72~C75）

- [x] C72: v0.95.0 交付物全覆盖：策略数据结构 / 云端下发（publish+重试）/ 边缘安全校验（validate）/ Ack 回收与拒绝可观测 / 断网补发
- [x] C73: 策略非强制、边缘主权保留：安全/容量违规可拒绝（蓝图 §5.2/§9）
- [x] C74: 无 BREAKING：既有全部 crate 零改动，既有公共 API 全保留
- [x] C75: 下游解锁：v0.96.0 数据汇聚（P2-D 收尾）/ v0.112.0 云端孪生主节点
