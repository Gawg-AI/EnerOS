# Tasks — v0.95.0 Cloud Coordinator 基础：策略下发

> Spec：`spec.md`（develop-v0950-cloud-strategy）。蓝图：`蓝图/phase2.md` v0.95.0。
> 全部 no_std + alloc 合规；新 crate `eneros-cloud-coordinator`；既有 crate 零改动。

- [x] Task 1: 新建 crate 骨架 `crates/agents/cloud-coordinator/`
  - [ ] SubTask 1.1: `Cargo.toml` — package `eneros-cloud-coordinator`，`description` 含 v0.95.0；依赖仅 2 个 path crate：`eneros-energy-market-agent = { path = "../energy-market-agent" }`、`eneros-coordinator = { path = "../coordinator" }`；无新第三方依赖
  - [ ] SubTask 1.2: `src/lib.rs` — `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；crate 文档含 v0.95.0 说明 + D1~D12 偏差表；`pub mod strategy; pub mod channel; pub mod publisher;` + 全部重导出
  - [ ] SubTask 1.3: 根 `Cargo.toml` `members` 追加 `"crates/agents/cloud-coordinator"`（既有成员零改动）
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 实现 `src/strategy.rs` — 策略数据结构与边缘安全校验
  - [ ] SubTask 2.1: `Strategy`（6 字段，u64 标识，D2）/ `StrategyContent`（4 变体，D4）/ `ModelRef` / `EdgeAck`（D7）/ `RejectReason`（2 变体）/ `LocalState`，派生宏按 spec 要求
  - [ ] SubTask 2.2: 常量 `SAFETY_WEIGHT_MIN: f32 = 0.5`（D10）/ `DEFAULT_ACK_TIMEOUT_MS: u64 = 10_000`（D3）/ `DEFAULT_MAX_RETRIES: u32 = 3`
  - [ ] SubTask 2.3: `validate_strategy(strategy, local_state) -> Result<(), RejectReason>` — safety weight 缺失/NaN 按 0.0 拒绝；DR target_mw 非有限或超容量拒绝；capacity 非有限或 ≤0 → 一切 DR 拒绝（D12）；PriceForecast/ModelUpdate 恒 Ok
  - [ ] SubTask 2.4: 内嵌测试 T1~T14（数据结构派生 + 校验通过/拒绝/边界/NaN 防御）
  - 验证：`cargo test -p eneros-cloud-coordinator strategy` 通过

- [x] Task 3: 实现 `src/channel.rs` — 云边通道抽象
  - [ ] SubTask 3.1: `CloudError { BroadcastFailed }`（Debug/Clone/Copy/PartialEq/Eq）
  - [ ] SubTask 3.2: sync trait `CloudChannel { broadcast(&mut self, &Strategy) -> Result<(), CloudError>; collect_acks(&mut self, strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck> }`（D3/D8，无 Send+Sync 要求）
  - [ ] SubTask 3.3: `MockCloudChannel` — `fail_times` 前 N 次 broadcast 失败（故障注入）、`sent: Vec<Strategy>` 已发记录、预置 acks 按 strategy_id 过滤返回
  - [ ] SubTask 3.4: 内嵌测试 T15~T22（失败计数耗尽后成功、已发记录、ack 过滤、空 ack）
  - 验证：`cargo test -p eneros-cloud-coordinator channel` 通过

- [x] Task 4: 实现 `src/publisher.rs` — 策略发布器（重试 + 补发 + 可观测）
  - [ ] SubTask 4.1: `StrategyPublisher` 全 pub 字段（channel/max_retries/4 计数器/pending，D9）；`new(channel)` 默认 max_retries=3、计数器全零
  - [ ] SubTask 4.2: `publish()` — 至多 max_retries 次（每次失败 retry_count+=1）；成功 published_count+=1；耗尽 → 克隆入 pending + Err(BroadcastFailed)
  - [ ] SubTask 4.3: `republish_pending() -> u32` — 逐条重发（每条仍限 max_retries），成功补发数返回，成功移除失败保留
  - [ ] SubTask 4.4: `collect_acks()` — 委托 channel，ack_count += accepted 数、reject_count += rejected 数
  - [ ] SubTask 4.5: 内嵌测试 T23~T40（重试成功/耗尽入 pending/补发清空/补发部分失败保留/ack 计数/断网补发故障注入集成/NaN 风暴/publish+validate+ack 全链路）
  - 验证：`cargo test -p eneros-cloud-coordinator` 40 测试全通过

- [x] Task 5: 新增 `configs/cloud_coordinator.toml`
  - [ ] SubTask 5.1: `[cloud_coordinator]` 段：`ack_timeout_ms = 10000` / `max_retries = 3` / `safety_weight_min = 0.5` / `endpoint` 占位
  - [ ] SubTask 5.2: 中文注释 6 点（下发延迟 <1s / 策略非强制边缘可拒绝 / 断网重连补发 / 策略版本化 / NaN 防御 / 新策略类型可扩展）

- [x] Task 6: 新增 `docs/agents/cloud-strategy-design.md`
  - [ ] SubTask 6.1: 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
  - [ ] SubTask 6.2: 2 个 Mermaid 图（云边策略下发数据流图 + publish/validate/ack 决策流程图含重试/拒绝/补发分支）
  - [ ] SubTask 6.3: D1~D12 偏差表与 spec 一致；接口契约与实现签名一致

- [x] Task 7: 根目录版本同步 0.94.0 → 0.95.0
  - [ ] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.95.0"`
  - [ ] SubTask 7.2: `Makefile` 版本注释同步
  - [ ] SubTask 7.3: `.github/workflows/ci.yml` 版本注释同步
  - [ ] SubTask 7.4: `ci/src/gate.rs` 注释串尾追加 v0.95.0 类型清单（Strategy/StrategyContent/ModelRef/EdgeAck/RejectReason/LocalState/CloudChannel/MockCloudChannel/CloudError/StrategyPublisher/validate_strategy）

- [x] Task 8: 构建验证（§2.4.2 全量）
  - [ ] SubTask 8.1: `cargo metadata --format-version 1` 成功
  - [ ] SubTask 8.2: `cargo test -p eneros-cloud-coordinator` 40 通过
  - [ ] SubTask 8.3: `cargo build -p eneros-cloud-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [ ] SubTask 8.4: `cargo fmt --all -- --check` 通过
  - [ ] SubTask 8.5: `cargo clippy -p eneros-cloud-coordinator --all-targets -- -D warnings` 0 warning
  - [ ] SubTask 8.6: `cargo deny check advisories licenses bans sources`（零新增依赖）
  - [ ] SubTask 8.7: 回归零破坏：eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

- [x] Task 9: 按 `checklist.md` 逐项核验并勾选（未通过禁止收工）

# Task Dependencies

- Task 2/3 依赖 Task 1（crate 骨架）
- Task 4 依赖 Task 2 + Task 3（strategy/channel 类型）
- Task 5/6/7 与 Task 2~4 可并行（文档/配置/版本同步）
- Task 8 依赖 Task 1~7 全部完成
- Task 9 依赖 Task 8 通过
