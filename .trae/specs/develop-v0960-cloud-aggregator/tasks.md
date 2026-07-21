# Tasks — v0.96.0 Cloud Coordinator 基础：数据汇聚

> Spec：`spec.md`（develop-v0960-cloud-aggregator）。蓝图：`蓝图/phase2.md` v0.96.0。
> 全部 no_std + alloc 合规；复用既有 crate `eneros-cloud-coordinator` 追加模块；既有 crate 零改动。

- [x] Task 1: crate 集成准备（lib.rs 追加 + 版本同步）
  - [ ] SubTask 1.1: `crates/agents/cloud-coordinator/src/lib.rs` — 追加 `pub mod aggregator;` + 重导出 10 项（DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink）；crate 文档升级为 v0.95.0 + v0.96.0 双版本说明（核心类型清单表追加 aggregator 行 + v0.96.0 D1~D12 简表）；v0.95.0 既有 strategy/channel/publisher 零改动
  - [ ] SubTask 1.2: `crates/agents/cloud-coordinator/Cargo.toml` — description 追加 v0.96.0；无新依赖（eneros-coordinator 已存在）
  - [ ] SubTask 1.3: 根 `Cargo.toml` `[workspace.package] version = "0.96.0"`（既有 members 零改动）
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 实现 `src/aggregator.rs` — 数据汇聚核心
  - [ ] SubTask 2.1: 数据结构 `DomainData` / `EventRecord` / `EventType` / `Severity`（derive 按 spec）+ `AggError`（3 变体）
  - [ ] SubTask 2.2: sync trait `DataSource { fetch(&mut self, now_ms) -> Result<EdgeBoxState, AggError> }` 与 `DataSink { store(&mut self, &DomainData, now_ms) -> Result<(), AggError> }`（无 async、无 Send+Sync）
  - [ ] SubTask 2.3: `DataAggregator`（5 字段全 pub）+ `new(sink)` / `add_source()` / `collect(now_ms)`（遍历 fetch、失败 timeout_count+=1 不中断、空源 Err(EmptySources)、metrics 含 states.len() 与总容量、collect_count+=1）/ `store(data, now_ms)`（委托 sink、计数）
  - [ ] SubTask 2.4: `MockDataSource`（fail_times 故障注入、state 返回）+ `MockDataSink`（fail_times 故障注入、stored 记录）
  - [ ] SubTask 2.5: 内嵌测试 T1~T40（数据结构派生 T1~T6 / collect 全通过/部分失败/空源 T7~T14 / store 成功/失败 T15~T20 / Mock 故障注入 T21~T26 / NaN 防御 T27~T30 / 脱敏标记 T31~T32 / 多 source 汇聚 T33~T36 / collect+store 全链路 T37~T40）
  - 验证：`cargo test -p eneros-cloud-coordinator aggregator` 40 通过

- [x] Task 3: 新增 `configs/cloud_aggregator.toml`
  - [ ] SubTask 3.1: `[cloud_aggregator]` 段：`collect_interval_ms = 5000` / `max_sources = 32` / `metric_sanitize = true`
  - [ ] SubTask 3.2: 中文注释 6 点（汇聚 <5s / 数据源超时跳过不中断 / 存储失败重试 / 数据脱敏标记 / NaN 防御 / 新数据源可扩展）

- [x] Task 4: 新增 `docs/agents/cloud-aggregation-design.md`
  - [ ] SubTask 4.1: 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
  - [ ] SubTask 4.2: 2 个 Mermaid 图（多 EdgeBox 数据汇聚数据流图 + collect/store 决策流程图含失败跳过/空源/存储失败分支）
  - [ ] SubTask 4.3: D1~D12 偏差表与 spec 一致；接口契约与实现签名一致

- [x] Task 5: 根目录版本同步 0.95.0 → 0.96.0
  - [ ] SubTask 5.1: `Makefile` 版本注释同步
  - [ ] SubTask 5.2: `.github/workflows/ci.yml` 版本注释同步
  - [ ] SubTask 5.3: `ci/src/gate.rs` 注释串尾追加 v0.96.0 类型清单（10 项）

- [x] Task 6: 构建验证（§2.4.2 全量）
  - [ ] SubTask 6.1: `cargo metadata --format-version 1` 成功
  - [ ] SubTask 6.2: `cargo test -p eneros-cloud-coordinator` 80 通过（40 既有 + 40 新增）
  - [ ] SubTask 6.3: `cargo build -p eneros-cloud-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [ ] SubTask 6.4: `cargo fmt --all -- --check` 通过
  - [ ] SubTask 6.5: `cargo clippy -p eneros-cloud-coordinator --all-targets -- -D warnings` 0 warning
  - [ ] SubTask 6.6: `cargo deny check advisories licenses bans sources`（零新增依赖）
  - [ ] SubTask 6.7: 回归零破坏：eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

- [x] Task 7: 按 `checklist.md` 逐项核验并勾选（未通过禁止收工）

# Task Dependencies

- Task 2 依赖 Task 1（lib.rs 追加与 crate 骨架确认）
- Task 3/4/5 与 Task 2 可并行（配置/文档/版本同步）
- Task 6 依赖 Task 1~5 全部完成
- Task 7 依赖 Task 6 通过
