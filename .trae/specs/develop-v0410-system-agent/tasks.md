# Tasks — v0.41.0 System Agent 核心

## Wave 1: 基础数据结构（可并行）

- [x] Task 1: 扩展 `AgentError` 新增 3 个错误变体
  - 在 `crates/agents/agent/src/error.rs` 的 `AgentError` 枚举中 `NoCapability` 之后新增：
    - `SystemOverload` — 系统过载
    - `OomRisk` — OOM 风险
    - `Overheat { temp: f32 }` — 系统过热（含温度值）
  - 为 3 个新变体实现 `Display` trait
  - 新增单元测试 `test_system_agent_error_variants_display`（验证 Display 输出）
  - 新增单元测试 `test_system_agent_error_variants_eq`（验证 PartialEq）
  - 保留 `#[derive(Debug, Clone, PartialEq)]`（不含 Eq，因 `Overheat` 含 f32）
  - 验证：`cargo build -p eneros-agent` 通过

- [x] Task 2: 创建 `system_agent/monitor.rs` — 资源监控层
  - 新建文件 `crates/agents/agent/src/system_agent/monitor.rs`
  - 模块文档注释：说明 ResourceMonitor 作为 SystemAgent 的资源监控层，包含 D2（ResourceSource trait）、D3（BTreeMap）、D7（SystemConfig 定义）、D8（is_oom + find_oom_victim 分离）偏差声明
  - 定义 `ResourceSource` trait（object-safe）：
    ```rust
    pub trait ResourceSource {
        fn cpu_usage(&self) -> f32;
        fn mem_used(&self) -> usize;
        fn mem_total(&self) -> usize;
        fn temperature(&self) -> f32;
    }
    ```
  - 定义 `ResourceMonitor` 结构：
    ```rust
    pub struct ResourceMonitor {
        pub cpu_usage: f32,
        pub mem_total: usize,
        pub mem_used: usize,
        pub temperature: f32,
        source: Option<Box<dyn ResourceSource>>,
    }
    ```
  - 实现 `ResourceMonitor` 方法：
    - `pub fn new() -> Self` — 创建空监控器（source=None）
    - `pub fn with_source(source: Box<dyn ResourceSource>) -> Self` — 创建带数据源的监控器
    - `pub fn poll(&mut self)` — 从 source 读取最新值（若无 source 则 no-op）
    - `pub fn set_values(&mut self, cpu: f32, mem_used: usize, mem_total: usize, temp: f32)` — 手动设置值（测试用）
    - `pub fn is_oom(&self, threshold: f32) -> bool` — 判断 OOM（mem_total==0 时返回 false）
    - `pub fn is_overheat(&self, threshold: f32) -> bool` — 判断过热
    - `pub fn mem_usage_percent(&self) -> f32` — 内存使用率（mem_total==0 时返回 0.0）
  - 实现 `Default` trait
  - 定义 `SystemConfig` 结构（D7 偏差）：
    ```rust
    pub struct SystemConfig {
        pub oom_threshold_percent: f32,   // 默认 0.9
        pub overheat_threshold: f32,      // 默认 80.0
        pub monitor_interval_ms: u64,     // 默认 1000
    }
    ```
    - 实现 `Default` trait（默认值：0.9 / 80.0 / 1000）
  - 定义 `SystemStats` 结构：
    ```rust
    pub struct SystemStats {
        pub cpu_usage: f32,
        pub mem_usage: f32,
        pub temperature: f32,
        pub agent_count: usize,
        pub alive_agents: usize,
        pub error_agents: usize,
    }
    ```
  - 定义 `SystemEvent` 枚举：
    ```rust
    pub enum SystemEvent {
        Overheat { temp: f32 },
        OomVictimSuspended { agent: AgentId },
        AgentCrashed { agent: AgentId },
        AgentRecovered { agent: AgentId },
        AgentRecoveryFailed { agent: AgentId },
    }
    ```
  - 定义 `AgentResourceUsage` 结构（蓝图 §4.1）：
    ```rust
    pub struct AgentResourceUsage {
        pub cpu_percent: f32,
        pub mem_bytes: usize,
        pub state: AgentState,
    }
    ```
  - 编写单元测试（至少 8 个）：
    - `test_monitor_new_empty`
    - `test_monitor_set_values`
    - `test_monitor_is_oom_true`
    - `test_monitor_is_oom_false`
    - `test_monitor_is_overheat_true`
    - `test_monitor_is_overheat_false`
    - `test_monitor_mem_usage_percent`
    - `test_monitor_with_source_poll`
    - `test_system_config_default`
  - 验证：`cargo build -p eneros-agent` 通过（注意：此时 mod.rs 尚未声明 `pub mod monitor;`，可临时在文件内 `#[cfg(test)] mod tests` 内部测试）

## Wave 2: SystemAgent 核心（依赖 Task 1 + Task 2）

- [x] Task 3: 创建 `system_agent/mod.rs` — SystemAgent 结构体 + tick + 统计
  - 新建文件 `crates/agents/agent/src/system_agent/mod.rs`
  - 声明子模块：`pub mod manager;` + `pub mod monitor;`
  - re-exports：`pub use monitor::{ResourceMonitor, ResourceSource, SystemConfig, SystemEvent, SystemStats, AgentResourceUsage};`
  - 模块文档注释：说明 SystemAgent 作为 OS 级管理 Agent，包含 D1（tick 替代 run）、D4（lifecycle 字段）、D6（SystemEvent 替代 log）、D10（mod.rs 模式）、D11（force_state Error 后 handle_crash）偏差声明
  - 导入：
    - `alloc::rc::Rc` / `alloc::vec::Vec` / `core::cell::RefCell`
    - `crate::heartbeat::HeartbeatMonitor`
    - `crate::lifecycle::LifecycleManager`
    - `crate::recovery::CrashRecovery`
    - `crate::registry::AgentRegistry`
    - `crate::spawner::AgentSpawner`
    - `crate::error::AgentError`
    - `crate::id::AgentId`
    - `crate::types::AgentState`
    - `crate::health::HealthStatus`
    - `super::monitor::{ResourceMonitor, ResourceSource, SystemConfig, SystemEvent, SystemStats}`
  - 定义 `SystemAgent` 结构（D4 偏差：含 lifecycle 字段）：
    ```rust
    pub struct SystemAgent {
        registry: Rc<RefCell<AgentRegistry>>,
        spawner: Rc<AgentSpawner>,
        recovery: Rc<CrashRecovery>,
        heartbeat: Rc<RefCell<HeartbeatMonitor>>,
        lifecycle: Rc<RefCell<LifecycleManager>>,
        monitor: ResourceMonitor,
        config: SystemConfig,
    }
    ```
  - **注意**：不 derive `Debug`（含 `Rc<RefCell<...>>` 和 `Rc<dyn>` 字段，无法自动派生，与 `AgentSpawner` / `CrashRecovery` 同一约定）
  - 实现方法：
    - `pub fn new(registry, spawner, recovery, heartbeat, lifecycle, config) -> Self`
    - `pub fn tick(&mut self, now: u64) -> Vec<SystemEvent>` — D1 偏差：单步执行
      - 1. `self.monitor.poll()` 资源监控
      - 2. `self.heartbeat.borrow_mut().check(now)` 心跳检查
      - 3. 遍历 health_results，对 Unhealthy Agent：
         - D11: `force_state(id, Error)` 后 `recovery.handle_crash(id, now)`
         - 成功 → `SystemEvent::AgentRecovered`
         - `MaxRestartsExceeded` → `SystemEvent::AgentRecoveryFailed`
         - 其他错误 → `SystemEvent::AgentCrashed`
      - 4. OOM 检查：`monitor.is_oom(config.oom_threshold_percent)`
         - 若 OOM 且 `find_oom_victim()` 返回 Some(victim)：
           - `suspend_agent(victim)` 挂起 victim
           - 产生 `SystemEvent::OomVictimSuspended { agent: victim }`
      - 5. 过热检查：`monitor.is_overheat(config.overheat_threshold)`
         - 若过热：产生 `SystemEvent::Overheat { temp: monitor.temperature }`
      - 6. 返回 events
    - `pub fn get_system_stats(&self) -> SystemStats`
      - 从 registry 统计 agent_count / alive_agents / error_agents
      - 从 monitor 读取 cpu_usage / mem_usage / temperature
    - `pub fn find_oom_victim(&self) -> Option<AgentId>` — D8 偏差
      - 遍历 registry 中所有存活 Agent（`is_alive()`）
      - 返回 `priority` 最低的 AgentId
      - 若无存活 Agent，返回 `None`
    - `pub fn monitor(&self) -> &ResourceMonitor`
    - `pub fn config(&self) -> &SystemConfig`
  - 编写单元测试（至少 8 个）：
    - `test_system_agent_new`
    - `test_system_agent_tick_no_events`
    - `test_system_agent_tick_overheat`
    - `test_system_agent_tick_oom_suspends_victim`
    - `test_system_agent_get_system_stats`
    - `test_system_agent_find_oom_victim_lowest_priority`
    - `test_system_agent_find_oom_victim_no_alive`
    - `test_system_agent_find_oom_victim_multiple_agents`
  - 验证：`cargo build -p eneros-agent` 通过

## Wave 3: Agent 管理方法（依赖 Task 3）

- [x] Task 4: 创建 `system_agent/manager.rs` — Agent 管理方法
  - 新建文件 `crates/agents/agent/src/system_agent/manager.rs`
  - 模块文档注释：说明 Agent 管理方法，包含 D5（start_agent 接受 now）、D9（stop_agent 用 force_state）偏差声明
  - 导入：
    - `crate::error::AgentError`
    - `crate::id::AgentId`
    - `crate::init::AgentConfig`
    - `crate::types::AgentState`
    - `super::SystemAgent`
  - 在 `manager.rs` 中为 `SystemAgent` 实现 4 个管理方法：
    - `impl super::SystemAgent { ... }`
    - `pub fn start_agent(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>` — D5 偏差
      - 调用 `self.spawner.spawn(config, now)?`
      - 调用 `self.heartbeat.borrow_mut().register(id, now)`
      - 返回 `Ok(id)`
    - `pub fn stop_agent(&self, id: AgentId) -> Result<(), AgentError>` — D9 偏差
      - `self.lifecycle.borrow().force_state(id, AgentState::Dead)?`
      - `self.heartbeat.borrow_mut().unregister(id)`
      - `self.registry.borrow_mut().unregister(id)?`
      - 返回 `Ok(())`
    - `pub fn suspend_agent(&self, id: AgentId) -> Result<(), AgentError>`
      - `self.lifecycle.borrow().transition(id, AgentState::Suspended)?`
      - 返回 `Ok(())`
    - `pub fn resume_agent(&self, id: AgentId) -> Result<(), AgentError>`
      - `self.lifecycle.borrow().transition(id, AgentState::Running)?`
      - 返回 `Ok(())`
  - 编写单元测试（至少 6 个）：
    - `test_start_agent_success`
    - `test_stop_agent_success`
    - `test_stop_agent_not_found`
    - `test_suspend_agent_success`
    - `test_resume_agent_success`
    - `test_suspend_resume_cycle`
  - 验证：`cargo build -p eneros-agent` 通过

## Wave 4: 模块集成（依赖 Task 2 + Task 3 + Task 4）

- [x] Task 5: 更新 `lib.rs` 模块集成
  - 修改 `crates/agents/agent/src/lib.rs`：
    - 新增 `pub mod system_agent;`（字母序在 `spawner` 之后、`types` 之前）
    - 新增 re-exports：`pub use system_agent::{ResourceMonitor, ResourceSource, SystemAgent, SystemConfig, SystemEvent, SystemStats};`
    - 更新 `pub const VERSION: &str = "0.41.0";`
    - 更新模块文档注释（包含 System Agent 描述 + v0.41.0 偏差声明引用）
  - 验证：`cargo build -p eneros-agent` 通过

## Wave 5: 集成测试 + 文档（可并行，依赖 Task 5）

- [x] Task 6: 编写集成测试 `tests/system_agent_test.rs`
  - 新建文件 `crates/agents/agent/tests/system_agent_test.rs`
  - 编写集成测试（至少 10 个）：
    - `test_system_agent_start_and_stop_agent` — 启动 + 停止 Agent
    - `test_system_agent_suspend_and_resume` — 挂起 + 恢复 Agent
    - `test_system_agent_tick_heartbeat_crash_recovery` — tick 检测心跳故障并恢复
    - `test_system_agent_tick_oom_suspends_lowest_priority` — OOM 挂起最低优先级 Agent
    - `test_system_agent_tick_overheat_event` — 过热事件
    - `test_system_agent_get_system_stats` — 系统统计
    - `test_system_agent_find_oom_victim` — OOM victim 选择
    - `test_system_agent_multiple_ticks` — 多周期 tick
    - `test_system_agent_start_registers_heartbeat` — start_agent 注册心跳
    - `test_system_agent_stop_unregisters` — stop_agent 注销心跳和注册
  - 需要创建 mock `AgentFactory` 和 mock `ResourceSource` 用于测试
  - 验证：`cargo test -p eneros-agent --test system_agent_test` 通过

- [x] Task 7: 编写设计文档 `docs/agents/system-agent-design.md`
  - 新建文件 `docs/agents/system-agent-design.md`
  - 文档结构（14 章）：
    1. 概述（v0.41.0 目标）
    2. 背景与动机（Agent Runtime 基础设施 → System Agent 管理层）
    3. 架构设计（SystemAgent + ResourceMonitor + Manager 三层架构图，mermaid）
    4. SystemAgent 数据结构（7 字段）
    5. ResourceMonitor 资源监控（ResourceSource trait + 4 字段）
    6. SystemConfig 配置（3 阈值）
    7. tick 主循环算法（5 步流程图，mermaid）
    8. Agent 管理方法（start/stop/suspend/resume）
    9. OOM 检测与 victim 选择
    10. 过热检测
    11. 故障恢复集成（D11: force_state Error 后 handle_crash）
    12. 错误处理（3 个新错误变体）
    13. no_std 合规性 + 偏差声明表（D1~D11）
    14. 测试覆盖
  - 验证：文档存在且包含 mermaid 图

## Wave 6: 版本同步 + 构建验证（依赖所有任务）

- [x] Task 8: 同步版本标识符 0.40.0 → 0.41.0
  - 修改 `Cargo.toml`（workspace 根）：`version = "0.40.0"` → `version = "0.41.0"`
  - 修改 `Makefile`：`VERSION := 0.40.0` → `VERSION := 0.41.0` + 注释版本号
  - 修改 `.github/workflows/ci.yml`：版本字符串 0.40.0 → 0.41.0
  - 修改 `ci/src/gate.rs`：版本字符串 0.40.0 → 0.41.0（2 处注释）
  - 验证：`grep -r "0.40.0" --include="*.toml" --include="*.yml" --include="*.rs" --include="Makefile" .` 仅剩历史 spec 文档

- [x] Task 9: 完整构建验证
  - 执行 `cargo fmt --all -- --check` 验证格式
  - 执行 `cargo clippy -p eneros-agent --all-targets -- -D warnings` 验证 lint
  - 执行 `cargo test -p eneros-agent` 验证所有单元测试通过
  - 执行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 验证 workspace 回归
  - 执行 WSL2 交叉编译：`wsl bash -c "cd /mnt/e/eneros && cargo build -p eneros-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"`
  - 执行 `cargo deny check licenses bans sources` 验证许可证
  - 执行 `cargo deny check advisories`（已知环境问题，记录但不阻塞）
  - 记录测试数量和构建时间

# Task Dependencies

- Task 1（error.rs）→ Task 3（mod.rs 依赖新错误变体）
- Task 2（monitor.rs）→ Task 3（mod.rs 依赖 ResourceMonitor / SystemConfig / SystemEvent）
- Task 3（mod.rs）→ Task 4（manager.rs 为 SystemAgent 实现 impl 块）
- Task 3 + Task 4 → Task 5（lib.rs 集成）
- Task 5 → Task 6（集成测试依赖模块声明）
- Task 5 → Task 7（设计文档依赖最终 API）
- Task 5 + Task 6 + Task 7 → Task 8（版本同步）
- Task 8 → Task 9（构建验证依赖所有完成）
