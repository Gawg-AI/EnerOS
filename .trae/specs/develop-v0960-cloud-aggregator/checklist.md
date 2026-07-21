# Checklist — v0.96.0 Cloud Coordinator 基础：数据汇聚

> Spec：`spec.md`（develop-v0960-cloud-aggregator）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 新源文件 `aggregator.rs` 位于既有 `crates/agents/cloud-coordinator/src/` 下，未新建根目录 crate
- [x] C2: 根 `Cargo.toml` workspace members 无新增（复用既有 cloud-coordinator 成员），workspace 仍可解析
- [x] C3: 无新增跨 crate path 引用；cloud-coordinator `Cargo.toml` 无新依赖
- [x] C4: 新文档 `cloud-aggregation-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-cloud-coordinator` 80 通过（40 既有 + 40 新增）
- [x] C8: `cargo build -p eneros-cloud-coordinator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy -p eneros-cloud-coordinator --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖；licenses/bans/sources 通过，advisories 因 github.com 网络不可达跳过 DB 更新——环境问题，与 v0.94.0/v0.95.0 相同先例）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪（git status --porcelain 实测：仅源码/配置/文档/specs 等合法文件）
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；子模块不重复加 no_std attr；测试模块内 std::cell/std::rc 位于 `#[cfg(test)]` 下，lib.rs `#![cfg_attr(not(test), no_std)]` 惯例允许）

## D. 数据结构（C16~C21）

- [x] C16: `DomainData` 6 字段（`domain_id: u64` / `timestamp: u64` / `states: Vec<EdgeBoxState>` / `events: Vec<EventRecord>` / `metrics: BTreeMap<u64, f32>` / `is_sensitive: bool`）派生 Debug/Clone/PartialEq
- [x] C17: `EventRecord` 4 字段（`event_id: u64` / `event_type: EventType` / `timestamp: u64` / `severity: Severity`）派生 Debug/Clone/Copy/PartialEq
- [x] C18: `EventType { StateChange, Alarm, Command, Metric }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default
- [x] C19: `Severity { Info, Warning, Error, Critical }` 派生 Debug/Clone/Copy/PartialEq/Eq/Default
- [x] C20: `AggError { SourceFailed(u64), StoreFailed, EmptySources }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C21: `EdgeBoxState` 复用 `eneros_coordinator::EdgeBoxState`（aggregator.rs 中 `use eneros_coordinator::EdgeBoxState;`，不重复定义）

## E. DataAggregator 核心逻辑（C22~C34）

- [x] C22: `DataAggregator` 5 字段全 pub：`sources: Vec<Box<dyn DataSource>>` / `sink: Box<dyn DataSink>` / `collect_count` / `timeout_count` / `store_count`
- [x] C23: `new(sink)` 创建时 sources 空、3 计数器全零
- [x] C24: `add_source` 追加数据源到 sources
- [x] C25: `collect(now_ms)` 遍历 sources 逐个 fetch，Ok 加入 states，Err → `timeout_count += 1` 继续（不中断）
- [x] C26: `collect` 空 sources → `Err(EmptySources)`
- [x] C27: `collect` 成功 → DomainData.timestamp == now_ms，states 顺序与 sources 一致
- [x] C28: `collect` 成功 → metrics 含 `states.len()` 计数（u64 键）+ 总容量汇总（Σ capacity_mw，sanitize 后）
- [x] C29: `collect` 成功 → `collect_count += 1`
- [x] C30: `collect` 部分失败 → `Ok(DomainData)`（非 Err），states 仅含成功源，timeout_count 记录失败数
- [x] C31: `collect` 全部失败 → `Ok(DomainData)`（states 空），timeout_count == sources.len()
- [x] C32: `store(data, now_ms)` 委托 sink.store，Ok → `store_count += 1`
- [x] C33: `store` sink 失败 → `Err(StoreFailed)`，`store_count` 不变
- [x] C34: `store` 不依赖 collect（独立调用可成功）

## F. Mock 故障注入（C35~C40）

- [x] C35: `MockDataSource { state, fail_times }` 构造时 fail_times=N，前 N 次 fetch `Err(SourceFailed(id))`，第 N+1 次起 `Ok(state)`（Err 载荷取 `state.box_id` 作源标识，省略冗余 source_id 字段，与 spec「id 从 1 顺序分配」表述偏差但语义一致——t23 验证 box_id=42 → SourceFailed(42)）
- [x] C36: `MockDataSource` 成功 fetch 返回的 state 与构造时一致（Clone 语义）
- [x] C37: `MockDataSink { stored, fail_times }` 构造时 fail_times=N，前 N 次 store `Err(StoreFailed)`，第 N+1 次起 `Ok` 且 data 入 stored
- [x] C38: `MockDataSink` stored 顺序与 store 调用顺序一致
- [x] C39: `MockDataSource` / `MockDataSink` 均实现对应 trait，可作 `Box<dyn DataSource>` / `Box<dyn DataSink>` 注入
- [x] C40: 多 MockDataSource 混合（1 成功 2 失败）→ collect 返回 1 个 state，timeout_count == 2

## G. NaN 防御与脱敏（C41~C45）

- [x] C41: metrics BTreeMap 值非有限（NaN/±Inf）→ 存入 DomainData 前 sanitize 为 0.0（D9）
- [x] C42: capacity_mw 非有限/≤0 → 该 source 对应 metrics 容量项 sanitize 为 0.0（不阻断汇聚）
- [x] C43: `is_sensitive` 字段存在且默认 false（构造时可显式设置 true）
- [x] C44: 脱敏标记不参与 collect/store 逻辑（仅元数据透传）
- [x] C45: 数据量计数（collect_count/timeout_count/store_count）为 u64 独立字段，不依赖 metrics

## H. crate 集成（C46~C49）

- [x] C46: `lib.rs` 追加 `pub mod aggregator;`（既有 strategy/channel/publisher 模块声明零改动）
- [x] C47: `lib.rs` 追加 10 项重导出（DomainData / EventRecord / EventType / Severity / DataAggregator / DataSource / DataSink / AggError / MockDataSource / MockDataSink）
- [x] C48: `lib.rs` crate 文档升级为 v0.95.0 + v0.96.0 双版本说明（核心类型清单表追加 aggregator 行 + v0.96.0 D1~D12 简表）
- [x] C49: cloud-coordinator `Cargo.toml` description 追加 v0.96.0，无新依赖

## I. 配置文件（C50~C52）

- [x] C50: `configs/cloud_aggregator.toml` 存在，`[cloud_aggregator]` 段含 `collect_interval_ms = 5000` / `max_sources = 32` / `metric_sanitize = true`
- [x] C51: 中文注释覆盖 6 点：汇聚 <5s / 数据源超时跳过不中断 / 存储失败重试 / 数据脱敏标记 / NaN 防御 / 新数据源可扩展
- [x] C52: 配置值与代码语义一致（5000ms / 32 源 / sanitize 开关）

## J. 设计文档（C53~C56）

- [x] C53: `docs/agents/cloud-aggregation-design.md` 存在，12 章节齐全
- [x] C54: 含 2 个 Mermaid 图（多 EdgeBox 数据汇聚数据流图 + collect/store 决策流程图含失败跳过/空源/存储失败分支）
- [x] C55: 含 D1~D12 偏差表，与 spec 偏差声明一致
- [x] C56: 接口契约与实现一致（函数签名、字段、错误变体、计数器语义）

## K. 版本同步（C57~C60）

- [x] C57: 根 `Cargo.toml` `[workspace.package] version = "0.96.0"`
- [x] C58: `Makefile` 版本注释同步 0.96.0
- [x] C59: `.github/workflows/ci.yml` 版本注释同步 0.96.0
- [x] C60: `ci/src/gate.rs` 注释追加 v0.96.0 类型清单（10 项）

## L. 测试覆盖（C61~C67）

- [x] C61: 内嵌 40 个单元测试（T1~T40）全部实现并通过
- [x] C62: 测试分布：数据结构 T1~T6 / collect 核心 T7~T14 / store T15~T20 / Mock 故障注入 T21~T26 / NaN 防御 T27~T30 / 脱敏标记 T31~T32 / 多 source 汇聚 T33~T36 / collect+store 全链路 T37~T40
- [x] C63: 含 3 source 汇聚集成测试（部分失败部分成功，验证计数与 states 长度）
- [x] C64: 含 collect+store 全链路测试（先 collect 得 DomainData，再 store 成功，计数器正确）
- [x] C65: 含 NaN 风暴防御测试（metrics 含 NaN/Inf，capacity 非有限）
- [x] C66: 含空 sources 边界测试（Err(EmptySources)）
- [x] C67: 回归零破坏：eneros-coordinator（120）/ eneros-energy-market-agent（185）/ eneros-twin-agent（120）/ eneros-agent-bus-dds（63）全通过

## M. 蓝图达成（C68~C72）

- [x] C68: v0.96.0 交付物全覆盖：数据汇聚（DataAggregator.collect）/ 存储（DataAggregator.store + DataSink trait）/ Schema（DomainData + EventRecord + BTreeMap metrics）
- [x] C69: 数据源超时跳过 + 标记（timeout_count），不中断汇聚（蓝图 §4.4）
- [x] C70: 数据脱敏标记（is_sensitive），支撑蓝图 §7.3 安全要求
- [x] C71: 无 BREAKING：既有全部 crate 零改动，v0.95.0 既有 3 模块零改动，既有公共 API 全保留（EdgeBoxState/DevicePool 各补 1 行 PartialEq derive，纯增量零行为变化，DomainData PartialEq 依赖链必需，回归 488 测试全过）
- [x] C72: 下游解锁：v0.112.0 云端孪生主节点数据基础
