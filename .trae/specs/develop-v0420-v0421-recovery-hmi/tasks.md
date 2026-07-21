# Tasks — v0.42.0 + v0.42.1 故障恢复编排 + 本地 HMI

## Wave 1: 错误变体 + 依赖图（并行）

- [x] Task 1: 扩展 `AgentError` 新增 2 个错误变体
  - 在 `crates/agents/agent/src/error.rs` 的 `Overheat { temp: f32 }` 之后新增：
    - `CircularDependency` — 循环依赖
    - `RecoveryFailed { agent: AgentId, attempts: u32 }` — 恢复失败
  - 实现 Display：`"circular dependency detected"` / `"recovery failed: agent {:?} after {} attempts"`
  - 新增 2 个单元测试：`test_recovery_orchestrator_error_variants_display` + `test_recovery_orchestrator_error_variants_eq`
  - 验证：`cargo build -p eneros-agent` 通过

- [x] Task 2: 创建 `system_agent/dependency.rs` — 依赖图
  - 新建文件 `crates/agents/agent/src/system_agent/dependency.rs`
  - 定义 `DependencyGraph` 结构体（D1：`BTreeMap<AgentId, Vec<AgentId>>` + `recovered: BTreeSet<AgentId>` + `failed: BTreeSet<AgentId>`）
  - 方法：`new()` / `add_dependency(agent, depends_on)` / `topological_sort() -> Result<Vec<AgentId>, AgentError>`（D6：Kahn 算法，环→`CircularDependency`）/ `can_recover(agent) -> bool`（D7：`recovered.contains(d) || failed.contains(d)`）/ `mark_recovered(agent)` / `mark_failed(agent)` / `has_cycle() -> bool`
  - 单元测试（≥8）：空图/添加依赖/拓扑排序/循环检测/can_recover/mark_recovered/mark_failed/多级依赖
  - 验证：文件编译正确（mod.rs 暂不声明，Task 4 统一接线）

## Wave 2: 恢复编排器（依赖 Task 1+2）

- [x] Task 3: 创建 `system_agent/recovery_orchestrator.rs` — 恢复编排器
  - 新建文件 `crates/agents/agent/src/system_agent/recovery_orchestrator.rs`
  - 定义 `RecoveryPriority` 枚举（Critical/High/Normal/Low）+ `priority_of(agent_type) -> RecoveryPriority`（D4）
  - 定义 `RecoveryOrchestrator` 结构体（D1：`BTreeMap` + `BTreeSet` + `VecDeque`）
    - `dependency_graph: DependencyGraph`
    - `queue: VecDeque<AgentId>`
    - `in_progress: BTreeSet<AgentId>`
    - `recovered: BTreeSet<AgentId>`
    - `failed: BTreeSet<AgentId>`
    - `agent_types: BTreeMap<AgentId, AgentType>`（用于优先级查询）
  - 方法：`new()` / `add_dependency(agent, depends_on, agent_type)` / `schedule_recovery(agent, agent_type)`（D3）/ `schedule_batch(agents, agent_types)` / `process_next() -> Option<AgentId>`（D4：按优先级排序后选可恢复的）/ `on_agent_recovered(agent)` / `on_agent_failed(agent)` / `pending_count() -> usize`（D3）/ `is_complete() -> bool`
  - 单元测试（≥10）：空编排器/单 Agent 恢复/多 Agent 顺序恢复/依赖阻塞/失败依赖不阻塞/优先级排序/批量调度/恢复完成/循环依赖场景/pending_count
  - 验证：文件编译正确

## Wave 3: v0.42.0 模块集成（依赖 Task 3）

- [x] Task 4: 更新 `system_agent/mod.rs` + `lib.rs` — v0.42.0 模块集成
  - 修改 `crates/agents/agent/src/system_agent/mod.rs`：
    - 新增 `pub mod dependency;` + `pub mod recovery_orchestrator;`
    - 新增 re-exports：`DependencyGraph` / `RecoveryOrchestrator` / `RecoveryPriority`
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 在 system_agent re-exports 中新增 `DependencyGraph` / `RecoveryOrchestrator` / `RecoveryPriority`
    - 更新模块文档注释（v0.42.0 描述）
    - 更新 `VERSION` 为 `"0.42.0"`（临时，Task 12 最终更新为 0.42.1）
  - 验证：`cargo build -p eneros-agent` 通过 + `cargo test -p eneros-agent --lib system_agent` 通过

## Wave 4: v0.42.0 集成测试 + HMI 骨架（并行，依赖 Task 4）

- [x] Task 5: 编写 v0.42.0 集成测试 `tests/recovery_orchestrator_test.rs`
  - 新建文件 `crates/agents/agent/tests/recovery_orchestrator_test.rs`
  - 10 个集成测试：
    1. `test_dependency_graph_basic` — 添加依赖 + 拓扑排序
    2. `test_dependency_graph_cycle_detection` — 循环依赖检测
    3. `test_recovery_orchestrator_single_agent` — 单 Agent 恢复
    4. `test_recovery_orchestrator_ordered_recovery` — 多 Agent 依赖有序恢复
    5. `test_recovery_orchestrator_dependency_blocked` — 依赖未恢复时阻塞
    6. `test_recovery_orchestrator_failed_dependency_not_blocked` — 失败依赖不阻塞（D7）
    7. `test_recovery_orchestrator_priority_ordering` — 优先级排序
    8. `test_recovery_orchestrator_batch_schedule` — 批量调度
    9. `test_recovery_orchestrator_is_complete` — 恢复完成检测
    10. `test_recovery_orchestrator_pending_count` — pending_count 正确性
  - 验证：`cargo test -p eneros-agent --test recovery_orchestrator_test` 通过

- [x] Task 6: 创建 eneros-hmi crate 骨架
  - 新建目录 `crates/agents/hmi/`
  - 新建 `crates/agents/hmi/Cargo.toml`：
    - `name = "eneros-hmi"` / `version.workspace = true` / `#![cfg_attr(not(test), no_std)]`
    - 依赖：`eneros-agent = { path = "../agent" }`
  - 新建 `crates/agents/hmi/src/lib.rs`：
    - `#![cfg_attr(not(test), no_std)]`
    - 定义类型（D11）：
      - `AgentStateSummary { agent_id: AgentId, name: String, state: AgentState, agent_type: AgentType }`
      - `NetworkStatus { connected: bool, ip_addr: Option<String>, rssi: Option<i8> }`
      - `PowerState { battery_pct: u8, charging: bool, ac_connected: bool }`
      - `SystemState { agent_states: Vec<AgentStateSummary>, storage_usage_mb: u32, network: NetworkStatus, power: PowerState, last_update_ms: u64 }`
      - `AlarmSummary { id: u64, severity: AlarmSeverity, message: String, timestamp: u64 }`
      - `AlarmSeverity { Info, Warning, Critical }`
      - `ManualAction { id: u64, action_type: String, target_agent: Option<AgentId>, params: String }`
      - `ApprovalId(u64)`
      - `HmiFrame { system_state: SystemState, active_alarms: Vec<AlarmSummary>, pending_approvals: Vec<PendingApproval>, manual_actions: Vec<ManualAction> }`
      - `HmiError` 枚举
      - `render_hmi_screen(state: &SystemState) -> HmiFrame`
    - 声明子模块：`pub mod approval;` / `pub mod console;` / `pub mod web;`
  - 暂不加入 workspace Cargo.toml（Task 10 统一添加）
  - 验证：文件创建正确

## Wave 5: HMI 子模块（并行，依赖 Task 6）

- [x] Task 7: 创建 `hmi/src/approval.rs` — 审批状态机
  - 定义 `ApprovalState` 枚举：Pending / Approved / Rejected / Executed / Expired
  - 定义 `PendingApproval { id: ApprovalId, action: ManualAction, requester: String, timestamp: u64, state: ApprovalState }`
  - 定义 `ApprovalManager` 结构体（D14：内存状态机）
    - `approvals: BTreeMap<ApprovalId, PendingApproval>`
    - `next_id: u64`
  - 方法：`new()` / `submit(&mut self, action: ManualAction, requester: &str, now: u64) -> ApprovalId` / `approve(&mut self, id: ApprovalId) -> Result<(), HmiError>` / `reject(&mut self, id: ApprovalId) -> Result<(), HmiError>` / `execute(&mut self, id: ApprovalId) -> Result<ManualAction, HmiError>` / `expire(&mut self, id: ApprovalId) -> Result<(), HmiError>` / `list_pending(&self) -> Vec<&PendingApproval>` / `get(&self, id: ApprovalId) -> Option<&PendingApproval>`
  - 单元测试（≥8）：提交/审批通过/审批拒绝/执行/过期/状态转换非法/列表查询/重复提交

- [x] Task 8: 创建 `hmi/src/console.rs` — 串口控制台渲染
  - 定义 `ConsoleRenderer` 结构体
  - 定义 `ConsoleOutput` trait（D9：I/O 抽象）：`fn write_str(&mut self, s: &str) -> Result<(), HmiError>`
  - 方法：`new()` / `render(&self, state: &SystemState) -> String`（D13：返回文本，VT100 可选）/ `render_frame(&self, frame: &HmiFrame) -> String` / `render_approvals(&self, approvals: &[PendingApproval]) -> String` / `write_to(&self, state: &SystemState, output: &mut dyn ConsoleOutput) -> Result<(), HmiError>`
  - 单元测试（≥6）：渲染状态/渲染帧/渲染审批/空状态/多 Agent/告警显示

- [x] Task 9: 创建 `hmi/src/web.rs` — HTTP 类型 + JSON
  - 定义 `HttpMethod` 枚举：Get / Post / Put / Delete
  - 定义 `HttpRequest { method: HttpMethod, path: String, body: Option<String> }`
  - 定义 `HttpResponse { status: u16, body: String, content_type: String }`
  - 定义 `WebHandler` 结构体（D10：无 TCP 服务器）
  - 方法：`new()` / `handle(&self, req: &HttpRequest, state: &SystemState) -> HttpResponse` / `status_to_json(state: &SystemState) -> String`（手动 JSON 序列化）/ `approvals_to_json(approvals: &[PendingApproval]) -> String`
  - 单元测试（≥6）：GET /status / POST /action / 404 / JSON 序列化 / JSON 反序列化 / 空状态

## Wave 6: HMI 集成（依赖 Task 7+8+9）

- [x] Task 10: 更新 workspace Cargo.toml + hmi/lib.rs 集成
  - 修改根 `Cargo.toml`：workspace members 新增 `"crates/agents/hmi"`
  - 修改 `crates/agents/hmi/src/lib.rs`：确认 re-exports（ApprovalManager / ConsoleRenderer / WebHandler 等）
  - 验证：`cargo build -p eneros-hmi` 通过

## Wave 7: 测试 + 文档 + 配置（并行，依赖 Task 10）

- [x] Task 11: 编写 HMI 集成测试 + 设计文档 + 配置模板
  - 新建 `crates/agents/hmi/tests/hmi_test.rs`（10 个集成测试）：
    1. `test_render_hmi_screen` — HmiFrame 渲染
    2. `test_approval_submit_and_approve` — 审批提交+通过
    3. `test_approval_reject` — 审批拒绝
    4. `test_approval_execute` — 审批执行
    5. `test_console_render_state` — 控制台状态渲染
    6. `test_console_render_frame` — 控制台帧渲染
    7. `test_web_status_endpoint` — Web 状态查询
    8. `test_web_action_endpoint` — Web 操作提交
    9. `test_web_404` — Web 未知路径
    10. `test_hmi_frame_empty_state` — 空状态帧
  - 新建 `docs/runtime/local-hmi-design.md`（≥10 章，含 mermaid 图）
  - 新建 `docs/agents/recovery-orchestrator-design.md`（≥10 章，含 mermaid 图，D1-D7 偏差表）
  - 新建 `configs/hmi.toml`（配置模板：串口波特率/Web 端口/权限配置）
  - 验证：`cargo test -p eneros-hmi` 通过

## Wave 8: 版本同步 + 构建验证（依赖所有）

- [x] Task 12: 同步版本标识符 0.41.0 → 0.42.1
  - 根 `Cargo.toml`：`version = "0.42.1"`
  - `Makefile`：`VERSION := 0.42.1` + 版本注释
  - `.github/workflows/ci.yml`：版本注释
  - `ci/src/gate.rs`：2 处版本注释（clippy + test，描述新增 eneros-hmi）
  - `crates/agents/agent/src/lib.rs`：`VERSION = "0.42.1"` + 文档注释更新
  - 验证：grep 无残留 0.41.0 版本号（历史引用除外）

- [x] Task 13: 完整构建验证
  - `cargo fmt --all -- --check` — 格式检查
  - `cargo clippy -p eneros-agent -p eneros-hmi --all-targets -- -D warnings` — lint
  - `cargo test -p eneros-agent -p eneros-hmi` — 单元+集成测试
  - `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` — workspace 回归
  - WSL2 交叉编译：`cargo build -p eneros-agent -p eneros-hmi --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - `cargo deny check licenses bans sources` — 许可证检查
  - `cargo run -p eneros-ci` — CI 质量门禁

# Task Dependencies

- Task 1, Task 2: 无依赖（Wave 1 并行）
- Task 3: 依赖 Task 1（错误变体）+ Task 2（DependencyGraph）
- Task 4: 依赖 Task 3
- Task 5, Task 6: 依赖 Task 4（Wave 4 并行）
- Task 7, Task 8, Task 9: 依赖 Task 6（Wave 5 并行）
- Task 10: 依赖 Task 7 + Task 8 + Task 9
- Task 11: 依赖 Task 10
- Task 12: 依赖 Task 11
- Task 13: 依赖 Task 12
