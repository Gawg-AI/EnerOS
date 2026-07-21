# Checklist — v0.37.0 Agent 心跳与健康检查

> **验证清单**：所有检查项必须通过才能标记版本完成。
> **回归保护**：workspace 已有测试（v0.31.0~v0.36.0）必须全部继续通过。
> **验证方式**：逐项检查代码 / 运行命令 / 审查文档。

## 一、目录结构校验

- [x] **C1 health.rs 位置**：`crates/agents/agent/src/health.rs` 存在
- [x] **C2 heartbeat.rs 位置**：`crates/agents/agent/src/heartbeat.rs` 存在
- [x] **C3 集成测试位置**：`crates/agents/agent/tests/heartbeat_test.rs` 存在
- [x] **C4 文档分类**：`docs/agents/agent-heartbeat-design.md` 在 `docs/agents/` 子目录下
- [x] **C5 无根目录 crate**：仓库根目录无新增 Rust crate 文件夹

## 二、代码结构校验

- [x] **C6 health.rs 存在**：`health.rs` 文件存在
- [x] **C7 heartbeat.rs 存在**：`heartbeat.rs` 文件存在
- [x] **C8 lib.rs 模块声明**：`lib.rs` 包含 `pub mod health;` 和 `pub mod heartbeat;`
- [x] **C9 lib.rs re-export**：`lib.rs` 包含 `pub use health::{HealthCheck, HealthStatus};` 和 `pub use heartbeat::{HeartbeatMonitor, HeartbeatState};`
- [x] **C10 no_std 声明**：`lib.rs` 仍有 `#![cfg_attr(not(test), no_std)]`（未改动）
- [x] **C11 extern crate alloc**：`lib.rs` 仍有 `extern crate alloc;`（未改动）
- [x] **C12 零外部依赖**：`Cargo.toml` 的 `[dependencies]` 仍为空
- [x] **C13 VERSION 常量**：`lib.rs` 有 `pub const VERSION: &str = "0.37.0";`

## 三、AgentError 扩展校验

- [x] **C14 HeartbeatTimeout 变体**：`error.rs` 包含 `HeartbeatTimeout { agent_id: AgentId, missed: u32 }`
- [x] **C15 AgentUnhealthy 变体**：`error.rs` 包含 `AgentUnhealthy { agent_id: AgentId }`
- [x] **C16 Display 实现**：2 个新变体的 Display 输出含 "heartbeat timeout" / "agent unhealthy"
- [x] **C17 既有变体未改动**：11 个既有变体（含 v0.36.0 的 CodeLoadFailed/InitFailed/StartFailed）及其 Display 保持不变
- [x] **C18 新变体测试**：tests 模块包含 HeartbeatTimeout/AgentUnhealthy 的 Display + clone/eq 测试
- [x] **C19 AgentId import**：`error.rs` 追加了 `use crate::id::AgentId;`

## 四、HealthStatus 结构校验

- [x] **C20 枚举定义**：`health.rs` 定义 `pub enum HealthStatus` 含 4 变体（Healthy / Degraded / Unhealthy / Dead）
- [x] **C21 derive**：`#[derive(Clone, Copy, Debug, PartialEq, Eq)]`（D3 偏差）
- [x] **C22 变体文档**：每个变体有文档注释说明语义

## 五、HealthCheck trait 校验

- [x] **C23 trait 定义**：`health.rs` 定义 `pub trait HealthCheck` 含 `check_health` 方法
- [x] **C24 方法签名**：`fn check_health(&self) -> HealthStatus`
- [x] **C25 object-safe**：trait 无泛型方法、无 Self 类型参数、无关联函数（支持 `dyn HealthCheck`）

## 六、HeartbeatState 结构校验

- [x] **C26 结构体定义**：`heartbeat.rs` 定义 `pub struct HeartbeatState` 含 4 字段（last_heartbeat / missed_count / status / interval_ms）
- [x] **C27 derive**：`#[derive(Clone, Debug)]`（D4 偏差）
- [x] **C28 字段类型正确**：last_heartbeat: u64 / missed_count: u32 / status: HealthStatus / interval_ms: u64
- [x] **C29 字段可见性**：4 字段均为 `pub`

## 七、HeartbeatMonitor 结构校验

- [x] **C30 结构体定义**：`heartbeat.rs` 定义 `pub struct HeartbeatMonitor` 含 3 字段（agents / default_interval_ms / max_missed）
- [x] **C31 derive**：`#[derive(Debug)]`（D4 偏差）
- [x] **C32 字段类型正确**：
  - `agents: BTreeMap<AgentId, HeartbeatState>`（D1 偏差，非 HashMap）
  - `default_interval_ms: u64`
  - `max_missed: u32`
- [x] **C33 字段可见性**：`agents` 为私有（通过方法访问），`default_interval_ms` / `max_missed` 可私有或 pub

## 八、HeartbeatMonitor API 校验

- [x] **C34 new() 方法**：`pub fn new(interval_ms: u64, max_missed: u32) -> Self`
- [x] **C35 register() 方法**：签名 `(&mut self, id: AgentId, now: u64)`（D2 偏差：追加 now 参数）
- [x] **C36 heartbeat() 方法**：签名 `(&mut self, id: AgentId, timestamp: u64)`
- [x] **C37 check() 方法**：签名 `(&mut self, now: u64) -> Vec<(AgentId, HealthStatus)>`
- [x] **C38 is_healthy() 方法**：签名 `(&self, id: AgentId) -> bool`
- [x] **C39 set_interval() 方法**：签名 `(&mut self, id: AgentId, interval_ms: u64)`
- [x] **C40 unregister() 方法**：签名 `(&mut self, id: AgentId)`

## 九、check 算法校验

- [x] **C41 saturating_sub 防溢出**：`now.saturating_sub(state.last_heartbeat)`（§8.3 时钟回拨）
- [x] **C42 elapsed > interval 判定**：使用 `>` 而非 `>=`（边界：恰好等于 interval 不算超时）
- [x] **C43 missed_count 计算**：`(elapsed / state.interval_ms) as u32`
- [x] **C44 Degraded 判定**：`missed_count > 0 && missed_count < max_missed`
- [x] **C45 Unhealthy 判定**：`missed_count >= max_missed`（D7：不设 Dead）
- [x] **C46 返回 Vec**：`results.push((id, state.status))` 返回所有 Agent 状态

## 十、heartbeat 行为校验

- [x] **C47 heartbeat 重置 last_heartbeat**：`state.last_heartbeat = timestamp`
- [x] **C48 heartbeat 重置 missed_count**：`state.missed_count = 0`
- [x] **C49 heartbeat 设置 Healthy**：`state.status = HealthStatus::Healthy`
- [x] **C50 heartbeat 未注册 agent**：静默忽略（if let Some）

## 十一、register 行为校验

- [x] **C51 register 初始化 last_heartbeat**：`last_heartbeat = now`（D2：now 参数）
- [x] **C52 register 初始化 missed_count**：`missed_count = 0`
- [x] **C53 register 初始化 status**：`status = HealthStatus::Healthy`
- [x] **C54 register 初始化 interval_ms**：`interval_ms = self.default_interval_ms`

## 十二、默认常量校验

- [x] **C55 DEFAULT_INTERVAL_MS**：`const DEFAULT_INTERVAL_MS: u64 = 1000;`
- [x] **C56 DEFAULT_MAX_MISSED**：`const DEFAULT_MAX_MISSED: u32 = 3;`

## 十三、no_std 合规校验

- [x] **C57 无 use std::**：`health.rs` 和 `heartbeat.rs` 中搜索 `use std::` 返回 0 匹配
- [x] **C58 无 panic 宏违规**：非测试代码中无 `panic!` / `todo!` / `unimplemented!`
- [x] **C59 子模块无 no_std 重复**：`health.rs` 和 `heartbeat.rs` 不包含 `#![cfg_attr(not(test), no_std)]`
- [x] **C60 aarch64 交叉编译**：`cargo build -p eneros-agent --target aarch64-unknown-none` 通过

## 十四、测试校验

- [x] **C61 health 单元测试**：`health.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C62 HealthStatus Clone/Copy 测试**：验证可 Clone/Copy
- [x] **C63 HealthStatus Debug/Eq 测试**：验证 Debug 输出 + PartialEq/Eq
- [x] **C64 HealthCheck object-safe 测试**：`Box<dyn HealthCheck>` 装箱并调用方法
- [x] **C65 heartbeat 单元测试**：`heartbeat.rs` 包含 `#[cfg(test)] mod tests`
- [x] **C66 register 测试**：register 后 is_healthy == true
- [x] **C67 heartbeat 更新测试**：heartbeat 后状态重置为 Healthy
- [x] **C68 check Healthy 测试**：elapsed <= interval → Healthy
- [x] **C69 check Degraded 测试**：1+ missed < max → Degraded
- [x] **C70 check Unhealthy 测试**：missed >= max → Unhealthy
- [x] **C71 is_healthy 未注册测试**：未注册 agent → false
- [x] **C72 set_interval 测试**：per-Agent 间隔覆盖生效
- [x] **C73 unregister 测试**：注销后不在 check 结果中
- [x] **C74 多 Agent 独立测试**：多个 agent 各自独立监控
- [x] **C75 时钟回拨测试**：saturating_sub 防溢出
- [x] **C76 边界测试**：elapsed == interval 不触发超时（> 而非 >=）
- [x] **C77 集成测试存在**：`tests/heartbeat_test.rs` 存在且通过
- [x] **C78 集成测试完整生命周期**：register → heartbeat → check(Healthy) → 停止 → check(Degraded) → check(Unhealthy)
- [x] **C79 集成测试恢复**：Degraded → heartbeat → Healthy
- [x] **C80 集成测试多 Agent**：3 个 agent 独立心跳/超时
- [x] **C81 集成测试 set_interval**：per-Agent 间隔影响检测时机
- [x] **C82 集成测试 HealthCheck**：Box<dyn HealthCheck> 可调用
- [x] **C83 集成测试时钟回拨**：saturating_sub 安全
- [x] **C84 测试覆盖率**：≥ 80%（蓝图 §6.1）

## 十五、版本标识一致性

- [x] **C85 根 Cargo.toml**：`version = "0.37.0"`
- [x] **C86 Makefile**：`VERSION := 0.37.0`
- [x] **C87 ci.yml**：`Version: v0.37.0`
- [x] **C88 gate.rs**：注释含 v0.37.0
- [x] **C89 lib.rs VERSION**：`VERSION = "0.37.0"`
- [x] **C90 无 0.36.0 残留**：grep "0.36.0" 无版本标识残留（历史注释除外）

## 十六、构建与质量校验

- [x] **C91 cargo fmt**：`cargo fmt --all -- --check` 通过
- [x] **C92 cargo clippy**：`cargo clippy -p eneros-agent --all-targets -- -D warnings` 无警告
- [x] **C93 cargo test (agent)**：`cargo test -p eneros-agent` 全部通过
- [x] **C94 workspace 回归**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C95 eneros-ci**：`cargo run -p eneros-ci` fmt/clippy/test PASS（audit 可能因网络失败 — 已知问题）
- [x] **C96 cargo deny**：`cargo deny check licenses bans sources` 通过

## 十七、文档校验

- [x] **C97 设计文档存在**：`docs/agents/agent-heartbeat-design.md` 存在
- [x] **C98 文档内容完整**：版本目标 / 架构定位 / check 算法图 / 数据结构 / 模块结构 / D1~D7 偏差 / 错误处理 / 心跳协议 / 独立监控器 / Dead 归属 / 时钟回拨 / 性能 / 后续解锁
- [x] **C99 文档位置正确**：在 `docs/agents/` 子目录下

## 十八、偏差声明记录

- [x] **C100 D1 偏差记录**：BTreeMap 代替 HashMap，文档记录
- [x] **C101 D2 偏差记录**：register 追加 now: u64（no_std 时间约定），文档记录
- [x] **C102 D3 偏差记录**：HealthStatus derives（Clone/Copy/Debug/PartialEq/Eq），文档记录
- [x] **C103 D4 偏差记录**：HeartbeatState/HeartbeatMonitor derives，文档记录
- [x] **C104 D5 偏差记录**：2 个新错误变体，文档记录
- [x] **C105 D6 偏差记录**：HeartbeatMonitor 独立运行（不引用 registry/lifecycle），文档记录
- [x] **C106 D7 偏差记录**：check 设 Unhealthy 而非 Dead（Dead 由 v0.38.0 设置），文档记录

## 十九、蓝图合规校验

- [x] **C107 接口完备性**：蓝图 §3 + §4.2 的所有方法（new/register/heartbeat/check/is_healthy/set_interval/unregister）全部实现
- [x] **C108 HealthCheck trait**：蓝图 §3 的 check_health 实现
- [x] **C109 HealthStatus 4 变体**：蓝图 §3 的 Healthy/Degraded/Unhealthy/Dead 全部实现
- [x] **C110 蓝图 §4.3 check 算法**：mermaid 流程实现
- [x] **C111 蓝图 §4.4 错误变体**：HeartbeatTimeout / AgentUnhealthy 实现
- [x] **C112 蓝图 §6.2 超时检测**：模拟心跳超时 → Unhealthy
- [x] **C113 蓝图 §6.3 检测延迟 <3s**：3 次超时（3s）→ Unhealthy
- [x] **C114 蓝图 §8.3 时钟回拨**：saturating_sub 防误判
- [x] **C115 蓝图 §9.1 功能**：心跳/超时/健康状态
- [x] **C116 蓝图 §9.2 性能**：检测延迟 <3s（3 × 1s = 3s）
- [x] **C117 蓝图 §9.4 可靠**：3 次超时 = 故障
- [x] **C118 蓝图 §9.5 可维护**：间隔可配置（set_interval）
- [x] **C119 蓝图 §9.6 可观测**：健康状态可查（is_healthy / check）
- [x] **C120 蓝图 §9.7 可扩展**：支持自定义健康检查（HealthCheck trait）
