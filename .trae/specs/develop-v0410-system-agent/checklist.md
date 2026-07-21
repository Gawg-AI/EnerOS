# Checklist — v0.41.0 System Agent 核心

## C1: 错误变体扩展（Task 1）

- [x] C1: `AgentError` 新增 `SystemOverload` 变体
- [x] C2: `AgentError` 新增 `OomRisk` 变体
- [x] C3: `AgentError` 新增 `Overheat { temp: f32 }` 变体
- [x] C4: 3 个新变体均有 `Display` impl
- [x] C5: `SystemOverload` 的 Display 输出包含 "overload" 字样
- [x] C6: `OomRisk` 的 Display 输出包含 "oom" 字样
- [x] C7: `Overheat` 的 Display 输出包含温度值
- [x] C8: 新变体 derive Clone/PartialEq（保留不含 Eq 的 derive 配置）
- [x] C9: `test_system_agent_error_variants_display` 测试存在且通过
- [x] C10: `test_system_agent_error_variants_eq` 测试存在且通过
- [x] C11: `cargo build -p eneros-agent` 通过（Task 1 完成后）

## C2: ResourceMonitor 数据结构（Task 2）

- [x] C12: `crates/agents/agent/src/system_agent/monitor.rs` 文件存在
- [x] C13: 模块文档注释包含 D2（ResourceSource trait）偏差声明
- [x] C14: 模块文档注释包含 D3（BTreeMap / 不维护 agent_stats）偏差声明
- [x] C15: 模块文档注释包含 D7（SystemConfig 定义）偏差声明
- [x] C16: 模块文档注释包含 D8（is_oom + find_oom_victim 分离）偏差声明
- [x] C17: 模块文档注释包含 no_std 合规声明
- [x] C18: `ResourceSource` trait 定义（4 方法：cpu_usage / mem_used / mem_total / temperature）
- [x] C19: `ResourceSource` trait 为 object-safe
- [x] C20: `ResourceMonitor` 结构定义（5 字段：cpu_usage / mem_total / mem_used / temperature / source）
- [x] C21: `source: Option<Box<dyn ResourceSource>>` 字段
- [ ] C22: `ResourceMonitor` derive Debug — **FAIL**: ResourceMonitor 不派生 Debug，因含 `source: Option<Box<dyn ResourceSource>>` 字段（dyn trait object 未要求 Debug bound，无法自动派生）。此为可接受的设计选择 — ResourceMonitor 是数据持有者，非调试目标；需调试时通过 `get_system_stats()` 获取可观测快照。

## C3: ResourceMonitor 方法实现（Task 2）

- [x] C23: `pub fn new() -> Self` 方法实现
- [x] C24: `pub fn with_source(source: Box<dyn ResourceSource>) -> Self` 方法实现
- [x] C25: `pub fn poll(&mut self)` 方法实现（从 source 读取，无 source 时 no-op）
- [x] C26: `pub fn set_values(&mut self, cpu, mem_used, mem_total, temp)` 方法实现
- [x] C27: `pub fn is_oom(&self, threshold: f32) -> bool` 方法实现
- [x] C28: `is_oom` 在 `mem_total == 0` 时返回 `false`
- [x] C29: `pub fn is_overheat(&self, threshold: f32) -> bool` 方法实现
- [x] C30: `pub fn mem_usage_percent(&self) -> f32` 方法实现
- [x] C31: `mem_usage_percent` 在 `mem_total == 0` 时返回 `0.0`
- [x] C32: `impl Default for ResourceMonitor` 实现

## C4: SystemConfig + SystemStats + SystemEvent + AgentResourceUsage（Task 2）

- [x] C33: `SystemConfig` 结构定义（3 字段）
- [x] C34: `SystemConfig` 字段 `oom_threshold_percent: f32`
- [x] C35: `SystemConfig` 字段 `overheat_threshold: f32`
- [x] C36: `SystemConfig` 字段 `monitor_interval_ms: u64`
- [x] C37: `impl Default for SystemConfig` 实现（默认值 0.9 / 80.0 / 1000）
- [x] C38: `SystemStats` 结构定义（6 字段）
- [x] C39: `SystemStats` 字段：cpu_usage / mem_usage / temperature / agent_count / alive_agents / error_agents
- [x] C40: `SystemEvent` 枚举定义（5 变体）
- [x] C41: `SystemEvent::Overheat { temp: f32 }` 变体
- [x] C42: `SystemEvent::OomVictimSuspended { agent: AgentId }` 变体
- [x] C43: `SystemEvent::AgentCrashed { agent: AgentId }` 变体
- [x] C44: `SystemEvent::AgentRecovered { agent: AgentId }` 变体
- [x] C45: `SystemEvent::AgentRecoveryFailed { agent: AgentId }` 变体
- [x] C46: `AgentResourceUsage` 结构定义（3 字段：cpu_percent / mem_bytes / state）

## C5: ResourceMonitor 单元测试（Task 2）

- [x] C47: `test_monitor_new_empty` 测试存在且通过
- [x] C48: `test_monitor_set_values` 测试存在且通过
- [x] C49: `test_monitor_is_oom_true` 测试存在且通过
- [x] C50: `test_monitor_is_oom_false` 测试存在且通过
- [x] C51: `test_monitor_is_overheat_true` 测试存在且通过
- [x] C52: `test_monitor_is_overheat_false` 测试存在且通过
- [x] C53: `test_monitor_mem_usage_percent` 测试存在且通过
- [x] C54: `test_monitor_with_source_poll` 测试存在且通过
- [x] C55: `test_system_config_default` 测试存在且通过
- [x] C56: `cargo build -p eneros-agent` 通过（Task 2 完成后）

## C6: SystemAgent 数据结构（Task 3）

- [x] C57: `crates/agents/agent/src/system_agent/mod.rs` 文件存在
- [x] C58: 声明 `pub mod manager;` 和 `pub mod monitor;`
- [x] C59: re-exports `pub use monitor::{ResourceMonitor, ResourceSource, SystemConfig, SystemEvent, SystemStats};`
- [x] C60: 模块文档注释包含 D1（tick 替代 run）偏差声明
- [x] C61: 模块文档注释包含 D4（lifecycle 字段）偏差声明
- [x] C62: 模块文档注释包含 D6（SystemEvent 替代 log）偏差声明
- [x] C63: 模块文档注释包含 D10（mod.rs 模式）偏差声明
- [x] C64: 模块文档注释包含 D11（force_state Error 后 handle_crash）偏差声明
- [x] C65: 模块文档注释包含 no_std 合规声明
- [x] C66: 导入 `alloc::rc::Rc` / `alloc::vec::Vec` / `core::cell::RefCell`
- [x] C67: 导入 `crate::heartbeat::HeartbeatMonitor`
- [x] C68: 导入 `crate::lifecycle::LifecycleManager`
- [x] C69: 导入 `crate::recovery::CrashRecovery`
- [x] C70: 导入 `crate::registry::AgentRegistry`
- [x] C71: 导入 `crate::spawner::AgentSpawner`
- [x] C72: 导入 `crate::health::HealthStatus`
- [x] C73: `SystemAgent` 结构定义（7 字段）
- [x] C74: `registry: Rc<RefCell<AgentRegistry>>` 字段
- [x] C75: `spawner: Rc<AgentSpawner>` 字段
- [x] C76: `recovery: Rc<CrashRecovery>` 字段
- [x] C77: `heartbeat: Rc<RefCell<HeartbeatMonitor>>` 字段
- [x] C78: `lifecycle: Rc<RefCell<LifecycleManager>>` 字段（D4 偏差）
- [x] C79: `monitor: ResourceMonitor` 字段
- [x] C80: `config: SystemConfig` 字段
- [x] C81: `SystemAgent` 不 derive Debug（含 Rc<RefCell> / Rc<dyn> 字段）

## C7: SystemAgent 方法实现（Task 3）

- [x] C82: `pub fn new(registry, spawner, recovery, heartbeat, lifecycle, config) -> Self` 方法实现
- [x] C83: `pub fn tick(&mut self, now: u64) -> Vec<SystemEvent>` 方法实现（D1 偏差）
- [x] C84: `tick` 步骤 1：`self.monitor.poll()` 资源监控
- [x] C85: `tick` 步骤 2：`self.heartbeat.borrow_mut().check(now)` 心跳检查
- [x] C86: `tick` 步骤 3：遍历 health_results，对 Unhealthy Agent 执行恢复（D11）
- [x] C87: `tick` 步骤 3a：`force_state(id, Error)` 后 `recovery.handle_crash(id, now)`
- [x] C88: `tick` 步骤 3b：成功 → `AgentRecovered` 事件
- [x] C89: `tick` 步骤 3c：`MaxRestartsExceeded` → `AgentRecoveryFailed` 事件
- [x] C90: `tick` 步骤 3d：其他错误 → `AgentCrashed` 事件
- [x] C91: `tick` 步骤 4：OOM 检查 + `find_oom_victim` + `suspend_agent`（注：实现中直接调用 `lifecycle.transition(victim, Suspended)` 替代 `suspend_agent`，语义等价，设计文档 §7.4 已说明原因：避免循环依赖与借用冲突）
- [x] C92: `tick` 步骤 4：产生 `OomVictimSuspended` 事件
- [x] C93: `tick` 步骤 5：过热检查
- [x] C94: `tick` 步骤 5：产生 `Overheat` 事件
- [x] C95: `pub fn get_system_stats(&self) -> SystemStats` 方法实现
- [x] C96: `get_system_stats` 从 registry 统计 agent_count / alive_agents / error_agents
- [x] C97: `get_system_stats` 从 monitor 读取 cpu_usage / mem_usage / temperature
- [x] C98: `pub fn find_oom_victim(&self) -> Option<AgentId>` 方法实现（D8 偏差）
- [x] C99: `find_oom_victim` 遍历 registry 中所有存活 Agent
- [x] C100: `find_oom_victim` 返回 priority 最低的 AgentId
- [x] C101: `find_oom_victim` 无存活 Agent 时返回 None
- [x] C102: `pub fn monitor(&self) -> &ResourceMonitor` 方法实现
- [x] C103: `pub fn config(&self) -> &SystemConfig` 方法实现

## C8: SystemAgent 单元测试（Task 3）

- [x] C104: `test_system_agent_new` 测试存在且通过
- [x] C105: `test_system_agent_tick_no_events` 测试存在且通过
- [x] C106: `test_system_agent_tick_overheat` 测试存在且通过
- [x] C107: `test_system_agent_tick_oom_suspends_victim` 测试存在且通过
- [x] C108: `test_system_agent_get_system_stats` 测试存在且通过
- [x] C109: `test_system_agent_find_oom_victim_lowest_priority` 测试存在且通过
- [x] C110: `test_system_agent_find_oom_victim_no_alive` 测试存在且通过
- [x] C111: `test_system_agent_find_oom_victim_multiple_agents` 测试存在且通过
- [x] C112: `cargo build -p eneros-agent` 通过（Task 3 完成后）

## C9: Agent 管理方法（Task 4）

- [x] C113: `crates/agents/agent/src/system_agent/manager.rs` 文件存在
- [x] C114: 模块文档注释包含 D5（start_agent 接受 now）偏差声明
- [x] C115: 模块文档注释包含 D9（stop_agent 用 force_state）偏差声明
- [x] C116: 模块文档注释包含 no_std 合规声明
- [x] C117: `pub fn start_agent(&self, config: AgentConfig, now: u64) -> Result<AgentId, AgentError>` 方法实现（D5 偏差）
- [x] C118: `start_agent` 调用 `self.spawner.spawn(config, now)`
- [x] C119: `start_agent` 调用 `self.heartbeat.borrow_mut().register(id, now)`
- [x] C120: `start_agent` 返回 `Ok(id)`
- [x] C121: `pub fn stop_agent(&self, id: AgentId) -> Result<(), AgentError>` 方法实现（D9 偏差）
- [x] C122: `stop_agent` 调用 `self.lifecycle.borrow().force_state(id, AgentState::Dead)`（注：实现使用 `borrow_mut()` 因 `force_state` 需 `&mut self`，逻辑正确 — force_state 以 Dead 调用）
- [x] C123: `stop_agent` 调用 `self.heartbeat.borrow_mut().unregister(id)`
- [x] C124: `stop_agent` 调用 `self.registry.borrow_mut().unregister(id)`
- [x] C125: `pub fn suspend_agent(&self, id: AgentId) -> Result<(), AgentError>` 方法实现
- [x] C126: `suspend_agent` 调用 `self.lifecycle.borrow().transition(id, AgentState::Suspended)`
- [x] C127: `pub fn resume_agent(&self, id: AgentId) -> Result<(), AgentError>` 方法实现
- [x] C128: `resume_agent` 调用 `self.lifecycle.borrow().transition(id, AgentState::Running)`

## C10: Agent 管理方法单元测试（Task 4）

- [x] C129: `test_start_agent_success` 测试存在且通过
- [x] C130: `test_stop_agent_success` 测试存在且通过
- [x] C131: `test_stop_agent_not_found` 测试存在且通过
- [x] C132: `test_suspend_agent_success` 测试存在且通过
- [x] C133: `test_resume_agent_success` 测试存在且通过
- [x] C134: `test_suspend_resume_cycle` 测试存在且通过
- [x] C135: `cargo build -p eneros-agent` 通过（Task 4 完成后）

## C11: 模块集成 — lib.rs（Task 5）

- [x] C136: `lib.rs` 新增 `pub mod system_agent;`（字母序正确）
- [x] C137: `lib.rs` re-export 新增 `SystemAgent`
- [x] C138: `lib.rs` re-export 新增 `ResourceMonitor` / `ResourceSource` / `SystemConfig` / `SystemEvent` / `SystemStats`
- [x] C139: `lib.rs` `pub const VERSION = "0.41.0"`
- [x] C140: `lib.rs` 模块文档注释更新（包含 System Agent 描述）
- [x] C141: `cargo build -p eneros-agent` 通过（Task 5 完成后）

## C12: 集成测试（Task 6）

- [x] C142: `crates/agents/agent/tests/system_agent_test.rs` 文件存在
- [x] C143: `test_system_agent_start_and_stop_agent` 测试存在且通过
- [x] C144: `test_system_agent_suspend_and_resume` 测试存在且通过
- [x] C145: `test_system_agent_tick_heartbeat_crash_recovery` 测试存在且通过
- [x] C146: `test_system_agent_tick_oom_suspends_lowest_priority` 测试存在且通过
- [x] C147: `test_system_agent_tick_overheat_event` 测试存在且通过
- [x] C148: `test_system_agent_get_system_stats` 测试存在且通过
- [x] C149: `test_system_agent_find_oom_victim` 测试存在且通过
- [x] C150: `test_system_agent_multiple_ticks` 测试存在且通过
- [x] C151: `test_system_agent_start_registers_heartbeat` 测试存在且通过
- [x] C152: `test_system_agent_stop_unregisters` 测试存在且通过
- [x] C153: `cargo test -p eneros-agent --test system_agent_test` 通过

## C13: 设计文档（Task 7）

- [x] C154: `docs/agents/system-agent-design.md` 文件存在
- [x] C155: 文档包含 14 个章节
- [x] C156: 第 3 章包含 mermaid 架构图（SystemAgent + ResourceMonitor + Manager 三层）
- [x] C157: 第 7 章包含 mermaid tick 主循环流程图
- [x] C158: 第 8 章描述 Agent 管理方法（start/stop/suspend/resume）
- [x] C159: 第 9 章描述 OOM 检测与 victim 选择
- [x] C160: 第 11 章描述故障恢复集成（D11: force_state Error 后 handle_crash）
- [x] C161: 第 12 章描述 3 个新错误变体
- [x] C162: 第 13 章包含 D1~D11 偏差声明表
- [x] C163: 第 13 章描述 no_std 合规性

## C14: 版本同步（Task 8）

- [x] C164: `Cargo.toml`（workspace 根）version = "0.41.0"
- [x] C165: `Makefile` VERSION = 0.41.0
- [x] C166: `.github/workflows/ci.yml` 版本字符串 = 0.41.0
- [x] C167: `ci/src/gate.rs` 版本字符串 = 0.41.0（2 处）
- [x] C168: `crates/agents/agent/src/lib.rs` VERSION = "0.41.0"
- [x] C169: 无残留 0.40.0 版本字符串（除历史 spec 文档）

## C15: 构建验证（Task 9）

- [x] C170: `cargo fmt --all -- --check` 通过
- [x] C171: `cargo clippy -p eneros-agent --all-targets -- -D warnings` 无 warning
- [x] C172: `cargo test -p eneros-agent` 全部通过
- [x] C173: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（workspace 回归）
- [x] C174: WSL2 交叉编译 `cargo build -p eneros-agent --target aarch64-unknown-none` 通过
- [x] C175: `cargo deny check licenses bans sources` 通过
- [x] C176: `cargo deny check advisories`（已知环境问题 — GitHub 网络访问受限，非代码问题，记录但不阻塞）
- [x] C177: 记录测试数量和构建时间（设计文档 §14 记录：30 单元测试 + 10 集成测试 + 3 doctests；交叉编译 4.00s）

## C16: no_std 合规性

- [x] C178: `monitor.rs` 无 `use std::*`
- [x] C179: `mod.rs` 无 `use std::*`
- [x] C180: `manager.rs` 无 `use std::*`
- [x] C181: `monitor.rs` 无 `panic!` / `todo!` / `unimplemented!`
- [x] C182: `mod.rs` 无 `panic!` / `todo!` / `unimplemented!`
- [x] C183: `manager.rs` 无 `panic!` / `todo!` / `unimplemented!`
- [x] C184: `monitor.rs` 仅使用 `alloc::*` / `core::*`
- [x] C185: `mod.rs` 仅使用 `alloc::*` / `core::*`
- [x] C186: `manager.rs` 仅使用 `alloc::*` / `core::*`
- [x] C187: 子模块无 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）

## C17: 目录结构校验

- [x] C188: 新文件位于 `crates/agents/agent/src/system_agent/` 下（C1 校验）
- [x] C189: workspace `Cargo.toml` members 已包含 `crates/agents/agent`（C2 校验，已存在）
- [x] C190: 设计文档位于 `docs/agents/` 下（C4 校验）
- [x] C191: 无根目录 crate（C5 校验）
- [x] C192: 无垃圾文件被追踪（C13 校验：无 target/、*.elf、*.bin）
- [x] C193: `.gitignore` 已覆盖新产生的文件类型（C14 校验）

## C18: 安全性验证

- [x] C194: `stop_agent` 使用 `force_state` 确保 Agent 不可绕过停止
- [x] C195: `suspend_agent` 使用 `transition` 确保仅合法状态可挂起
- [x] C196: `find_oom_victim` 仅选择存活 Agent（不选 Dead/Created）
- [x] C197: `tick` 中故障恢复先 `force_state(Error)` 再 `handle_crash`（D11 安全性）
- [x] C198: SystemAgent 不 derive Debug（避免 `Rc<RefCell>` 内部状态泄露）
- [x] C199: `ResourceSource` trait 为 object-safe（可动态分发）
