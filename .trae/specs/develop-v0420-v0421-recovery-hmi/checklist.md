# Checklist — v0.42.0 + v0.42.1 故障恢复编排 + 本地 HMI

## C1-C12: Task 1 — 错误变体

- [ ] C1: `AgentError::CircularDependency` 变体存在于 error.rs
- [ ] C2: `AgentError::RecoveryFailed { agent: AgentId, attempts: u32 }` 变体存在于 error.rs
- [ ] C3: 两个新变体位于 `Overheat { temp: f32 }` 之后
- [ ] C4: `CircularDependency` 的 Display 输出为 `"circular dependency detected"`
- [ ] C5: `RecoveryFailed` 的 Display 输出包含 agent 和 attempts
- [ ] C6: Display 实现使用 `write!(f, ...)` 格式化
- [ ] C7: 单元测试 `test_recovery_orchestrator_error_variants_display` 存在
- [ ] C8: 单元测试 `test_recovery_orchestrator_error_variants_eq` 存在
- [ ] C9: 测试验证 `CircularDependency == CircularDependency`
- [ ] C10: 测试验证 `RecoveryFailed { agent: AgentId(1), attempts: 3 } == RecoveryFailed { agent: AgentId(1), attempts: 3 }`
- [ ] C11: 测试验证不同 attempts 的 RecoveryFailed 不相等
- [ ] C12: `cargo build -p eneros-agent` 通过

## C13-C30: Task 2 — DependencyGraph

- [ ] C13: 文件 `crates/agents/agent/src/system_agent/dependency.rs` 存在
- [ ] C14: `DependencyGraph` 结构体定义存在
- [ ] C15: `dependencies: BTreeMap<AgentId, Vec<AgentId>>` 字段（D1：BTreeMap 而非 HashMap）
- [ ] C16: `recovered: BTreeSet<AgentId>` 字段（D1：BTreeSet 而非 HashSet）
- [ ] C17: `failed: BTreeSet<AgentId>` 字段
- [ ] C18: `new()` 方法返回空 DependencyGraph
- [ ] C19: `add_dependency(agent, depends_on)` 方法添加依赖关系
- [ ] C20: `topological_sort() -> Result<Vec<AgentId>, AgentError>` 方法存在
- [ ] C21: 拓扑排序使用 Kahn 算法（D6）
- [ ] C22: 循环依赖时 `topological_sort()` 返回 `Err(CircularDependency)`
- [ ] C23: `can_recover(agent) -> bool` 方法存在
- [ ] C24: `can_recover` 检查 `recovered.contains(d) || failed.contains(d)`（D7）
- [ ] C25: `mark_recovered(agent)` 方法将 agent 加入 recovered 集合
- [ ] C26: `mark_failed(agent)` 方法将 agent 加入 failed 集合
- [ ] C27: `has_cycle() -> bool` 方法存在
- [ ] C28: 无依赖的 agent `can_recover` 返回 true
- [ ] C29: 依赖未恢复/未失败的 agent `can_recover` 返回 false
- [ ] C30: 单元测试 ≥8 个（空图/添加依赖/拓扑排序/循环检测/can_recover/mark_recovered/mark_failed/多级依赖）

## C31-C55: Task 3 — RecoveryOrchestrator

- [ ] C31: 文件 `crates/agents/agent/src/system_agent/recovery_orchestrator.rs` 存在
- [ ] C32: `RecoveryPriority` 枚举定义存在（Critical/High/Normal/Low）
- [ ] C33: `priority_of(agent_type) -> RecoveryPriority` 函数存在（D4）
- [ ] C34: System 类型映射为 Critical
- [ ] C35: Energy 类型映射为 High
- [ ] C36: Market/Grid 类型映射为 Normal
- [ ] C37: Device 类型映射为 Low
- [ ] C38: `RecoveryOrchestrator` 结构体定义存在
- [ ] C39: `dependency_graph: DependencyGraph` 字段
- [ ] C40: `queue: VecDeque<AgentId>` 字段
- [ ] C41: `in_progress: BTreeSet<AgentId>` 字段
- [ ] C42: `recovered: BTreeSet<AgentId>` 字段
- [ ] C43: `failed: BTreeSet<AgentId>` 字段
- [ ] C44: `agent_types: BTreeMap<AgentId, AgentType>` 字段（用于优先级查询）
- [ ] C45: `new()` 方法返回空 RecoveryOrchestrator
- [ ] C46: `add_dependency(agent, depends_on, agent_type)` 方法存在
- [ ] C47: `schedule_recovery(agent, agent_type)` 方法存在（D3）
- [ ] C48: `schedule_batch(agents, agent_types)` 方法存在
- [ ] C49: `process_next() -> Option<AgentId>` 方法存在
- [ ] C50: `process_next` 按优先级排序后选可恢复的 Agent（D4）
- [ ] C51: `process_next` 无可恢复 Agent 时返回 None
- [ ] C52: `on_agent_recovered(agent)` 方法将 agent 从 in_progress 移到 recovered
- [ ] C53: `on_agent_failed(agent)` 方法将 agent 从 in_progress 移到 failed
- [ ] C54: `pending_count() -> usize` 方法存在（D3）
- [ ] C55: `is_complete() -> bool` 方法存在（queue 空 + in_progress 空）

## C56-C65: Task 4 — v0.42.0 模块集成

- [ ] C56: `system_agent/mod.rs` 包含 `pub mod dependency;`
- [ ] C57: `system_agent/mod.rs` 包含 `pub mod recovery_orchestrator;`
- [ ] C58: `system_agent/mod.rs` re-exports `DependencyGraph`
- [ ] C59: `system_agent/mod.rs` re-exports `RecoveryOrchestrator`
- [ ] C60: `system_agent/mod.rs` re-exports `RecoveryPriority`
- [ ] C61: `lib.rs` re-exports `DependencyGraph` / `RecoveryOrchestrator` / `RecoveryPriority`
- [ ] C62: `lib.rs` VERSION 更新为 `"0.42.0"`（临时）
- [ ] C63: `lib.rs` 模块文档注释包含 v0.42.0 描述
- [ ] C64: `cargo build -p eneros-agent` 通过
- [ ] C65: `cargo test -p eneros-agent --lib system_agent` 全部通过

## C66-C80: Task 5 — v0.42.0 集成测试

- [ ] C66: 文件 `crates/agents/agent/tests/recovery_orchestrator_test.rs` 存在
- [ ] C67: `test_dependency_graph_basic` 测试存在
- [ ] C68: `test_dependency_graph_cycle_detection` 测试存在
- [ ] C69: `test_recovery_orchestrator_single_agent` 测试存在
- [ ] C70: `test_recovery_orchestrator_ordered_recovery` 测试存在
- [ ] C71: `test_recovery_orchestrator_dependency_blocked` 测试存在
- [ ] C72: `test_recovery_orchestrator_failed_dependency_not_blocked` 测试存在
- [ ] C73: `test_recovery_orchestrator_priority_ordering` 测试存在
- [ ] C74: `test_recovery_orchestrator_batch_schedule` 测试存在
- [ ] C75: `test_recovery_orchestrator_is_complete` 测试存在
- [ ] C76: `test_recovery_orchestrator_pending_count` 测试存在
- [ ] C77: 测试覆盖拓扑排序验证（依赖在前）
- [ ] C78: 测试覆盖循环依赖场景
- [ ] C79: 测试覆盖优先级排序（Critical 先于 Low）
- [ ] C80: `cargo test -p eneros-agent --test recovery_orchestrator_test` 通过

## C81-C100: Task 6 — HMI crate 骨架

- [ ] C81: 目录 `crates/agents/hmi/` 存在
- [ ] C82: `crates/agents/hmi/Cargo.toml` 存在
- [ ] C83: Cargo.toml `name = "eneros-hmi"`
- [ ] C84: Cargo.toml `version.workspace = true`
- [ ] C85: Cargo.toml 依赖 `eneros-agent = { path = "../agent" }`
- [ ] C86: `crates/agents/hmi/src/lib.rs` 存在
- [ ] C87: lib.rs 包含 `#![cfg_attr(not(test), no_std)]`（D8）
- [ ] C88: `AgentStateSummary` 结构体定义（agent_id, name, state, agent_type）
- [ ] C89: `NetworkStatus` 结构体定义（connected, ip_addr, rssi）（D11）
- [ ] C90: `PowerState` 结构体定义（battery_pct, charging, ac_connected）（D11）
- [ ] C91: `SystemState` 结构体定义（agent_states, storage_usage_mb, network, power, last_update_ms）
- [ ] C92: `AlarmSummary` 结构体定义（id, severity, message, timestamp）
- [ ] C93: `AlarmSeverity` 枚举定义（Info, Warning, Critical）
- [ ] C94: `ManualAction` 结构体定义（id, action_type, target_agent, params）
- [ ] C95: `ApprovalId(u64)` 类型定义
- [ ] C96: `HmiFrame` 结构体定义（system_state, active_alarms, pending_approvals, manual_actions）
- [ ] C97: `HmiError` 枚举定义
- [ ] C98: `render_hmi_screen(state: &SystemState) -> HmiFrame` 函数存在
- [ ] C99: lib.rs 声明 `pub mod approval;` / `pub mod console;` / `pub mod web;`
- [ ] C100: 文件创建正确（无语法错误）

## C101-C115: Task 7 — 审批状态机

- [ ] C101: 文件 `crates/agents/hmi/src/approval.rs` 存在
- [ ] C102: `ApprovalState` 枚举定义（Pending, Approved, Rejected, Executed, Expired）
- [ ] C103: `PendingApproval` 结构体定义（id, action, requester, timestamp, state）
- [ ] C104: `ApprovalManager` 结构体定义（approvals, next_id）（D14：内存状态机）
- [ ] C105: `ApprovalManager::new()` 方法
- [ ] C106: `submit(action, requester, now) -> ApprovalId` 方法
- [ ] C107: `approve(id) -> Result<(), HmiError>` 方法
- [ ] C108: `reject(id) -> Result<(), HmiError>` 方法
- [ ] C109: `execute(id) -> Result<ManualAction, HmiError>` 方法
- [ ] C110: `expire(id) -> Result<(), HmiError>` 方法
- [ ] C111: `list_pending() -> Vec<&PendingApproval>` 方法
- [ ] C112: `get(id) -> Option<&PendingApproval>` 方法
- [ ] C113: Pending→Approved 转换合法
- [ ] C114: Approved→Executed 转换合法
- [ ] C115: 单元测试 ≥8 个

## C116-C128: Task 8 — 串口控制台渲染

- [ ] C116: 文件 `crates/agents/hmi/src/console.rs` 存在
- [ ] C117: `ConsoleRenderer` 结构体定义
- [ ] C118: `ConsoleOutput` trait 定义（D9：I/O 抽象，`write_str` 方法）
- [ ] C119: `ConsoleRenderer::new()` 方法
- [ ] C120: `render(state: &SystemState) -> String` 方法（D13：返回文本）
- [ ] C121: `render_frame(frame: &HmiFrame) -> String` 方法
- [ ] C122: `render_approvals(approvals: &[PendingApproval]) -> String` 方法
- [ ] C123: `write_to(state, output: &mut dyn ConsoleOutput) -> Result<(), HmiError>` 方法
- [ ] C124: 渲染输出包含 Agent 列表
- [ ] C125: 渲染输出包含系统资源信息
- [ ] C126: 渲染输出包含告警信息
- [ ] C127: 空状态渲染不 panic
- [ ] C128: 单元测试 ≥6 个

## C129-C140: Task 9 — Web 类型 + JSON

- [ ] C129: 文件 `crates/agents/hmi/src/web.rs` 存在
- [ ] C130: `HttpMethod` 枚举定义（Get, Post, Put, Delete）
- [ ] C131: `HttpRequest` 结构体定义（method, path, body）
- [ ] C132: `HttpResponse` 结构体定义（status, body, content_type）
- [ ] C133: `WebHandler` 结构体定义（D10：无 TCP 服务器）
- [ ] C134: `WebHandler::new()` 方法
- [ ] C135: `handle(req: &HttpRequest, state: &SystemState) -> HttpResponse` 方法
- [ ] C136: `status_to_json(state: &SystemState) -> String` 方法（手动 JSON）
- [ ] C137: `approvals_to_json(approvals: &[PendingApproval]) -> String` 方法
- [ ] C138: GET /status 返回 200 + JSON
- [ ] C139: 未知路径返回 404
- [ ] C140: 单元测试 ≥6 个

## C141-C148: Task 10 — HMI 集成

- [ ] C141: 根 `Cargo.toml` workspace members 包含 `"crates/agents/hmi"`
- [ ] C142: `crates/agents/hmi/src/lib.rs` re-exports `ApprovalManager`
- [ ] C143: `crates/agents/hmi/src/lib.rs` re-exports `ConsoleRenderer`
- [ ] C144: `crates/agents/hmi/src/lib.rs` re-exports `WebHandler`
- [ ] C145: `crates/agents/hmi/src/lib.rs` re-exports `ApprovalState` / `PendingApproval`
- [ ] C146: `crates/agents/hmi/src/lib.rs` re-exports `HttpRequest` / `HttpResponse` / `HttpMethod`
- [ ] C147: `crates/agents/hmi/src/lib.rs` re-exports `ConsoleOutput` trait
- [ ] C148: `cargo build -p eneros-hmi` 通过

## C149-C165: Task 11 — HMI 测试 + 文档 + 配置

- [ ] C149: 文件 `crates/agents/hmi/tests/hmi_test.rs` 存在
- [ ] C150: `test_render_hmi_screen` 测试存在
- [ ] C151: `test_approval_submit_and_approve` 测试存在
- [ ] C152: `test_approval_reject` 测试存在
- [ ] C153: `test_approval_execute` 测试存在
- [ ] C154: `test_console_render_state` 测试存在
- [ ] C155: `test_console_render_frame` 测试存在
- [ ] C156: `test_web_status_endpoint` 测试存在
- [ ] C157: `test_web_action_endpoint` 测试存在
- [ ] C158: `test_web_404` 测试存在
- [ ] C159: `test_hmi_frame_empty_state` 测试存在
- [ ] C160: `cargo test -p eneros-hmi` 通过
- [ ] C161: 文件 `docs/runtime/local-hmi-design.md` 存在
- [ ] C162: local-hmi-design.md ≥10 章，含 mermaid 图
- [ ] C163: 文件 `docs/agents/recovery-orchestrator-design.md` 存在
- [ ] C164: recovery-orchestrator-design.md ≥10 章，含 mermaid 图，D1-D7 偏差表
- [ ] C165: 文件 `configs/hmi.toml` 存在（配置模板）

## C166-C175: Task 12 — 版本同步

- [ ] C166: 根 `Cargo.toml` version = "0.42.1"
- [ ] C167: `Makefile` VERSION := 0.42.1
- [ ] C168: `Makefile` 版本注释更新为 v0.42.1
- [ ] C169: `.github/workflows/ci.yml` 版本注释更新为 v0.42.1
- [ ] C170: `ci/src/gate.rs` clippy 注释更新为 v0.42.1（含 eneros-hmi 描述）
- [ ] C171: `ci/src/gate.rs` test 注释更新为 v0.42.1（含 eneros-hmi 描述）
- [ ] C172: `crates/agents/agent/src/lib.rs` VERSION = "0.42.1"
- [ ] C173: `crates/agents/agent/src/lib.rs` 模块文档更新为 v0.42.1
- [ ] C174: grep 无残留 0.41.0 版本号（历史引用除外）
- [ ] C175: 版本同步未修改历史注释中的版本引用

## C176-C190: Task 13 — 构建验证

- [ ] C176: `cargo fmt --all -- --check` 通过
- [ ] C177: `cargo clippy -p eneros-agent -p eneros-hmi --all-targets -- -D warnings` 通过
- [ ] C178: `cargo test -p eneros-agent` 全部通过
- [ ] C179: `cargo test -p eneros-hmi` 全部通过
- [ ] C180: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [ ] C181: WSL2 交叉编译 `cargo build -p eneros-agent --target aarch64-unknown-none` 通过
- [ ] C182: WSL2 交叉编译 `cargo build -p eneros-hmi --target aarch64-unknown-none` 通过
- [ ] C183: `cargo deny check licenses bans sources` 通过
- [ ] C184: `cargo deny check advisories` 已知 GitHub 网络问题（环境限制）
- [ ] C185: `cargo run -p eneros-ci` fmt+clippy 通过
- [ ] C186: eneros-ci test 通过（eneros-fs wear test 偶发失败为 pre-existing 问题）
- [ ] C187: 无 `use std::*` 在非测试代码中
- [ ] C188: 无 `panic!` / `todo!` / `unimplemented!` 在非测试代码中
- [ ] C189: 无 `HashMap` / `HashSet` 在新代码中（使用 BTreeMap/BTreeSet）
- [ ] C190: 所有新 crate 放入 `crates/<subsystem>/` 下

## C191-C200: 偏差合规与目录结构

- [ ] C191: D1 合规 — DependencyGraph/RecoveryOrchestrator 使用 BTreeMap/BTreeSet
- [ ] C192: D2 合规 — new() 使用 BTreeMap::new() 而非 HashMap::new()
- [ ] C193: D3 合规 — schedule_recovery + pending_count 已实现
- [ ] C194: D4 合规 — RecoveryPriority 枚举 + priority_of 函数 + process_next 优先级排序
- [ ] C195: D5 合规 — dependency.rs + recovery_orchestrator.rs 分离
- [ ] C196: D6 合规 — Kahn 算法拓扑排序，循环依赖返回 CircularDependency
- [ ] C197: D7 合规 — can_recover 检查 recovered || failed
- [ ] C198: D8 合规 — eneros-hmi crate 声明 no_std
- [ ] C199: D9-D14 合规 — HMI I/O 抽象 + 无 TCP 服务器 + 类型定义 + 配置模板 + 文本渲染 + 内存状态机
- [ ] C200: 目录结构合规 — hmi crate 在 `crates/agents/hmi/`，文档在 `docs/runtime/` + `docs/agents/`，配置在 `configs/`
