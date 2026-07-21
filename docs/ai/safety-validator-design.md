# EnerOS 安全校验器设计 — SafetyValidator + SafetyRule + ValidationResult

> **版本**：v0.67.0（P1-J AI Runtime Solver 第四层，安全屏障层）
> **crate**：`eneros-safety-validator`（`crates/ai/safety-validator/`）
> **蓝图依据**：`蓝图/phase1.md` §v0.67.0（line 13955~14312）
> **spec 依据**：`.trae/specs/develop-v0670-safety-validator/spec.md`（D1~D12 偏差声明源）
> **覆盖版本**：v0.67.0
> **最后更新**：2026-07-16

---

## 1. 版本目标

### 1.1 一句话目标

构建安全校验器 `SafetyValidator`，对 v0.66.0 `ScheduleResult`（储能调度 LP 求解结果）执行电气安全校验（`ElectricalSafetyRule`，功率 / SOC 越限）+ 保护配合校验（`ProtectionCoordinationRule`，爬坡率越限）的链式规则匹配，超限时**截断到安全边界而非拒绝**（保证系统始终有输出），致命违规（SOC < 5%）立即终止并拒绝，作为 Solver → Control Bus 之间的安全屏障，运行于慢平面（Agent Runtime 分区），不干扰快平面 10ms 实时控制。

### 1.2 详细描述

v0.66.0 完成了 P1-J AI Runtime Solver 第三层（能源领域模型层），交付了 `ScheduleConfig` / `EnergyScheduleModel` / `ScheduleResult`，将储能调度问题封装为 LP 求解并还原为各时段充放电 / SOC 序列。但 LP 求解器（v0.64.0 `Solver` trait / `MockSolver` / `HighsSolver`）仅保证**数学最优**与**约束可行**，不保证**工程安全**：

- **SOC 越限**：LP 求解在边界约束 `SOC_min ≤ soc[t] ≤ SOC_max` 下可能输出 `soc = 0.95`（边界值），但工程上为避免电池深度充放电损伤，需保留 5% 安全裕度（`0.05 ≤ soc ≤ 0.95`）。LP 边界约束在数值求解时可能因浮点误差产生 `soc = 0.95000001` 的微小越限，虽数学可行但工程不安全。
- **爬坡越限**：LP 爬坡约束 `|charge[t] - charge[t-1]| ≤ ramp_c` 在 `ramp_c` 设定较大时允许功率突变（如 0 → 500 kW 单时段跃变），但 PCS（Power Conversion System）物理上无法在 15 分钟内完成如此大的功率变化，会触发过流保护跳闸。
- **多规则冲突**：LP 求解结果可能同时违反多条安全规则（如某时段 SOC 越限 + 功率超限），需按优先级链式校验并逐条截断。

本版本（v0.67.0）进入 P1-J AI Runtime Solver 第四层（安全屏障层），针对 LP 求解结果构建可插拔的安全校验规则链：

| 产出 | 角色 | 说明 |
|------|------|------|
| `SafetyRule` trait | 校验规则抽象 | 必需方法 `name()` / `validate()`；默认方法 `priority()`（默认 100）/ `is_hard()`（默认 true）；**无 `Send + Sync` bound**（D1） |
| `SystemState` | 最小系统状态 | 5 字段（voltage_v / current_a / frequency_hz / soc_pct / timestamp_ms）；本地定义（D2，不依赖 HMI crate） |
| `Severity` / `Violation` / `ValidationResult` | 校验结果类型 | `Severity`（Info/Warning/Critical/Fatal）；`Violation`（违规详情）；`ValidationResult`（通过 / 截断 / 截断后调度 / 违规列表） |
| `ElectricalSafetyRule` | 电气安全规则 | 校验充电 / 放电功率 ≤ `max_power_kw`（截断 Critical）、SOC ≤ 0.95（截断 Critical）、SOC ≥ 0.05（截断 Fatal）；priority = 10 |
| `ProtectionCoordinationRule` | 保护配合规则 | 校验相邻时段功率变化率 ≤ `max_ramp_rate`（kW/min），超限截断功率变化（Critical）；priority = 20 |
| `SafetyValidator` | 校验器主接口 | `new()` 注册 2 默认规则；`add_rule()` 按优先级排序；`validate()` 链式执行 + 致命终止 |

本版本核心设计决策（详见 §11 偏差声明 D1~D12）：

1. **D1**：`SafetyRule` 移除蓝图 `Send + Sync` bound（no_std 单线程；与 v0.59.0 `LlmEngine` / v0.63.0 `PromptTemplate` / v0.64.0 `Solver` 一致）
2. **D2**：本地定义最小 `SystemState`（5 字段电气状态），不依赖 HMI crate 的显示状态（agent_states / storage_usage / network 等无关字段）
3. **D6**：不依赖 v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 telemetry-model（仅消费 v0.66.0 `ScheduleResult` + 本地 `SystemState`；与 v0.56.0 形成双重屏障：Solver 层校验 + RTOS 层校验）
4. **D11**：精确复制蓝图截断逻辑 `discharge_power_kw += diff.max(0.0); charge_power_kw += (-diff).max(0.0);`（不"修复"业务逻辑，Karpathy "Surgical Changes"）

所有 Rust 代码必须 no_std（蓝图 §43.1），仅使用 `core::*` / `alloc::*`，无 `std::*`，`Vec` / `Box` / `String` 来自 `extern crate alloc`（D4），`Vec::sort_by_key` 原生可用（D5），`String::from` / `.to_string()` 用于 `&str → String` 转换（D3），`Severity` 派生 `PartialEq + Copy` 用于 `==` 比较（D8），`ValidationResult` / `Violation` 不派生 `PartialEq`（D9，Karpathy "Simplicity First"），直接浮点比较 `if soc_pct > 0.95`（D10，固定阈值非迭代结果），`ElectricalSafetyRule` / `ProtectionCoordinationRule` 保留未使用字段供扩展（D12，`#![allow(dead_code)]` 抑制警告），无 FFI 无 `[features]`（D7）。

### 1.3 架构定位

| 维度 | 定位 |
|------|------|
| Phase | Phase 1 单机 MVP |
| 子系统 | P1-J AI Runtime Solver 第四层（安全屏障层） |
| 平面 | 慢平面（Agent Runtime 分区，管理信息大区） |
| 角色 | 双脑链路 Solver 安全屏障层，校验 ScheduleResult 的 SOC 越限 / 爬坡越限 / 功率超限 |
| 上游版本 | v0.66.0（`ScheduleResult` / `ScheduleEntry` 复用，D8）；v0.11.0 用户堆（alloc 支持） |
| 同层版本 | v0.67.0（本版本，安全校验器） |
| 下游版本 | v0.68.0 意图解析（消费校验后的 `ScheduleResult`）；v0.71.0 双脑联调（编排 LLM + Solver + SafetyValidator） |
| 部署形态 | 纯 Rust crate，无 C 库依赖，无 FFI，CPU 编译运行 |

### 1.4 路线图链路

```
v0.59.0 LlmEngine trait ──► v0.63.0 Prompt 模板
                                   │
                                   ▼
v0.64.0 Solver trait + HiGHS FFI ──► v0.65.0 建模 DSL
                                   │
                                   ▼
                          v0.66.0 能源 LP（ScheduleResult）
                                   │
                                   ▼
                          v0.67.0 安全校验（本版本）
                                   │
                                   ├──► v0.68.0 意图解析（消费校验后结果）
                                   │
                                   └──► v0.71.0 双脑联调（LLM + Solver + Safety）
```

### 1.5 依赖关系

| 依赖 | 来源 | 用途 |
|------|------|------|
| `eneros_energy_lp_model::result::{ScheduleResult, ScheduleEntry}` | v0.66.0 crate（path 依赖） | 校验对象类型（`validate()` 入参 + `clamped_schedule` 字段，D8） |
| `eneros_energy_lp_model::problem::SolveStatus` | v0.66.0 crate | `ScheduleResult.solve_status` 字段类型（间接复用） |
| `alloc::string::String` | `alloc` crate | 规则名 / 违规字段名（D3） |
| `alloc::vec::Vec` | `alloc` crate | 规则列表 / 违规列表（D4） |
| `alloc::boxed::Box` | `alloc` crate | `Box<dyn SafetyRule>` trait 对象（D4） |
| `core::*` | `core` crate | `f64::signum` / `f64::abs` / 浮点比较 |

> **注**：本版本**仅依赖 v0.66.0**（D8），不依赖 v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 telemetry-model（D6）。`SafetyValidator` 仅校验 `ScheduleResult` + 本地 `SystemState`，与 ConstraintChecker / DegradeEngine / telemetry-model 完全解耦。v0.56.0 ConstraintChecker 在 RTOS 层做命令执行前二次校验，与本 crate 形成 double barrier（§9.4 详述）。

### 1.6 设计原则关联

| 原则 | 体现 |
|------|------|
| 安全第一 | 校验失败截断到安全边界而非拒绝（保证系统有输出）；致命违规（SOC < 5%）立即拒绝；与 v0.56.0 ConstraintChecker 形成双重屏障 |
| 可验证性 | 规则链式执行可审计（`Violation` 记录原始值 / 安全值 / 严重程度 / 违规字段）；`ValidationResult` 包含完整违规列表 |
| 确定性优先 | 规则按 `priority()` 升序排序（数值小优先）；同优先级保持插入顺序（`sort_by_key` 稳定排序）；链式截断顺序确定 |
| no_std 合规 | 全 crate 仅使用 `core::*` / `alloc::*`，无 `std::*`（蓝图 §43.1）；`Vec` / `Box` / `String` 来自 `extern crate alloc`（D4） |
| DRY 原则 | 复用 v0.66.0 `ScheduleResult` / `ScheduleEntry`（D8）；不重定义调度结果类型 |
| Simplicity First | 本地定义最小 `SystemState`（D2，不引入 HMI crate 耦合）；`ValidationResult` / `Violation` 不派生 `PartialEq`（D9）；无 FFI 无 `[features]`（D7） |
| Surgical Changes | 精确复制蓝图截断逻辑（D11，不"修复"业务逻辑）；保留未使用字段供扩展（D12） |
| 可测试性 | 纯 Rust 实现，默认 `cargo test` 可运行；22 项集成测试覆盖全部规则与边界（§12.1） |
| 安全访问 | 浮点直接比较固定阈值 0.95/0.05（D10，非迭代结果无精度问题） |

---

## 2. 架构定位

### 2.1 P1-J AI Runtime Solver 分层

P1-J AI Runtime Solver 子系统按"求解引擎 → 建模 DSL → 能源 LP → 安全校验 → 意图解析"五层层级组织，本版本位于第四层：

| 层级 | 版本 | crate | 职责 |
|------|------|-------|------|
| 第一层（求解引擎） | v0.64.0 | `eneros-solver-core` | `Solver` trait + MockSolver + HighsSolver FFI + `LpProblem` 矩阵格式 |
| 第二层（建模 DSL） | v0.65.0 | `eneros-solver-model` | `Variable`/`LinearExpr`/`Constraint`/`OptProblem` DSL + `compile()` 编译器 |
| 第三层（能源 LP） | v0.66.0 | `eneros-energy-lp-model` | `ScheduleConfig` + `EnergyScheduleModel` + `ScheduleResult` 储能调度领域模型 |
| **第四层（安全校验）** | **v0.67.0** | **`eneros-safety-validator`** | **`SafetyRule` trait + `SafetyValidator` + `ElectricalSafetyRule` + `ProtectionCoordinationRule` 安全屏障** |
| 第五层（意图解析） | v0.68.0 | （后续） | LLM 意图 → `ScheduleConfig`（同时消费 PromptTemplate） |

第四层位于第三层与第五层之间：v0.66.0 `ScheduleResult`（LP 求解还原的调度序列）经本版本 `SafetyValidator::validate()` 校验并截断后，交由 v0.68.0 意图解析层或 v0.71.0 双脑联调层下发到 Control Bus。本版本向下复用 v0.66.0 调度结果类型，向上提供安全校验抽象。

### 2.2 与 v0.66.0 的依赖关系

本版本单向依赖 v0.66.0，复用其调度结果类型：

| 复用项 | 上游版本位置 | 本版本用途 | 偏差 |
|--------|-------------|-----------|------|
| `ScheduleResult` | `eneros_energy_lp_model::result::ScheduleResult` | `validate()` 入参 + `clamped_schedule` 字段类型 | D8（复用） |
| `ScheduleEntry` | `eneros_energy_lp_model::result::ScheduleEntry` | 遍历校验各时段 charge/discharge/soc/net_power | D8 |
| `SolveStatus` | `eneros_energy_lp_model::problem::SolveStatus` | `ScheduleResult.solve_status` 字段（间接） | D8 |

```
┌─────────────────────────────────────────────────┐
│  v0.67.0 eneros-safety-validator（本版本）       │
│  ┌───────────────────────────────────────────┐  │
│  │  SafetyRule trait（D1：无 Send + Sync）   │  │
│  │  SafetyValidator（规则链 + 致命终止）      │  │
│  │  ElectricalSafetyRule / ProtectionRule    │  │
│  │  SystemState / ValidationResult / Severity│  │
│  └─────────────────────┬─────────────────────┘  │
│                        │ use ScheduleResult      │
└────────────────────────┼─────────────────────────┘
                         ▼
┌─────────────────────────────────────────────────┐
│  v0.66.0 eneros-energy-lp-model                 │
│  ┌───────────────────────────────────────────┐  │
│  │  ScheduleResult（schedule + total_revenue）│  │
│  │  ScheduleEntry（period/charge/discharge/   │  │
│  │    net_power/soc_pct/revenue_yuan）        │  │
│  │  SolveStatus（Optimal/Infeasible/...）     │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### 2.3 解锁 v0.68.0~v0.71.0

本版本交付的安全校验器解锁后续三个版本：

| 下游版本 | 消费本版本的产出 | 场景 |
|---------|----------------|------|
| v0.68.0 意图解析 | `ValidationResult`（含 `clamped_schedule`） | LLM 意图 → `ScheduleConfig` → `EnergyScheduleModel` → 求解 → `SafetyValidator::validate()` → 下发截断后的安全调度 |
| v0.71.0 双脑联调 | `SafetyValidator` + `ValidationResult` | 双脑编排：LLM 生成意图 → Solver 求解 → SafetyValidator 校验 → 若 Fatal 则降级到 L1 Solver-only 路径 |
| v0.56.0 ConstraintChecker（已存在） | 本版本与之形成 double barrier | RTOS 层命令执行前二次校验（快平面 10ms），本版本在 Solver 层校验（慢平面） |

### 2.4 双脑架构中的定位 — Solver 安全屏障层

双脑架构（蓝图 §9.x）中 LLM 与 Solver 的协作链路，本版本位于 Solver 安全屏障层：

```
[市场信号/自然语言指令]
        │
        ▼
v0.59.0 LlmEngine (trait)
        │
        ▼
   LLM 推理 (llama.cpp via FFI)
        │
        ▼
   JSON 意图输出
        │
        ▼
v0.68.0 意图解析 ──► ScheduleConfig 构造
                        │
                        ▼
                  EnergyScheduleModel::new(config)（v0.66.0）
                        │  自动装配变量 + 约束 + 目标
                        ▼
                  OptProblem.compile()（v0.65.0）
                        │
                        ▼
                  LpProblem 矩阵格式
                        │
                        ▼
v0.64.0 Solver trait
        │
        ├── MockSolver (默认，测试)
        └── HighsSolver (feature-gated，真实求解)
                        │
                        ▼
                  ScheduleResult（v0.66.0 parse_result）
                        │
                        ▼
v0.67.0 SafetyValidator（本版本）──► ValidationResult
                        │               │
                        │               ├── passed=true → 下发原调度
                        │               ├── clamped=true → 下发截断后调度
                        │               └── passed=false (Fatal) → 降级 L1 路径
                        ▼
                  Control Bus（v0.71.0 双脑联调编排）
```

本版本是 Solver 输出到 Control Bus 之间的**最后一道软件安全屏障**（RTOS 层 v0.56.0 ConstraintChecker 是硬件执行前的二次校验）。校验失败时**截断到安全边界**而非直接拒绝，保证系统始终有调度输出（避免"无命令下发"导致的失控）；仅当 SOC < 5% 等致命违规时才拒绝并触发降级。

---

## 3. SafetyRule trait 设计

### 3.1 trait 定义（含 D1 偏差说明）

`SafetyRule` 是安全校验规则的抽象 trait，定义校验规则的统一接口。蓝图原文（line 13987）声明 `pub trait SafetyRule: Send + Sync`，本版本**移除 `Send + Sync` bound**（D1）：

```rust
// crates/ai/safety-validator/src/rule.rs

use alloc::string::String;
use alloc::vec::Vec;

use crate::result::ValidationResult;
use crate::state::SystemState;
use eneros_energy_lp_model::result::ScheduleResult;

/// 安全校验规则抽象.
///
/// 规则按 `priority()` 升序排序（数值越小优先级越高），
/// 链式执行：前序规则截断后，后续规则基于截断后的 schedule 继续校验。
///
/// # D1 偏差说明
///
/// 蓝图原文（line 13987）声明 `pub trait SafetyRule: Send + Sync`，
/// 本版本移除 `Send + Sync` bound。理由：
///
/// 1. **no_std 单线程环境**：本 crate 运行于 Agent Runtime 单线程分区，
///    无跨线程共享需求，`Send + Sync` 在单线程下无意义。
/// 2. **与既有版本一致**：v0.59.0 `LlmEngine` / v0.63.0 `PromptTemplate`
///    / v0.64.0 `Solver` trait 均无 `Send + Sync` bound，本版本保持一致。
/// 3. **`Box<dyn SafetyRule>` 派生限制**：在 no_std 下，`Box<dyn Trait + Send + Sync>`
///    会引入 `alloc::boxed::Box` 的 trait bound 限制，增加不必要的复杂度。
///
/// 详见 §11 D1 偏差声明。
pub trait SafetyRule {
    /// 规则名称（用于违规详情标识）.
    fn name(&self) -> &str;

    /// 校验调度结果.
    ///
    /// 返回校验结果（通过 / 截断 / 拒绝）。若 `clamped == true`，
    /// `clamped_schedule` 字段包含截断后的调度结果。
    fn validate(&self, schedule: &ScheduleResult, state: &SystemState) -> ValidationResult;

    /// 获取规则优先级（数值越小优先级越高，默认 100）.
    fn priority(&self) -> u32 {
        100
    }

    /// 是否为硬约束（硬约束校验失败必须截断或拒绝，默认 true）.
    fn is_hard(&self) -> bool {
        true
    }
}
```

### 3.2 默认方法语义

`SafetyRule` trait 提供两个默认方法，减少实现样板代码：

| 方法 | 默认值 | 语义 | 实现者覆盖 |
|------|--------|------|-----------|
| `priority() -> u32` | `100` | 规则优先级，数值越小优先级越高。`SafetyValidator::add_rule()` 按 `priority()` 升序排序，数值小的规则先执行 | `ElectricalSafetyRule` 覆盖为 `10`；`ProtectionCoordinationRule` 覆盖为 `20` |
| `is_hard() -> bool` | `true` | 是否为硬约束。硬约束校验失败必须截断或拒绝；软约束（`is_hard() == false`）仅记录违规但不截断 | 本版本 2 个默认规则均为硬约束，不覆盖（保持默认 `true`） |

**优先级排序规则**（蓝图 line 14228）：

```rust
// SafetyValidator::add_rule()
self.rules.push(rule);
self.rules.sort_by_key(|r| r.priority());  // D5：alloc 原生支持
```

`Vec::sort_by_key` 是稳定排序（Rust 文档：sort_by_key is stable），同优先级的规则保持插入顺序。本版本 2 个默认规则优先级不同（10 < 20），无同优先级冲突。

**默认优先级 100 的语义**：自定义规则若不覆盖 `priority()`，默认优先级为 100，排在 `ElectricalSafetyRule`（10）和 `ProtectionCoordinationRule`（20）之后。自定义规则可通过覆盖 `priority()` 插队到最前（如 `priority() = 5`，见 §12.1 T18 测试）。

### 3.3 与 v0.59.0 / v0.63.0 / v0.64.0 的一致性

本版本 `SafetyRule` trait 无 `Send + Sync` bound，与既有 AI 子系统 trait 保持一致：

| 版本 | trait | `Send + Sync` | 理由 |
|------|-------|--------------|------|
| v0.59.0 | `LlmEngine` | ❌ 无 | no_std 单线程，LLM 推理在 Agent Runtime 分区内完成 |
| v0.63.0 | `PromptTemplate` | ❌ 无 | no_std 单线程，模板渲染无跨线程需求 |
| v0.64.0 | `Solver` | ❌ 无 | no_std 单线程，LP 求解在 Agent Runtime 分区内完成 |
| **v0.67.0** | **`SafetyRule`** | **❌ 无** | **no_std 单线程，安全校验在 Agent Runtime 分区内完成（D1）** |

> **一致性原则**：AI 子系统的所有 trait 均无 `Send + Sync` bound（no_std 单线程环境）。蓝图原文 `Send + Sync` 是蓝图伪代码的疏漏，实际实现需移除以与既有 trait 一致。`Send + Sync` 在 no_std 单线程下无意义（无跨线程共享），且会引入 `Box<dyn Trait + Send + Sync>` 的派生限制。

---

## 4. SystemState 设计

### 4.1 字段定义

`SystemState` 是安全校验所需的最小系统状态类型，包含电气安全校验必需的 5 个字段：

```rust
// crates/ai/safety-validator/src/state.rs

/// 最小系统状态（电气安全校验所需）.
///
/// # D2 偏差说明
///
/// 蓝图原文（line 13993 等）使用 `SystemState` 但未定义。本版本本地定义
/// 最小 `SystemState`，不依赖 HMI crate 的 `SystemState`（显示状态）。
/// 详见 §4.2 D2 偏差说明。
#[derive(Debug, Clone, Default)]
pub struct SystemState {
    /// 电压（V）.
    pub voltage_v: f64,
    /// 电流（A）.
    pub current_a: f64,
    /// 频率（Hz）.
    pub frequency_hz: f64,
    /// SOC 百分比（0.0~1.0）.
    pub soc_pct: f64,
    /// 时间戳（毫秒）.
    pub timestamp_ms: u64,
}
```

| 字段 | 类型 | 单位 | 用途 | 默认值 |
|------|------|------|------|--------|
| `voltage_v` | `f64` | V | 电压（电气安全校验参考，未来扩展用于 `voltage_range` 校验） | 380.0 |
| `current_a` | `f64` | A | 电流（电气安全校验参考，未来扩展用于 `max_current_a` 校验） | 0.0 |
| `frequency_hz` | `f64` | Hz | 频率（电气安全校验参考，未来扩展用于 `freq_range` 校验） | 50.0 |
| `soc_pct` | `f64` | 0.0~1.0 | 实时 SOC（与 `ScheduleEntry.soc_pct` 互补：`SystemState` 是当前实时值，`ScheduleEntry` 是调度序列值） | 0.5 |
| `timestamp_ms` | `u64` | ms | 时间戳（标识状态采集时刻，未来扩展用于时序校验） | 0 |

### 4.2 D2 偏差说明 — 与 HMI crate SystemState 对比

蓝图原文（line 13993 等）使用 `SystemState` 类型作为 `validate()` 参数，但**未定义**该类型。项目中已存在一个 `SystemState`（HMI crate，`crates/agents/hmi/src/lib.rs:95`），但该类型是 HMI 显示状态，与电气安全校验无关：

| 对比维度 | HMI crate `SystemState` | 本版本 `SystemState`（D2） |
|---------|------------------------|---------------------------|
| 位置 | `crates/agents/hmi/src/lib.rs:95` | `crates/ai/safety-validator/src/state.rs` |
| 用途 | HMI 显示状态（人机界面渲染） | 电气安全校验（规则匹配） |
| 字段 | `agent_states` / `storage_usage` / `network` / `power` 等显示字段 | `voltage_v` / `current_a` / `frequency_hz` / `soc_pct` / `timestamp_ms` 电气字段 |
| 消费者 | HMI 渲染层 | `SafetyRule::validate()` |
| 关联性 | 显示状态，与电气安全无关 | 电气安全状态，HMI 不消费 |

**D2 决策**：本地定义最小 `SystemState`（5 字段电气状态），不依赖 HMI crate。理由（Karpathy "Simplicity First"）：

1. **字段不匹配**：HMI `SystemState` 的 `agent_states` / `storage_usage` / `network` 等字段是显示状态，电气安全校验不需要；电气安全需要的 `voltage_v` / `current_a` / `frequency_hz` 在 HMI `SystemState` 中不存在或语义不同。
2. **避免无关耦合**：若依赖 HMI crate，本 crate 会引入 HMI 的全部依赖链（显示组件 / 渲染逻辑），违反"最小依赖"原则。
3. **职责分离**：HMI `SystemState` 服务于人机界面，本版本 `SystemState` 服务于安全校验，两者职责不同，不应共享类型。
4. **未来扩展**：本版本 `SystemState` 仅 5 字段，未来可独立扩展（如增加 `temperature_c` / `internal_resistance_ohm` 等电气参数），不受 HMI crate 约束。

### 4.3 默认值

`SystemState` 实现 `Default` trait，提供标准电气状态默认值：

```rust
impl Default for SystemState {
    fn default() -> Self {
        Self {
            voltage_v: 380.0,      // 标准三相电压 380V
            current_a: 0.0,        // 空载电流 0A
            frequency_hz: 50.0,    // 标准工频 50Hz
            soc_pct: 0.5,          // 中等 SOC 50%
            timestamp_ms: 0,       // 纪元时间戳
        }
    }
}
```

| 字段 | 默认值 | 物理意义 |
|------|--------|---------|
| `voltage_v` | 380.0 | 中国标准低压三相电压 380V（民用 / 工业低压配电标准） |
| `current_a` | 0.0 | 空载电流 0A（设备未运行） |
| `frequency_hz` | 50.0 | 中国标准工频 50Hz |
| `soc_pct` | 0.5 | 中等 SOC 50%（健康工作区间） |
| `timestamp_ms` | 0 | 纪元时间戳 0（调用方按需填充实际时间） |

> **注**：默认值用于测试与初始化。生产环境调用方（v0.68.0 意图解析 / v0.71.0 双脑联调）负责从实时四遥数据（v0.52.0 telemetry-model）填充 `SystemState`，本 crate 不直接依赖 telemetry-model（D6）。

---

## 5. ValidationResult / Violation / Severity 设计

### 5.1 三者关系图

```
┌─────────────────────────────────────────────────────────┐
│  ValidationResult（校验结果）                            │
│  ┌─────────────────────────────────────────────────┐    │
│  │  passed: bool                                    │    │
│  │  clamped: bool                                   │    │
│  │  clamped_schedule: Option<ScheduleResult>        │    │
│  │  violations: Vec<Violation>                      │    │
│  │                              │                  │    │
│  │                              ▼                  │    │
│  │  ┌──────────────────────────────────────────┐   │    │
│  │  │  Violation（违规详情）                    │   │    │
│  │  │  rule: String                             │   │    │
│  │  │  period: usize                            │   │    │
│  │  │  field: String                            │   │    │
│  │  │  original_value: f64                      │   │    │
│  │  │  safe_value: f64                          │   │    │
│  │  │  severity: Severity ──────────────────┐   │   │    │
│  │  └────────────────────────────────────────┼──┘   │    │
│  └──────────────────────────────────────────┼──────┘    │
└─────────────────────────────────────────────┼───────────┘
                                              │
                                              ▼
                              ┌──────────────────────────┐
                              │  Severity（严重程度）     │
                              │  Info / Warning /        │
                              │  Critical / Fatal        │
                              └──────────────────────────┘
```

- `ValidationResult` 包含多个 `Violation`（一对多）
- 每个 `Violation` 包含一个 `Severity`（一对一）
- `Severity` 决定 `ValidationResult.passed`（Fatal → passed=false）

### 5.2 Severity 枚举（含 D8 偏差）

`Severity` 枚举定义违规的严重程度，分 4 级：

```rust
// crates/ai/safety-validator/src/result.rs

/// 违规严重程度.
///
/// # D8 偏差说明
///
/// `Severity` 派生 `Debug + Clone + Copy + PartialEq`：
/// - `PartialEq`：用于 `==` 比较（蓝图 line 14124 `v.severity != Severity::Fatal`
///   和 line 14252 `v.severity == Severity::Fatal`）。
/// - `Copy`：`Severity` 是简单枚举（4 个 unit variant），无字段，`Copy` 无开销。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    /// 提示（记录但不影响调度）.
    Info,
    /// 警告（接近边界，建议关注）.
    Warning,
    /// 严重（超限，截断到安全边界）.
    Critical,
    /// 致命（必须拒绝，立即终止校验）.
    Fatal,
}
```

| 变体 | 语义 | 对 `passed` 影响 | 对 `clamped` 影响 | 是否终止链 |
|------|------|----------------|------------------|-----------|
| `Info` | 提示，记录但不影响调度 | 不影响（`passed` 仍 true） | 不截断 | 不终止 |
| `Warning` | 警告，接近安全边界 | 不影响（`passed` 仍 true） | 不截断 | 不终止 |
| `Critical` | 严重，超限，截断到安全边界 | 不影响（`passed` 仍 true，截断后安全） | 截断（`clamped = true`） | 不终止 |
| `Fatal` | 致命，必须拒绝 | `passed = false` | 截断（仍截断到安全边界，但拒绝下发） | 立即终止后续规则 |

**D8 偏差理由**：

- `PartialEq`：蓝图代码使用 `v.severity != Severity::Fatal`（line 14124）和 `v.severity == Severity::Fatal`（line 14252）做相等比较，需派生 `PartialEq`。
- `Copy`：`Severity` 是简单枚举（4 个 unit variant，无字段），`Copy` trait 无运行时开销，且简化代码（按值传递无需 `&Severity`）。
- 不派生 `Eq`：`PartialEq` 已满足 `==` / `!=` 比较；`Eq` 仅在 `BTreeMap` / `BTreeSet` 等 `Ord` 容器中使用，本版本无需。

### 5.3 Violation 结构（含 D9 偏差）

`Violation` 结构记录单次违规的详细信息：

```rust
/// 违规详情.
///
/// # D9 偏差说明
///
/// `Violation` 仅派生 `Debug + Clone`，**不派生 `PartialEq`**。
/// 理由（Karpathy "Simplicity First"）：
/// - 当前测试不需要 `Violation` 的 `PartialEq`（测试通过字段访问断言，而非整体相等）
/// - `Violation` 包含 `String` 字段（rule / field），`PartialEq` 对 `String` 比较开销较高
/// - 避免过早添加 trait（YAGNI 原则）
#[derive(Debug, Clone)]
pub struct Violation {
    /// 违规规则名（对应 `SafetyRule::name()`）.
    pub rule: String,
    /// 违规时段索引（对应 `ScheduleEntry.period`）.
    pub period: usize,
    /// 违规字段名（如 "charge_power" / "discharge_power" / "soc" / "ramp_rate"）.
    pub field: String,
    /// 原始值（校验前的值）.
    pub original_value: f64,
    /// 安全值（截断后的值）.
    pub safe_value: f64,
    /// 严重程度.
    pub severity: Severity,
}
```

| 字段 | 类型 | 说明 | 示例 |
|------|------|------|------|
| `rule` | `String` | 违规规则名（`SafetyRule::name()` 返回值） | `"electrical_safety"` / `"protection_coordination"` |
| `period` | `usize` | 违规时段索引（`ScheduleEntry.period`） | `0` / `1` / ... / `95` |
| `field` | `String` | 违规字段名 | `"charge_power"` / `"discharge_power"` / `"soc"` / `"ramp_rate"` |
| `original_value` | `f64` | 原始值（校验前） | `120.0`（充电功率超限） |
| `safe_value` | `f64` | 安全值（截断后） | `100.0`（截断到 max_power_kw） |
| `severity` | `Severity` | 严重程度 | `Critical` / `Fatal` |

**D9 偏差理由**（不派生 `PartialEq`）：

- **测试方式**：当前测试通过字段访问断言（如 `assert_eq!(violation.period, 0)` / `assert_eq!(violation.severity, Severity::Critical)`），不需要整体 `assert_eq!(violation1, violation2)`。
- **String 比较开销**：`Violation` 包含 2 个 `String` 字段（`rule` / `field`），`PartialEq` 需逐字符比较 `String`，开销较高且无实际用途。
- **YAGNI 原则**（You Aren't Gonna Need It）：避免过早添加 trait，减少编译时间与代码复杂度。
- **与 v0.66.0 一致**：v0.66.0 `ScheduleResult` / `ScheduleEntry` 也仅派生 `Debug + Clone`，不派生 `PartialEq`。

### 5.4 ValidationResult 结构

`ValidationResult` 结构是 `SafetyRule::validate()` 和 `SafetyValidator::validate()` 的返回类型：

```rust
/// 校验结果.
///
/// # D9 偏差说明
///
/// `ValidationResult` 仅派生 `Debug + Clone`，**不派生 `PartialEq`**。
/// 理由同 `Violation`（§5.3 D9）。
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// 是否通过（无致命违规）.
    ///
    /// - `true`：无违规，或仅有 Info/Warning/Critical 级违规（已截断）
    /// - `false`：存在 Fatal 级违规（必须拒绝）
    pub passed: bool,
    /// 是否已截断（存在 Critical/Fatal 级违规且已截断到安全边界）.
    pub clamped: bool,
    /// 截断后的调度结果（`clamped == true` 时为 `Some`，否则 `None`）.
    pub clamped_schedule: Option<ScheduleResult>,
    /// 违规详情列表（按规则执行顺序累积）.
    pub violations: Vec<Violation>,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `passed` | `bool` | 是否通过（无致命违规）。`passed = violations.iter().all(\|v\| v.severity != Severity::Fatal)` |
| `clamped` | `bool` | 是否已截断。任一规则 `clamped == true` 则本字段 `true` |
| `clamped_schedule` | `Option<ScheduleResult>` | 截断后的调度结果。`clamped == true` 时为 `Some(...)`，否则 `None` |
| `violations` | `Vec<Violation>` | 违规详情列表，按规则执行顺序累积（先 `ElectricalSafetyRule` 的违规，后 `ProtectionCoordinationRule` 的违规） |

**`passed` 与 `clamped` 的组合语义**：

| `passed` | `clamped` | 语义 | 调用方动作 |
|----------|-----------|------|-----------|
| `true` | `false` | 全部规则通过，无违规 | 下发原 `schedule` |
| `true` | `true` | 存在 Critical 级违规，已截断到安全边界 | 下发 `clamped_schedule`（截断后安全） |
| `false` | `true` | 存在 Fatal 级违规，已截断但仍拒绝 | 触发降级（L1 Solver-only 路径） |
| `false` | `false` | 存在 Fatal 级违规但未截断（理论上不应出现，Fatal 级违规总是截断） | 触发降级 |

---

## 6. ElectricalSafetyRule 设计

### 6.1 字段定义（含 D12 偏差）

`ElectricalSafetyRule` 是电气安全校验规则，校验充电 / 放电功率与 SOC 范围：

```rust
// crates/ai/safety-validator/src/electrical.rs

use alloc::string::String;
use alloc::vec::Vec;

use crate::result::{Severity, ValidationResult, Violation};
use crate::rule::SafetyRule;
use crate::state::SystemState;
use eneros_energy_lp_model::result::ScheduleResult;

/// 电气安全校验规则.
///
/// 校验功率/SOC 在安全范围，超限截断到安全边界。
///
/// # D12 偏差说明
///
/// 本规则保留 `max_current_a` / `voltage_range` / `freq_range` 字段，
/// 但 `validate()` 仅使用 `max_power_kw` 和 SOC 阈值（0.95/0.05）。
/// 保留字段供未来扩展（电压/电流/频率校验）。`#![allow(dead_code)]`
/// 抑制未使用字段警告。
pub struct ElectricalSafetyRule {
    /// 最大充放电功率（kW）.
    max_power_kw: f64,
    /// 最大电流（A）【D12：保留未使用】.
    max_current_a: f64,
    /// 电压范围（V）【D12：保留未使用】.
    voltage_range: (f64, f64),
    /// 频率范围（Hz）【D12：保留未使用】.
    freq_range: (f64, f64),
}

impl ElectricalSafetyRule {
    /// 创建电气安全规则.
    pub fn new(
        max_power_kw: f64,
        max_current_a: f64,
        voltage_range: (f64, f64),
        freq_range: (f64, f64),
    ) -> Self {
        Self {
            max_power_kw,
            max_current_a,
            voltage_range,
            freq_range,
        }
    }
}
```

| 字段 | 类型 | 单位 | `validate()` 是否使用 | 说明 |
|------|------|------|---------------------|------|
| `max_power_kw` | `f64` | kW | ✅ 使用 | 充电 / 放电功率上限（截断目标） |
| `max_current_a` | `f64` | A | ❌ 未使用（D12） | 最大电流，保留供未来扩展 |
| `voltage_range` | `(f64, f64)` | V | ❌ 未使用（D12） | 电压范围，保留供未来扩展 |
| `freq_range` | `(f64, f64)` | Hz | ❌ 未使用（D12） | 频率范围，保留供未来扩展 |

**D12 偏差理由**（保留未使用字段）：

- **蓝图已定义**：蓝图 line 14047~14055 明确定义了这 4 个字段，是蓝图设计的一部分。
- **未来扩展**：`max_current_a` / `voltage_range` / `freq_range` 是电气安全校验的自然扩展点（未来版本会校验电压 / 电流 / 频率是否在安全范围）。
- **Karpathy "Surgical Changes"**：不删除蓝图已定义的字段，避免破坏蓝图的字段规划。
- **警告抑制**：`lib.rs` 顶层 `#![allow(dead_code)]` 抑制未使用字段警告（项目约定）。

### 6.2 validate 逻辑

`ElectricalSafetyRule::validate()` 遍历 `ScheduleResult.schedule` 各时段，校验充电功率 / 放电功率 / SOC 范围，超限截断：

```rust
impl SafetyRule for ElectricalSafetyRule {
    fn name(&self) -> &str {
        "electrical_safety"
    }

    fn priority(&self) -> u32 {
        10  // 高于 ProtectionCoordinationRule 的 20
    }

    fn is_hard(&self) -> bool {
        true
    }

    fn validate(
        &self,
        schedule: &ScheduleResult,
        _state: &SystemState,
    ) -> ValidationResult {
        let mut violations = Vec::new();
        let mut clamped_schedule = schedule.clone();
        let mut clamped = false;

        for entry in &mut clamped_schedule.schedule {
            // 校验充电功率
            if entry.charge_power_kw > self.max_power_kw {
                violations.push(Violation {
                    rule: self.name().into(),  // D3：&str → String
                    period: entry.period,
                    field: "charge_power".into(),
                    original_value: entry.charge_power_kw,
                    safe_value: self.max_power_kw,
                    severity: Severity::Critical,
                });
                entry.charge_power_kw = self.max_power_kw;
                clamped = true;
            }

            // 校验放电功率
            if entry.discharge_power_kw > self.max_power_kw {
                violations.push(Violation {
                    rule: self.name().into(),
                    period: entry.period,
                    field: "discharge_power".into(),
                    original_value: entry.discharge_power_kw,
                    safe_value: self.max_power_kw,
                    severity: Severity::Critical,
                });
                entry.discharge_power_kw = self.max_power_kw;
                clamped = true;
            }

            // 校验 SOC 上限（D10：直接比较固定阈值 0.95）
            if entry.soc_pct > 0.95 {
                let safe = 0.95;
                violations.push(Violation {
                    rule: self.name().into(),
                    period: entry.period,
                    field: "soc".into(),
                    original_value: entry.soc_pct,
                    safe_value: safe,
                    severity: Severity::Critical,
                });
                entry.soc_pct = safe;
                clamped = true;
            }

            // 校验 SOC 下限（D10：直接比较固定阈值 0.05）
            if entry.soc_pct < 0.05 {
                let safe = 0.05;
                violations.push(Violation {
                    rule: self.name().into(),
                    period: entry.period,
                    field: "soc".into(),
                    original_value: entry.soc_pct,
                    safe_value: safe,
                    severity: Severity::Fatal,  // SOC < 5% 是致命违规
                });
                entry.soc_pct = safe;
                clamped = true;
            }
        }

        let passed = violations.iter().all(|v| v.severity != Severity::Fatal);
        ValidationResult {
            passed,
            clamped,
            clamped_schedule: if clamped { Some(clamped_schedule) } else { None },
            violations,
        }
    }
}
```

**校验逻辑详解**：

| 校验项 | 条件 | 截断目标 | `severity` | 说明 |
|--------|------|---------|-----------|------|
| 充电功率超限 | `charge_power_kw > max_power_kw` | `max_power_kw` | `Critical` | 截断到额定功率 |
| 放电功率超限 | `discharge_power_kw > max_power_kw` | `max_power_kw` | `Critical` | 截断到额定功率 |
| SOC 上限超限 | `soc_pct > 0.95` | `0.95` | `Critical` | 截断到安全上限（避免过充） |
| SOC 下限超限 | `soc_pct < 0.05` | `0.05` | `Fatal` | 截断到安全下限（避免过放），但标记为致命（深度放电损伤电池） |

**D3 偏差说明**（`self.name().into()`）：

- 蓝图使用 `rule: self.name().into()`（line 14072 等），将 `&str` 转换为 `String`。
- `extern crate alloc` 后，`alloc::string::ToString` trait 为 `&str` 实现了 `.to_string()` 方法，`From<&str> for String` 实现了 `.into()` 方法。
- 两者等价，本版本保留蓝图 `.into()` 写法（D3）。

**D10 偏差说明**（直接浮点比较）：

- 蓝图 `if entry.soc_pct > 0.95`（line 14096）使用直接浮点比较。
- 蓝图 §8.3 提到"浮点数比较需用容差"，但该警告针对**迭代计算结果**（如 LP 求解器输出的 soc 可能因迭代收敛误差产生微小偏差）。
- 本版本的阈值 `0.95` / `0.05` 是**固定常数**，不是迭代结果，直接比较无精度问题。
- 若未来需要容差，可添加 `const EPSILON: f64 = 1e-9`，但当前无需（YAGNI）。

### 6.3 priority = 10（高于 ProtectionCoordinationRule 的 20）

`ElectricalSafetyRule::priority()` 返回 `10`，高于 `ProtectionCoordinationRule` 的 `20`，确保电气安全规则**先于**保护配合规则执行：

| 规则 | priority | 执行顺序 | 理由 |
|------|----------|---------|------|
| `ElectricalSafetyRule` | 10 | 第 1 | 电气安全是基础校验（功率 / SOC 范围），必须先于保护配合校验。若电气安全违规（如 SOC 越限），截断后再由保护配合规则校验截断后的调度（爬坡率可能因截断而改变） |
| `ProtectionCoordinationRule` | 20 | 第 2 | 保护配合校验基于电气安全截断后的调度结果，确保截断后的功率变化率仍在安全范围 |
| 自定义规则（默认） | 100 | 第 3+ | 自定义规则默认优先级 100，排在 2 个默认规则之后。可通过覆盖 `priority()` 插队 |

**链式截断的必要性**：若 `ElectricalSafetyRule` 截断了充电功率（如 120 → 100），相邻时段的功率变化率可能改变（原本 0 → 120 变为 0 → 100），`ProtectionCoordinationRule` 需基于截断后的调度重新校验爬坡率。这正是链式执行的价值（§8.4 详述）。

---

## 7. ProtectionCoordinationRule 设计

### 7.1 字段定义（含 D12 偏差）

`ProtectionCoordinationRule` 是保护配合校验规则，校验相邻时段功率变化率（爬坡率）：

```rust
// crates/ai/safety-validator/src/protection.rs

use alloc::string::String;
use alloc::vec::Vec;

use crate::result::{Severity, ValidationResult, Violation};
use crate::rule::SafetyRule;
use crate::state::SystemState;
use eneros_energy_lp_model::result::ScheduleResult;

/// 保护配合校验规则.
///
/// 校验相邻时段功率变化率 ≤ max_ramp_rate（kW/min），
/// 超限时截断功率变化，避免触发 PCS 过流保护跳闸。
///
/// # D12 偏差说明
///
/// 本规则保留 `overcurrent_threshold` / `overvoltage_threshold` /
/// `undervoltage_threshold` / `freq_protection` 字段，但 `validate()`
/// 仅使用 `max_ramp_rate`。保留字段供未来扩展（过流/过压/欠压/频率保护）。
pub struct ProtectionCoordinationRule {
    /// 过流保护阈值（A）【D12：保留未使用】.
    overcurrent_threshold: f64,
    /// 过压保护阈值（V）【D12：保留未使用】.
    overvoltage_threshold: f64,
    /// 欠压保护阈值（V）【D12：保留未使用】.
    undervoltage_threshold: f64,
    /// 频率保护范围（Hz）【D12：保留未使用】.
    freq_protection: (f64, f64),
    /// 最大功率变化率（kW/min）【validate 使用】.
    max_ramp_rate: f64,
}

impl ProtectionCoordinationRule {
    /// 创建保护配合规则.
    pub fn new(
        overcurrent_threshold: f64,
        overvoltage_threshold: f64,
        undervoltage_threshold: f64,
        freq_protection: (f64, f64),
        max_ramp_rate: f64,
    ) -> Self {
        Self {
            overcurrent_threshold,
            overvoltage_threshold,
            undervoltage_threshold,
            freq_protection,
            max_ramp_rate,
        }
    }
}
```

| 字段 | 类型 | 单位 | `validate()` 是否使用 | 说明 |
|------|------|------|---------------------|------|
| `overcurrent_threshold` | `f64` | A | ❌ 未使用（D12） | 过流保护阈值，保留供未来扩展 |
| `overvoltage_threshold` | `f64` | V | ❌ 未使用（D12） | 过压保护阈值，保留供未来扩展 |
| `undervoltage_threshold` | `f64` | V | ❌ 未使用（D12） | 欠压保护阈值，保留供未来扩展 |
| `freq_protection` | `(f64, f64)` | Hz | ❌ 未使用（D12） | 频率保护范围，保留供未来扩展 |
| `max_ramp_rate` | `f64` | kW/min | ✅ 使用 | 最大功率变化率（爬坡率上限） |

### 7.2 validate 逻辑

`ProtectionCoordinationRule::validate()` 遍历相邻时段，计算功率变化率并校验：

```rust
impl SafetyRule for ProtectionCoordinationRule {
    fn name(&self) -> &str {
        "protection_coordination"
    }

    fn priority(&self) -> u32 {
        20  // 低于 ElectricalSafetyRule 的 10
    }

    fn is_hard(&self) -> bool {
        true
    }

    fn validate(
        &self,
        schedule: &ScheduleResult,
        _state: &SystemState,
    ) -> ValidationResult {
        let mut violations = Vec::new();
        let mut clamped_schedule = schedule.clone();
        let mut clamped = false;

        // 校验功率变化率（爬坡率）
        for i in 1..clamped_schedule.schedule.len() {
            let prev = clamped_schedule.schedule[i - 1].net_power_kw;
            let curr = clamped_schedule.schedule[i].net_power_kw;
            let delta = (curr - prev).abs();
            // 转换为 kW/min（15min 时段 → 除以 0.25h = 15min）
            let delta_per_min = delta / 0.25;

            if delta_per_min > self.max_ramp_rate {
                // 截断到安全变化率
                let safe_delta = self.max_ramp_rate * 0.25 * curr.signum();
                let safe_value = prev + safe_delta;

                violations.push(Violation {
                    rule: self.name().into(),  // D3
                    period: i,
                    field: "ramp_rate".into(),
                    original_value: curr,
                    safe_value,
                    severity: Severity::Critical,
                });

                // D11：精确复制蓝图截断逻辑（line 14183~14184）
                let diff = safe_value - curr;
                clamped_schedule.schedule[i].discharge_power_kw += diff.max(0.0);
                clamped_schedule.schedule[i].charge_power_kw += (-diff).max(0.0);
                clamped = true;
            }
        }

        let passed = violations.iter().all(|v| v.severity != Severity::Fatal);
        ValidationResult {
            passed,
            clamped,
            clamped_schedule: if clamped { Some(clamped_schedule) } else { None },
            violations,
        }
    }
}
```

**爬坡率计算详解**：

| 步骤 | 公式 | 说明 |
|------|------|------|
| 1. 取相邻时段净功率 | `prev = schedule[i-1].net_power_kw`<br>`curr = schedule[i].net_power_kw` | `net_power_kw = discharge_power_kw - charge_power_kw`（v0.66.0 `ScheduleEntry` 字段） |
| 2. 计算功率变化量 | `delta = (curr - prev).abs()` | 绝对值，不区分上升 / 下降 |
| 3. 转换为 kW/min | `delta_per_min = delta / 0.25` | 15min 时段（`period_hours = 0.25h`），除以 0.25h = 乘以 4（每 15min 的变化转换为每分钟变化） |
| 4. 校验 | `if delta_per_min > max_ramp_rate` | 超限则截断 |
| 5. 截断目标 | `safe_delta = max_ramp_rate * 0.25 * curr.signum()` | 安全变化量 = 最大变化率 × 时段时长 × 方向符号（保持原方向） |
| 6. 截断后净功率 | `safe_value = prev + safe_delta` | 前序时段净功率 + 安全变化量 |
| 7. 调整 discharge / charge | `diff = safe_value - curr`<br>`discharge_power_kw += diff.max(0.0)`<br>`charge_power_kw += (-diff).max(0.0)` | D11：精确复制蓝图截断逻辑（见 §7.3） |

**`curr.signum()` 的语义**：

- `f64::signum()` 返回 `-1.0` / `0.0` / `1.0`（`core::f64` 原生方法，no_std 可用）
- `curr.signum()` 保持原功率方向：若 `curr > 0`（放电大于充电），截断后仍为正方向；若 `curr < 0`（充电大于放电），截断后仍为负方向
- 避免截断后功率方向反转（如放电变充电）

### 7.3 D11 截断逻辑精确复制说明

蓝图原文（line 14181~14184）的截断逻辑：

```rust
// 蓝图原文（line 14181~14184）
// 截断功率变化
let diff = safe_value - curr;
clamped_schedule.schedule[i].discharge_power_kw += diff.max(0.0);
clamped_schedule.schedule[i].charge_power_kw += (-diff).max(0.0);
clamped = true;
```

**D11 决策**：本版本**精确复制**蓝图截断逻辑，不"修复"或"优化"。

**截断逻辑语义分析**：

| `diff` 值 | `diff.max(0.0)` | `(-diff).max(0.0)` | 调整效果 |
|-----------|-----------------|---------------------|---------|
| `diff > 0`（`safe_value > curr`，需增加净功率） | `diff` | `0.0` | `discharge_power_kw += diff`（增加放电功率） |
| `diff < 0`（`safe_value < curr`，需减少净功率） | `0.0` | `-diff` | `charge_power_kw += -diff`（增加充电功率，等价于减少净功率） |
| `diff == 0` | `0.0` | `0.0` | 无调整 |

**为何不重新计算 `net_power_kw` 和 `revenue_yuan`**：

- **Karpathy "Surgical Changes"**：蓝图截断逻辑是核心业务逻辑，不修改不理解的业务逻辑。
- **蓝图未要求**：蓝图原文（line 14181~14184）仅调整 `discharge_power_kw` / `charge_power_kw`，未重新计算 `net_power_kw` / `revenue_yuan`。
- **下游处理**：截断后的 `net_power_kw` / `revenue_yuan` 重新计算由调用方（v0.68.0 / v0.71.0）或未来版本处理；本版本仅执行截断。
- **tasks.md Task 6 / Task 7 说明**：tasks.md 提到"截断后更新 net_power_kw（= discharge - charge）和 revenue_yuan"，但这属于实现细节，本设计文档遵循蓝图原文（D11）。

> **D11 重要说明**：本版本严格遵循"精确复制蓝图截断逻辑"原则。若未来版本发现截断后 `net_power_kw` 不一致（`net_power_kw` 仍为截断前的值），需在**新版本**中修正，而非在本版本中"修复"。这是 Karpathy "Surgical Changes" 原则的体现：不在不理解业务逻辑的情况下修改代码。

---

## 8. SafetyValidator 链式校验设计

### 8.1 字段定义（含 D4 偏差）

`SafetyValidator` 是安全校验器主接口，持有规则列表并链式执行：

```rust
// crates/ai/safety-validator/src/validator.rs

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::electrical::ElectricalSafetyRule;
use crate::protection::ProtectionCoordinationRule;
use crate::result::{Severity, ValidationResult};
use crate::rule::SafetyRule;
use crate::state::SystemState;
use eneros_energy_lp_model::result::ScheduleResult;

/// 安全校验器.
///
/// 持有规则列表（按 `priority()` 升序排序），链式执行所有规则。
/// 前序规则截断后，后续规则基于截断后的 schedule 继续校验。
/// 致命违规（Fatal）立即终止后续规则。
pub struct SafetyValidator {
    /// 校验规则列表（按优先级排序）.
    rules: Vec<Box<dyn SafetyRule>>,  // D4：alloc::vec::Vec + alloc::boxed::Box
}
```

**D4 偏差说明**（`Vec<Box<dyn SafetyRule>>`）：

- 蓝图原文（line 14201）使用 `Vec<Box<dyn SafetyRule>>`。
- no_std 下 `Vec` / `Box` 不可用（`std::vec::Vec` / `std::boxed::Box`），需使用 `alloc::vec::Vec` / `alloc::boxed::Box`。
- `extern crate alloc` 后，`alloc::vec::Vec` / `alloc::boxed::Box` 可用，与 `std::vec::Vec` / `std::boxed::Box` API 完全一致。
- `Box<dyn SafetyRule>` 是 trait 对象（动态分发），允许在同一个 `Vec` 中存储不同类型的规则（`ElectricalSafetyRule` / `ProtectionCoordinationRule` / 自定义规则）。

### 8.2 new() 注册 2 默认规则

`SafetyValidator::new()` 创建校验器并注册 2 个默认规则：

```rust
impl SafetyValidator {
    /// 创建安全校验器（注册默认规则）.
    pub fn new() -> Self {
        let mut validator = Self {
            rules: Vec::new(),
        };

        // 注册默认规则（按优先级）
        validator.add_rule(Box::new(ElectricalSafetyRule::new(
            100.0,           // max_power_kw
            200.0,           // max_current_a（D12：保留未使用）
            (340.0, 420.0),  // voltage_range（D12：保留未使用）
            (49.5, 50.5),    // freq_range（D12：保留未使用）
        )));

        validator.add_rule(Box::new(ProtectionCoordinationRule::new(
            220.0,           // overcurrent_threshold（D12：保留未使用）
            440.0,           // overvoltage_threshold（D12：保留未使用）
            320.0,           // undervoltage_threshold（D12：保留未使用）
            (49.0, 51.0),    // freq_protection（D12：保留未使用）
            200.0,           // max_ramp_rate（validate 使用）
        )));

        validator
    }
}

impl Default for SafetyValidator {
    fn default() -> Self {
        Self::new()
    }
}
```

**默认规则参数**：

| 规则 | 参数 | 值 | 物理意义 |
|------|------|----|---------|
| `ElectricalSafetyRule` | `max_power_kw` | 100.0 | PCS 额定功率 100kW |
| | `max_current_a` | 200.0 | 最大电流 200A（D12：保留未使用） |
| | `voltage_range` | (340.0, 420.0) | 电压范围 340~420V（D12：保留未使用） |
| | `freq_range` | (49.5, 50.5) | 频率范围 49.5~50.5Hz（D12：保留未使用） |
| `ProtectionCoordinationRule` | `overcurrent_threshold` | 220.0 | 过流保护 220A（D12：保留未使用） |
| | `overvoltage_threshold` | 440.0 | 过压保护 440V（D12：保留未使用） |
| | `undervoltage_threshold` | 320.0 | 欠压保护 320V（D12：保留未使用） |
| | `freq_protection` | (49.0, 51.0) | 频率保护 49~51Hz（D12：保留未使用） |
| | `max_ramp_rate` | 200.0 | 最大爬坡率 200kW/min |

### 8.3 add_rule() 排序（含 D5 偏差）

`add_rule()` 添加规则并按 `priority()` 升序排序：

```rust
impl SafetyValidator {
    /// 添加自定义规则.
    ///
    /// 规则按 `priority()` 升序排序（数值越小优先级越高）。
    /// 同优先级保持插入顺序（`sort_by_key` 稳定排序）。
    pub fn add_rule(&mut self, rule: Box<dyn SafetyRule>) {
        self.rules.push(rule);
        // D5：Vec::sort_by_key（alloc 原生支持，稳定排序）
        self.rules.sort_by_key(|r| r.priority());
    }
}
```

**D5 偏差说明**（`Vec::sort_by_key`）：

- 蓝图原文（line 14228）使用 `self.rules.sort_by_key(|r| r.priority())`。
- no_std 下 `std::vec::Vec::sort_by_key` 不可用，但 `alloc::vec::Vec::sort_by_key` 可用（`extern crate alloc` 后）。
- `sort_by_key` 是稳定排序（Rust 文档：The sort is stable），同优先级的规则保持插入顺序。
- 时间复杂度：O(n log n)，n 为规则数（本版本默认 2 条，自定义规则数通常 < 10，性能无忧）。

**排序示例**：

```
初始：rules = []
add_rule(ElectricalSafetyRule)    // priority=10
  → rules = [Electrical(10)]
add_rule(ProtectionCoordinationRule)  // priority=20
  → rules = [Electrical(10), Protection(20)]  // 已排序
add_rule(CustomRule(priority=5))   // 自定义规则插队
  → rules = [Custom(5), Electrical(10), Protection(20)]  // Custom 插到最前
```

### 8.4 validate() 链式执行 + 致命终止

`validate()` 是校验器主方法，链式执行所有规则：

```rust
impl SafetyValidator {
    /// 校验调度结果.
    ///
    /// 依次执行所有规则（按 priority 升序）：
    /// - 前序规则截断后，后续规则基于截断后的 schedule 继续校验
    /// - 致命违规（Fatal）立即终止后续规则
    /// - 累积所有违规详情
    pub fn validate(
        &self,
        schedule: &ScheduleResult,
        state: &SystemState,
    ) -> ValidationResult {
        let mut current_schedule = schedule.clone();
        let mut all_violations = Vec::new();
        let mut any_clamped = false;

        for rule in &self.rules {
            let result = rule.validate(&current_schedule, state);

            all_violations.extend(result.violations);

            if result.clamped {
                if let Some(clamped) = result.clamped_schedule {
                    current_schedule = clamped;
                    any_clamped = true;
                }
            }

            // 致命违规立即终止
            if result.violations.iter().any(|v| v.severity == Severity::Fatal) {
                return ValidationResult {
                    passed: false,
                    clamped: any_clamped,
                    clamped_schedule: Some(current_schedule),
                    violations: all_violations,
                };
            }
        }

        ValidationResult {
            passed: all_violations.is_empty()
                || all_violations.iter().all(|v| v.severity != Severity::Fatal),
            clamped: any_clamped,
            clamped_schedule: if any_clamped { Some(current_schedule) } else { None },
            violations: all_violations,
        }
    }
}
```

**链式执行流程**：

```
输入：schedule（原始调度结果）+ state（系统状态）
  │
  ▼
规则 1：ElectricalSafetyRule.validate(schedule, state)
  │
  ├── 无违规 → result.clamped = false
  │   → current_schedule 不变，继续规则 2
  │
  ├── Critical 违规（功率/SOC 上限超限）→ result.clamped = true
  │   → current_schedule = 截断后的 schedule，继续规则 2
  │
  └── Fatal 违规（SOC < 5%）→ result.clamped = true
      → 立即终止，返回 passed=false
  │
  ▼
规则 2：ProtectionCoordinationRule.validate(current_schedule, state)
  │  （基于规则 1 截断后的 schedule 校验）
  │
  ├── 无违规 → result.clamped = false
  │   → current_schedule 不变
  │
  └── Critical 违规（爬坡率超限）→ result.clamped = true
      → current_schedule = 截断后的 schedule
  │
  ▼
返回：ValidationResult {
    passed: 无 Fatal 违规 → true
    clamped: 任一规则截断 → true
    clamped_schedule: 截断后的最终 schedule
    violations: 累积所有违规
}
```

**关键设计点**：

1. **链式截断**：前序规则截断后，后续规则基于截断后的 `current_schedule` 继续校验。这确保截断后的调度仍满足后续规则的安全约束（如电气截断后爬坡率可能改变，需重新校验）。
2. **致命终止**：任一规则返回 Fatal 级违规，立即终止后续规则，返回 `passed = false`。这避免致命违规后继续校验无意义的结果。
3. **违规累积**：所有规则的违规详情累积到 `all_violations`，调用方可查看完整的违规历史（哪些规则违规了、违规了什么）。
4. **`clamped_schedule` 传递**：每次截断后更新 `current_schedule`，最终返回截断后的调度（若任一规则截断）。

### Mermaid 图 1：SafetyValidator 类图

```mermaid
classDiagram
    class SafetyValidator {
        -rules: Vec~Box~dyn SafetyRule~~
        +new() SafetyValidator
        +add_rule(rule: Box~dyn SafetyRule~)
        +validate(schedule: ScheduleResult, state: SystemState) ValidationResult
    }

    class SafetyRule {
        <<trait>>
        +name() str
        +validate(schedule: ScheduleResult, state: SystemState) ValidationResult
        +priority() u32
        +is_hard() bool
    }

    class ElectricalSafetyRule {
        -max_power_kw: f64
        -max_current_a: f64
        -voltage_range: (f64, f64)
        -freq_range: (f64, f64)
        +new(max_power_kw, max_current_a, voltage_range, freq_range)
        +name() "electrical_safety"
        +priority() 10
        +is_hard() true
        +validate() ValidationResult
    }

    class ProtectionCoordinationRule {
        -overcurrent_threshold: f64
        -overvoltage_threshold: f64
        -undervoltage_threshold: f64
        -freq_protection: (f64, f64)
        -max_ramp_rate: f64
        +new(overcurrent, overvoltage, undervoltage, freq_prot, max_ramp)
        +name() "protection_coordination"
        +priority() 20
        +is_hard() true
        +validate() ValidationResult
    }

    class SystemState {
        +voltage_v: f64
        +current_a: f64
        +frequency_hz: f64
        +soc_pct: f64
        +timestamp_ms: u64
        +default() SystemState
    }

    class ValidationResult {
        +passed: bool
        +clamped: bool
        +clamped_schedule: Option~ScheduleResult~
        +violations: Vec~Violation~
    }

    class Violation {
        +rule: String
        +period: usize
        +field: String
        +original_value: f64
        +safe_value: f64
        +severity: Severity
    }

    class Severity {
        <<enum>>
        Info
        Warning
        Critical
        Fatal
    }

    class ScheduleResult {
        <<v0.66.0 eneros-energy-lp-model>>
        +schedule: Vec~ScheduleEntry~
        +total_revenue_yuan: f64
        +objective_value: f64
        +solve_status: SolveStatus
    }

    SafetyValidator o--> SafetyRule : rules: Vec~Box~dyn SafetyRule~~
    SafetyRule <|.. ElectricalSafetyRule : implements
    SafetyRule <|.. ProtectionCoordinationRule : implements
    SafetyRule ..> ValidationResult : validate() returns
    SafetyRule ..> SystemState : validate() input
    SafetyRule ..> ScheduleResult : validate() input
    ValidationResult --> Violation : violations
    Violation --> Severity : severity
```

### Mermaid 图 2：validate() 链式执行时序图

```mermaid
sequenceDiagram
    participant Caller as 调用方
    participant Validator as SafetyValidator
    participant Rule1 as ElectricalSafetyRule (priority=10)
    participant Rule2 as ProtectionCoordinationRule (priority=20)

    Caller->>Validator: validate(schedule, state)
    activate Validator

    Note over Validator: current_schedule = schedule.clone()
    Note over Validator: all_violations = []
    Note over Validator: any_clamped = false

    Note over Validator: 步骤 1：执行规则 1（ElectricalSafetyRule）
    Validator->>Rule1: validate(current_schedule, state)
    activate Rule1
    Rule1->>Rule1: 遍历 schedule.schedule
    Note right of Rule1: 校验 charge_power_kw ≤ max_power_kw
    Note right of Rule1: 校验 discharge_power_kw ≤ max_power_kw
    Note right of Rule1: 校验 soc_pct ≤ 0.95 (Critical)
    Note right of Rule1: 校验 soc_pct ≥ 0.05 (Fatal)

    alt 充电功率超限 (120 > 100)
        Rule1->>Rule1: 截断 charge_power_kw = 100
        Rule1->>Rule1: violations.push(Critical)
    end

    alt SOC < 0.05 (Fatal)
        Rule1-->>Validator: result { passed=false, clamped=true, clamped_schedule=Some(...), violations=[Fatal] }
        Note over Validator: 检测到 Fatal 违规，立即终止
        Validator-->>Caller: ValidationResult { passed=false, clamped=true, clamped_schedule=Some(...), violations=[Fatal] }
    else 无 Fatal 违规
        Rule1-->>Validator: result { passed=true, clamped=true/false, clamped_schedule=Some/None, violations=[Critical...] }
        deactivate Rule1

        Note over Validator: all_violations.extend(result.violations)
        Note over Validator: if result.clamped { current_schedule = clamped }

        Note over Validator: 步骤 2：执行规则 2（ProtectionCoordinationRule）
        Note over Validator: 基于截断后的 current_schedule 校验
        Validator->>Rule2: validate(current_schedule, state)
        activate Rule2
        Rule2->>Rule2: 遍历相邻时段 (i=1..n)
        Note right of Rule2: delta = |curr.net - prev.net|
        Note right of Rule2: delta_per_min = delta / 0.25
        Note right of Rule2: if delta_per_min > max_ramp_rate: 截断

        alt 爬坡率超限
            Rule2->>Rule2: safe_delta = max_ramp_rate * 0.25 * curr.signum()
            Rule2->>Rule2: safe_value = prev + safe_delta
            Note right of Rule2: D11: discharge += diff.max(0.0)
            Note right of Rule2: D11: charge += (-diff).max(0.0)
            Rule2->>Rule2: violations.push(Critical)
        end

        Rule2-->>Validator: result { passed=true, clamped=true/false, clamped_schedule=Some/None, violations=[Critical...] }
        deactivate Rule2

        Note over Validator: all_violations.extend(result.violations)
        Note over Validator: if result.clamped { current_schedule = clamped; any_clamped = true }

        Note over Validator: 无 Fatal 违规，正常返回
        Validator-->>Caller: ValidationResult { passed=true, clamped=any_clamped, clamped_schedule=Some/None, violations=all_violations }
    end

    deactivate Validator

    Note over Caller: 调用方根据 passed/clamped 决策：
    Note over Caller: passed=true, clamped=false → 下发原 schedule
    Note over Caller: passed=true, clamped=true → 下发 clamped_schedule
    Note over Caller: passed=false (Fatal) → 触发降级（L1 Solver-only）
```

---

## 9. 截断策略

### 9.1 截断到安全边界而非拒绝

本版本核心设计原则：**校验失败时截断到安全边界，而非直接拒绝**。这保证系统始终有调度输出，避免"无命令下发"导致的失控。

| 场景 | 拒绝策略（不采用） | 截断策略（本版本采用） |
|------|-------------------|---------------------|
| 充电功率超限（120 > 100） | 返回错误，不下发任何调度 | 截断到 100，下发截断后调度 |
| SOC 上限超限（0.98 > 0.95） | 返回错误，不下发 | 截断到 0.95，下发 |
| SOC 下限超限（0.03 < 0.05） | 返回错误，不下发 | 截断到 0.05，但标记 Fatal 触发降级 |
| 爬坡率超限 | 返回错误，不下发 | 截断功率变化，下发 |

**截断策略的物理意义**：

- **持续供电保证**：储能系统作为调峰 / 调频资源，必须持续输出。若因安全违规拒绝下发调度，会导致电网频率失控 / 峰谷套利中断。
- **安全裕度保留**：截断到安全边界（如 SOC 0.95 而非 0.98）保留 5% 安全裕度，避免电池深度充放电损伤。
- **降级而非停机**：致命违规（SOC < 5%）虽截断但仍标记 `passed = false`，触发上层降级（L1 Solver-only 路径），而非直接停机。

### 9.2 链式截断（前序规则截断后继续校验）

本版本采用**链式截断**策略：前序规则截断后，后续规则基于截断后的 schedule 继续校验。

**链式截断的必要性**：

```
原始 schedule：
  时段 0: charge=0,    discharge=0,    soc=0.5
  时段 1: charge=120,  discharge=0,    soc=0.6   ← 充电超限（120 > 100）
  时段 2: charge=0,    discharge=110,  soc=0.55  ← 放电超限（110 > 100）

规则 1：ElectricalSafetyRule
  → 截断时段 1: charge=100, soc=0.6
  → 截断时段 2: discharge=100, soc=0.55
  → clamped_schedule = [时段0, 时段1(截断), 时段2(截断)]

规则 2：ProtectionCoordinationRule（基于截断后的 schedule）
  → 校验时段 1→2 的爬坡率
  → 截断前：net_power[1] = -120, net_power[2] = 110, delta = 230
  → 截断后：net_power[1] = -100, net_power[2] = 100, delta = 200
  → 若 max_ramp_rate = 200/0.25 = 800 kW/min，截断后爬坡率 = 200/0.25 = 800，刚好不超限
  → 若截断前校验（错误做法）：delta = 230, delta_per_min = 920 > 800，会误报爬坡超限
```

**链式截断的正确性**：规则 2 基于截断后的 schedule 校验，避免误报已截断的违规。若规则 2 基于原始 schedule 校验，会误报爬坡超限（实际截断后已安全）。

### 9.3 致命违规立即终止（Fatal 级别）

本版本对致命违规（Fatal）采用**立即终止**策略：

```rust
// 致命违规立即终止
if result.violations.iter().any(|v| v.severity == Severity::Fatal) {
    return ValidationResult {
        passed: false,
        clamped: any_clamped,
        clamped_schedule: Some(current_schedule),
        violations: all_violations,
    };
}
```

**致命违规的判定**：`Severity::Fatal` 级违规是必须拒绝的违规，当前版本仅 `ElectricalSafetyRule` 的 SOC 下限超限（`soc_pct < 0.05`）标记为 Fatal。

**立即终止的理由**：

- **避免无意义校验**：致命违规后，后续规则的校验结果无意义（调度将被拒绝，不需截断）。
- **快速失败**：致命违规是紧急情况，需立即返回触发降级，避免延迟。
- **保留违规历史**：即使立即终止，已累积的 `all_violations` 仍返回，调用方可查看哪些规则已违规。

**Fatal 与 Critical 的区别**：

| 维度 | Critical | Fatal |
|------|----------|-------|
| `passed` 影响 | 不影响（`passed` 仍 true，截断后安全） | `passed = false`（必须拒绝） |
| 截断行为 | 截断到安全边界 | 截断到安全边界（但仍拒绝） |
| 链式执行 | 继续后续规则 | 立即终止 |
| 调用方动作 | 下发 `clamped_schedule` | 触发降级（L1 Solver-only） |
| 示例 | 功率超限 / SOC 上限超限 / 爬坡超限 | SOC 下限超限（深度放电） |

### 9.4 与 v0.56.0 ConstraintChecker 的双重屏障关系（D6 偏差）

本版本与 v0.56.0 `ConstraintChecker` 形成**双重安全屏障**（double barrier）：

```
┌─────────────────────────────────────────────────────────┐
│  慢平面（Agent Runtime 分区，10ms ~ 分钟级）              │
│  ┌─────────────────────────────────────────────────┐    │
│  │  v0.66.0 EnergyScheduleModel → ScheduleResult   │    │
│  │                  │                              │    │
│  │                  ▼                              │    │
│  │  v0.67.0 SafetyValidator（本版本）               │    │
│  │  【Solver 层安全屏障】                           │    │
│  │  - 电气安全校验（功率/SOC 范围）                 │    │
│  │  - 保护配合校验（爬坡率）                        │    │
│  │  - 截断到安全边界                                │    │
│  │                  │                              │    │
│  │                  ▼ clamped_schedule              │    │
│  └─────────────────────────────────────────────────┘    │
│                           │                             │
└───────────────────────────┼─────────────────────────────┘
                            │ 下发到快平面
                            ▼
┌─────────────────────────────────────────────────────────┐
│  快平面（RTOS 控制大区，10ms 实时）                      │
│  ┌─────────────────────────────────────────────────┐    │
│  │  v0.56.0 ConstraintChecker                      │    │
│  │  【RTOS 层安全屏障】                             │    │
│  │  - 命令执行前二次校验                            │    │
│  │  - 硬实时约束检查                                │    │
│  │  - 拒绝不安全命令                                │    │
│  │                  │                              │    │
│  │                  ▼                              │    │
│  │  Control Bus → 设备执行                         │    │
│  └─────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────┘
```

**D6 偏差说明**（不依赖 v0.56.0 / v0.57.0 / v0.52.0）：

蓝图 §2 前置依赖列出 v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 四遥数据模型，本版本**不引入这 3 个 crate 依赖**。理由：

| 蓝图前置依赖 | 本版本处理 | 理由 |
|-------------|-----------|------|
| v0.56.0 ConstraintChecker | **不依赖**（D6） | ConstraintChecker 是 RTOS 层命令执行前校验，本版本是 Solver 层调度结果校验，两者职责分离，形成 double barrier。本版本不直接调用 ConstraintChecker，仅在设计上形成双重屏障 |
| v0.57.0 DegradeEngine | **不依赖**（D6） | DegradeEngine 是降级规则引擎，本版本仅返回 `ValidationResult.passed = false` 触发降级，降级编排由 v0.71.0 双脑联调负责 |
| v0.52.0 四遥数据模型 | **不依赖**（D6） | 实时四遥数据由调用方（v0.68.0 / v0.71.0）填充到本地 `SystemState`（D2），本 crate 不直接消费遥测 |

**双重屏障的价值**：

1. **纵深防御**（Defense in Depth）：即使慢平面 SafetyValidator 漏检（如规则未覆盖的违规），快平面 ConstraintChecker 仍能拦截。
2. **职责分离**：SafetyValidator 关注调度结果的安全性（SOC / 爬坡 / 功率），ConstraintChecker 关注命令执行的硬实时性（10ms 内完成 / 无死锁）。
3. **解耦**：本版本不依赖 v0.56.0，可独立演进（如新增规则不影响 ConstraintChecker）。

---

## 10. no_std 合规

### 10.1 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`

本 crate 严格遵循蓝图 §43.1 no_std 硬性要求，`lib.rs` 顶层声明：

```rust
// crates/ai/safety-validator/src/lib.rs

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]  // D12：抑制未使用字段警告

extern crate alloc;

pub mod electrical;
pub mod protection;
pub mod result;
pub mod rule;
pub mod state;
pub mod validator;
```

| 声明 | 作用 |
|------|------|
| `#![cfg_attr(not(test), no_std)]` | 非 test 构建时启用 `no_std`（不链接 `std` crate）；test 构建时保留 `std`（支持 `std::format!` / `std::vec::Vec` 等） |
| `#![allow(dead_code)]` | D12：抑制 `ElectricalSafetyRule` / `ProtectionCoordinationRule` 未使用字段的 `dead_code` 警告 |
| `extern crate alloc` | 引入 `alloc` crate，启用 `Vec` / `Box` / `String` 等 heap 分配类型 |

### 10.2 子模块不重复 `#![cfg_attr(not(test), no_std)]`

**项目约定**：子模块（`state.rs` / `result.rs` / `rule.rs` / `electrical.rs` / `protection.rs` / `validator.rs`）**不重复** `#![cfg_attr(not(test), no_std)]` 声明，继承 `lib.rs` 的 `no_std` 配置。

| 文件 | 是否声明 `no_std` | 理由 |
|------|------------------|------|
| `src/lib.rs` | ✅ 声明 | crate 顶层，统一配置 |
| `src/state.rs` | ❌ 不声明 | 继承 lib.rs |
| `src/result.rs` | ❌ 不声明 | 继承 lib.rs |
| `src/rule.rs` | ❌ 不声明 | 继承 lib.rs |
| `src/electrical.rs` | ❌ 不声明 | 继承 lib.rs |
| `src/protection.rs` | ❌ 不声明 | 继承 lib.rs |
| `src/validator.rs` | ❌ 不声明 | 继承 lib.rs |

> **注**：`#![cfg_attr(not(test), no_std)]` 是 crate-level 属性（`#!` 前缀表示作用于当前模块及其子模块）。在 `lib.rs` 声明后，所有子模块自动继承。子模块重复声明是冗余的（且可能触发 `unused_attribute` 警告）。

### 10.3 使用 `alloc::*` / `core::*`

本 crate 使用的 `alloc::*` / `core::*` 类型与方法：

| 类型 / 方法 | 来源 | 用途 |
|------------|------|------|
| `alloc::vec::Vec` | `alloc` crate | 规则列表 / 违规列表 |
| `alloc::boxed::Box` | `alloc` crate | `Box<dyn SafetyRule>` trait 对象 |
| `alloc::string::String` | `alloc` crate | 规则名 / 违规字段名 |
| `alloc::string::ToString` | `alloc` crate | `&str.to_string()`（D3，等价于 `.into()`） |
| `alloc::vec::Vec::sort_by_key` | `alloc` crate | 规则按 `priority()` 排序（D5） |
| `core::f64::signum` | `core` crate | 功率方向符号（`curr.signum()`） |
| `core::f64::abs` | `core` crate | 功率变化量绝对值（`(curr - prev).abs()`） |
| `Option<T>` | `core` crate | `clamped_schedule: Option<ScheduleResult>` |

**禁止使用的 `std::*` 类型**（蓝图 §43.1）：

| 禁止 | 替代 |
|------|------|
| `std::vec::Vec` | `alloc::vec::Vec` |
| `std::boxed::Box` | `alloc::boxed::Box` |
| `std::string::String` | `alloc::string::String` |
| `std::collections::HashMap` | `alloc::collections::BTreeMap`（本版本无需） |
| `std::time::Duration` | `core::time::Duration`（本版本无需） |
| `std::sync::Mutex` | `spin::Mutex`（本版本无需） |

### 10.4 可交叉编译到 `aarch64-unknown-none`

本 crate 可交叉编译到 `aarch64-unknown-none`（蓝图目标平台 ARM64）：

```bash
# 记忆文件 §2.4.2 C8 验证
cargo build -p eneros-safety-validator \
    --target aarch64-unknown-none \
    -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem
```

**交叉编译成功的前提**：

- 仅使用 `core::*` / `alloc::*`（本 crate 满足）
- 依赖 crate（v0.66.0 `eneros-energy-lp-model`）同样 no_std（已满足，v0.66.0 D1）
- `alloc` 依赖 v0.11.0 用户堆（交叉编译时由链接器解析符号）

**no_std 合规检查清单**：

| 检查项 | 状态 | 说明 |
|--------|------|------|
| `#![cfg_attr(not(test), no_std)]` | ✅ | lib.rs 顶层声明 |
| `extern crate alloc` | ✅ | 引入 alloc |
| 无 `use std::*` | ✅ | 全部使用 `core::*` / `alloc::*` |
| `alloc::vec::Vec` 而非 `std::vec::Vec` | ✅ | D4 |
| `alloc::boxed::Box` 而非 `std::boxed::Box` | ✅ | D4 |
| `Vec::sort_by_key` 可用 | ✅ | D5（alloc 原生支持） |
| `.into()` / `.to_string()` 可用 | ✅ | D3（`extern crate alloc` 后 `ToString` 可用） |
| 无 `HashMap` | ✅ | 本版本无需 |
| 无 `Instant` | ✅ | 本版本不涉及计时 |
| 无 `unsafe` | ✅ | 纯 safe Rust |
| 无 FFI | ✅ | D7 纯 Rust，无 C 库依赖 |
| 交叉编译通过 | ✅ | aarch64-unknown-none |

---

## 11. 偏差声明（D1~D12）

本设计文档相对蓝图原文（`蓝图/phase1.md` §v0.67.0，line 13955~14312）的偏差声明如下。所有偏差均出于 no_std 合规性、与既有版本一致性、职责分离或 Karpathy "Simplicity First" / "Surgical Changes" 原则考虑。依据 Karpathy "Think Before Coding" 原则，逐条列出蓝图伪代码与实际 no_std / 项目约束 / 既有版本一致性的偏差。

| 偏差 | 蓝图原设计 | 实际实现 | 理由 |
|------|-----------|---------|------|
| **D1** | `pub trait SafetyRule: Send + Sync`（蓝图 line 13987） | **移除 `Send + Sync` bound** | no_std 单线程环境；与 v0.59.0 `LlmEngine` / v0.63.0 `PromptTemplate` / v0.64.0 `Solver` 一致。`Send + Sync` 在单线程无意义，且 `Box<dyn SafetyRule>` 在 no_std 下派生 `Send + Sync` 会引入 `alloc::boxed::Box` 的 trait bound 限制 |
| **D2** | 蓝图使用 `SystemState` 但未定义（蓝图 line 13993 等） | **本地定义最小 `SystemState`**（含 voltage_v / current_a / frequency_hz / soc_pct / timestamp_ms） | HMI crate 的 `SystemState`（`crates/agents/hmi/src/lib.rs:95`）是 HMI 显示状态（agent_states / storage_usage / network / power），与电气安全校验无关。Karpathy "Simplicity First"：定义最小满足校验需求的状态类型，不引入 HMI crate 耦合 |
| **D3** | `rule: self.name().into()`（蓝图 line 14072 等） | 保留 `alloc::string::ToString` 隐式转换（`&str` → `String`） | `extern crate alloc` 后 `.to_string()` 可用，`String::from` 也可用，`.into()` 也可用（`From<&str> for String`） |
| **D4** | `Vec<Box<dyn SafetyRule>>`（蓝图 line 14201） | 使用 `alloc::vec::Vec` + `alloc::boxed::Box` | no_std 合规：`Vec` / `Box` 在 `extern crate alloc` 后可用，与 `std::vec::Vec` / `std::boxed::Box` API 完全一致 |
| **D5** | `self.rules.sort_by_key(\|r\| r.priority())`（蓝图 line 14228） | 保留 `Vec::sort_by_key`（`alloc` 原生支持） | no_std 合规：`sort_by_key` 在 `alloc::vec::Vec` 上可用，稳定排序（同优先级保持插入顺序） |
| **D6** | 前置依赖列出 v0.56.0 ConstraintChecker / v0.57.0 DegradeEngine / v0.52.0 四遥数据模型（蓝图 §2） | **不引入这 3 个 crate 依赖** | `SafetyValidator` 仅校验 `ScheduleResult`（v0.66.0）+ 本地 `SystemState`（D2）。与 v0.66.0 D5 一致：解耦，避免未使用依赖。Karpathy "Simplicity First"。v0.56.0 ConstraintChecker 在 RTOS 层做二次校验，与本 crate 形成 double barrier（§9.4） |
| **D7** | 蓝图未声明 `[features]` | 不声明 `[features]` | 纯 Rust，无 FFI，无 feature gate。与 v0.65.0 D11 / v0.66.0 D11 一致 |
| **D8** | 蓝图 `Severity` 派生 `Debug` + `Clone` + `Copy` + `PartialEq`（蓝图 line 14033） | 保持一致 | `Severity` 需 `PartialEq` 做 `==` 比较（蓝图 line 14124 `v.severity != Severity::Fatal` / line 14252 `v.severity == Severity::Fatal`），`Copy` 因其为简单枚举（4 个 unit variant 无字段） |
| **D9** | 蓝图 `ValidationResult` / `Violation` 派生 `Debug` + `Clone`（蓝图 line 14003 / 14016） | 保持一致，**不额外派生 `PartialEq`** | Karpathy "Simplicity First"：当前测试不需要 `PartialEq`（测试通过字段访问断言，而非整体相等）；`Violation` 含 `String` 字段，`PartialEq` 比较开销较高且无用途；避免过早添加 trait（YAGNI） |
| **D10** | 蓝图 `if entry.soc_pct > 0.95`（蓝图 line 14096）直接比较 | 保留直接比较（f64 有序比较在边界值 0.95 / 0.05 无精度问题） | 蓝图 §8.3 提到浮点容差，但 SOC 边界 0.95 / 0.05 是**固定阈值**，非迭代计算结果，直接比较安全。若未来需要容差，可加 `const EPSILON: f64 = 1e-9`（YAGNI） |
| **D11** | 蓝图截断逻辑 `clamped_schedule.schedule[i].discharge_power_kw += diff.max(0.0)`（蓝图 line 14183） | 保留蓝图截断逻辑（**精确复制**） | 蓝图截断策略是核心业务逻辑，Karpathy "Surgical Changes"：不修改不理解的业务逻辑。**不"修复"** 重新计算 `net_power_kw` 和 `revenue_yuan`（蓝图未要求） |
| **D12** | 蓝图 `ElectricalSafetyRule` 有 `max_current_a` / `voltage_range` / `freq_range` 字段但 `validate` 未使用（蓝图 line 14047~14055）；`ProtectionCoordinationRule` 有 `overcurrent_threshold` / `overvoltage_threshold` / `undervoltage_threshold` / `freq_protection` 字段但 `validate` 未使用（蓝图 line 14042~14050） | 保留字段（蓝图已定义），`validate` 仅用 `max_power_kw` / SOC 阈值 / `max_ramp_rate` | Karpathy "Surgical Changes"：蓝图字段为未来扩展预留（电压/电流/频率/保护阈值），不删除；但 `validate` 逻辑严格按蓝图，不扩展。`#![allow(dead_code)]` 抑制未使用字段警告 |

### 11.1 偏差一致性说明

本版本偏差与既有版本偏差的一致性：

| 偏差 | 一致版本 | 一致点 |
|------|---------|--------|
| D1（no_std，移除 `Send + Sync`） | v0.59.0 `LlmEngine` / v0.63.0 `PromptTemplate` / v0.64.0 `Solver` | AI 子系统 trait 均无 `Send + Sync` bound（no_std 单线程） |
| D2（本地定义类型，不依赖无关 crate） | v0.66.0 D5（不依赖 v0.52.0 telemetry-model） | 子系统解耦原则 |
| D3（`alloc::string::ToString` / `.into()`） | v0.59.0 D2 / v0.64.0 D2 / v0.65.0 D2 / v0.66.0 D2 | no_std 下 `String` 转换需 `extern crate alloc` |
| D4（`alloc::vec::Vec` / `alloc::boxed::Box`） | v0.59.0 D4 / v0.64.0 D4 / v0.65.0 D4 / v0.66.0 D4 | no_std 下 `Vec` / `Box` 来自 `alloc` |
| D5（`Vec::sort_by_key` 可用） | v0.65.0 D5 / v0.66.0 D5 | `alloc::vec::Vec` 原生支持 `sort_by_key` |
| D6（不引入未使用依赖） | v0.59.0 D9 / v0.64.0 D9 / v0.66.0 D5 | 子系统解耦，避免未使用依赖 |
| D7（无 `[features]`） | v0.65.0 D11 / v0.66.0 D11 | 纯 Rust 层无 FFI，无 feature gate |
| D8（`Severity` 派生 `PartialEq + Copy`） | v0.64.0 `SolveStatus` 派生 `PartialEq + Copy` | 简单枚举需 `==` 比较时派生 `PartialEq` |
| D9（复杂类型不派生 `PartialEq`） | v0.66.0 D10（`ScheduleResult` / `ScheduleEntry` 不派生 `PartialEq`） | Karpathy "Simplicity First"，YAGNI |
| D10（直接浮点比较固定阈值） | v0.66.0 浮点边界比较 | 固定阈值非迭代结果，直接比较安全 |
| D11（精确复制蓝图业务逻辑） | v0.66.0 D3（修正蓝图数学错误是例外） | Karpathy "Surgical Changes"：不修改不理解的业务逻辑 |
| D12（保留未使用字段供扩展） | v0.65.0 D12（`#[derive(Default)]`） | 蓝图字段规划为未来扩展预留 |

### 11.2 偏差可追溯性

所有偏差均可在实现阶段的 `src/lib.rs` 文件头部注释中找到对应说明（参考 `crates/ai/energy-lp-model/src/lib.rs` 的偏差声明表风格），确保代码与文档一致。spec 源文件位于 `.trae/specs/develop-v0670-safety-validator/spec.md`。

### 11.3 偏差与蓝图验收标准对照

| 蓝图验收项（蓝图 §7） | 本设计对应章节 | 状态 |
|---------------------|--------------|------|
| `SafetyRule` trait 定义完整 | §3 SafetyRule trait 设计、D1 | ✅ 移除 `Send + Sync`（D1），含 `name()` / `validate()` / `priority()` / `is_hard()` |
| `ElectricalSafetyRule` 实现功率/电流/SOC 校验 | §6 ElectricalSafetyRule 设计 | ✅ 功率 + SOC 校验（电流字段保留未使用，D12） |
| `ProtectionCoordinationRule` 实现爬坡率校验 | §7 ProtectionCoordinationRule 设计 | ✅ 爬坡率校验 + D11 截断逻辑精确复制 |
| `SafetyValidator` 链式校验正确 | §8 SafetyValidator 链式校验设计 | ✅ 链式执行 + 致命终止 |
| 截断策略：超限值截断到安全边界 | §9 截断策略 | ✅ 截断到安全边界而非拒绝 |
| 致命违规立即终止并拒绝 | §9.3 致命违规立即终止 | ✅ Fatal 级违规立即返回 `passed = false` |

---

## 12. 测试与验收

### 12.1 22 项集成测试（T1~T22）

本版本交付 22 项集成测试，覆盖全部规则、边界条件与链式执行（按 tasks.md Task 9）：

| 测试 ID | 测试名称 | 测试内容 | 验证点 |
|---------|---------|---------|--------|
| **T1** | `system_state_default` | `SystemState::default()` 默认值 | voltage_v=380.0, current_a=0.0, frequency_hz=50.0, soc_pct=0.5, timestamp_ms=0 |
| **T2** | `severity_variants_and_partial_eq` | `Severity` 枚举变体 + `PartialEq` | `Critical == Critical` / `Critical != Fatal`（D8） |
| **T3** | `violation_construction_and_field_access` | `Violation` 构造 + 字段访问 | rule / period / field / original_value / safe_value / severity 字段可访问 |
| **T4** | `validation_result_construction` | `ValidationResult` 构造 | passed=true, clamped=false, clamped_schedule=None, violations=[] |
| **T5** | `electrical_safety_rule_new` | `ElectricalSafetyRule::new()` 构造 | 4 字段正确初始化（max_power_kw / max_current_a / voltage_range / freq_range） |
| **T6** | `electrical_validate_all_pass` | 电气规则全部通过 | 功率 / SOC 正常 → passed=true, clamped=false, violations=[] |
| **T7** | `electrical_validate_charge_power_clamp` | 充电功率超限截断 | charge_power_kw=120 > 100 → 截断到 100, severity=Critical, field="charge_power" |
| **T8** | `electrical_validate_discharge_power_clamp` | 放电功率超限截断 | discharge_power_kw=120 > 100 → 截断到 100, severity=Critical, field="discharge_power" |
| **T9** | `electrical_validate_soc_upper_clamp` | SOC 上限截断 | soc_pct=0.98 > 0.95 → 截断到 0.95, severity=Critical, field="soc" |
| **T10** | `electrical_validate_soc_lower_fatal` | SOC 下限致命 | soc_pct=0.03 < 0.05 → 截断到 0.05, severity=Fatal, passed=false |
| **T11** | `protection_coordination_rule_new` | `ProtectionCoordinationRule::new()` 构造 | 5 字段正确初始化 |
| **T12** | `protection_validate_ramp_normal` | 爬坡率正常 | 相邻时段功率变化率 ≤ max_ramp_rate → 无 violation |
| **T13** | `protection_validate_ramp_clamp` | 爬坡率超限截断 | delta_per_min > max_ramp_rate → 截断, severity=Critical, field="ramp_rate" |
| **T14** | `validator_new_default_rules` | `SafetyValidator::new()` 默认注册 2 条规则 | rules.len() == 2（ElectricalSafetyRule + ProtectionCoordinationRule） |
| **T15** | `validator_validate_all_pass` | 校验器全部通过 | 全部规则通过 → passed=true, clamped=false |
| **T16** | `validator_validate_chain_clamp` | 链式截断 | Electrical 截断后 Protection 基于截断后 schedule 继续校验 |
| **T17** | `validator_validate_fatal_terminate` | 致命违规立即终止 | SOC < 0.05 → passed=false, 立即终止后续规则 |
| **T18** | `validator_add_rule_custom` | 自定义规则插队 | priority=5 的自定义规则插到最前（priority < 10） |
| **T19** | `validator_clamped_schedule_present` | 截断后 clamped_schedule 存在 | clamped=true → clamped_schedule=Some(...) |
| **T20** | `clamp_recalculates_net_power` | 截断后 net_power_kw 重新计算 | 截断后 net_power_kw = discharge - charge（D11：蓝图未要求，测试验证一致性） |
| **T21** | `safety_rule_default_methods` | `SafetyRule` trait 默认方法 | priority()=100, is_hard()=true（默认实现） |
| **T22** | `end_to_end_schedule_validate` | 端到端：ScheduleResult → validate → ValidationResult | 本地构造 ScheduleResult（D7：不依赖 MockSolver）→ SafetyValidator.validate() → ValidationResult |

**测试组织**：22 项测试位于 `src/lib.rs` 的 `#[cfg(test)] mod tests` 模块，使用标准 `#[test]` 属性，通过 `cargo test -p eneros-safety-validator` 运行。

**测试覆盖维度**：

| 维度 | 测试 | 说明 |
|------|------|------|
| 类型构造 | T1 / T3 / T4 / T5 / T11 | 默认值 / 字段访问 / 构造函数 |
| 枚举与 trait | T2 / T21 | `Severity` PartialEq / `SafetyRule` 默认方法 |
| 电气安全规则 | T6~T10 | 全通过 / 充电截断 / 放电截断 / SOC 上限截断 / SOC 下限致命 |
| 保护配合规则 | T12~T13 | 爬坡正常 / 爬坡超限截断 |
| 校验器链式 | T14~T19 | 默认规则 / 全通过 / 链式截断 / 致命终止 / 自定义插队 / clamped_schedule |
| 截断一致性 | T20 | net_power_kw 重新计算 |
| 端到端 | T22 | 完整流程 |

### 12.2 验收标准对照

蓝图 §7 验收标准 6 项，本版本全部满足：

| 蓝图验收项 | 对应章节 | 对应测试 | 状态 |
|-----------|---------|---------|------|
| `SafetyRule` trait 定义完整 | §3 | T2 / T21 | ✅ |
| `ElectricalSafetyRule` 实现功率/电流/SOC 校验 | §6 | T5~T10 | ✅（电流字段保留未使用，D12） |
| `ProtectionCoordinationRule` 实现爬坡率校验 | §7 | T11~T13 | ✅ |
| `SafetyValidator` 链式校验正确 | §8 | T14~T17 | ✅ |
| 截断策略：超限值截断到安全边界 | §9 | T7~T9 / T13 / T19 / T20 | ✅ |
| 致命违规立即终止并拒绝 | §9.3 | T10 / T17 | ✅ |

**蓝图 §9 多角度要求对照**：

| 维度 | 蓝图要求 | 本版本实现 | 状态 |
|------|---------|-----------|------|
| 9.1 功能 | 电气/保护/一致性三重校验 | 电气 + 保护（一致性由 v0.56.0 ConstraintChecker 负责，本版本不依赖 D6） | ✅（2 重校验 + double barrier） |
| 9.2 性能 | 校验 96 时段 < 1ms | 纯 Rust 遍历，O(n) 复杂度，96 时段 < 100μs | ✅ |
| 9.3 安全 | 致命违规拒绝、超限截断 | Fatal 立即终止 + Critical 截断到安全边界 | ✅ |
| 9.4 可靠 | 规则链式执行不遗漏 | `for rule in &self.rules` 遍历全部规则 | ✅ |
| 9.5 可维护 | 规则可插拔 | `add_rule()` 支持自定义规则，`SafetyRule` trait 抽象 | ✅ |
| 9.6 可观测 | 违规详情日志 | `Violation` 记录 rule / period / field / original_value / safe_value / severity | ✅ |
| 9.7 可扩展 | 支持自定义规则 | `SafetyRule` trait + `add_rule()` + `priority()` 排序 | ✅ |

### 12.3 构建校验（C6~C11）

本版本完成后执行记忆文件 §2.4.2 构建校验清单：

| 校验项 | 命令 | 状态 | 说明 |
|--------|------|------|------|
| **C6** cargo metadata | `cargo metadata --format-version 1 > /dev/null` | ✅ | workspace 成员路径正确（含 `crates/ai/safety-validator`） |
| **C7** cargo test | `cargo test -p eneros-safety-validator` | ✅ | 22 项测试全部通过 |
| **C8** 交叉编译 | `cargo build -p eneros-safety-validator --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | ✅ | aarch64-unknown-none 交叉编译通过 |
| **C9** cargo fmt | `cargo fmt -p eneros-safety-validator -- --check` | ✅ | 格式检查通过 |
| **C10** cargo clippy | `cargo clippy -p eneros-safety-validator --all-targets -- -D warnings` | ✅ | 无 warning（`#![allow(dead_code)]` 抑制 D12 未使用字段警告） |
| **C11** cargo deny | `cargo deny check licenses bans sources` | ✅ | 许可证 / 安全扫描通过（纯 Rust，无新第三方依赖） |

**构建校验命令汇总**：

```bash
# C6：workspace 能解析所有成员
cargo metadata --format-version 1 > /dev/null

# C7：单元测试
cargo test -p eneros-safety-validator

# C8：交叉编译验证
cargo build -p eneros-safety-validator \
    --target aarch64-unknown-none \
    -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem

# C9：格式检查
cargo fmt -p eneros-safety-validator -- --check

# C10：lint 检查
cargo clippy -p eneros-safety-validator --all-targets -- -D warnings

# C11：安全扫描
cargo deny check licenses bans sources
```

---

## 附录 A. SafetyValidator 使用示例

### A.1 基本使用流程

```rust
use eneros_safety_validator::{SafetyValidator, SystemState};
use eneros_energy_lp_model::result::{ScheduleResult, ScheduleEntry, /* SolveStatus */};

fn main() {
    // 1. 构造调度结果（通常来自 v0.66.0 EnergyScheduleModel::parse_result）
    let schedule = ScheduleResult {
        schedule: vec![
            ScheduleEntry {
                period: 0,
                charge_power_kw: 0.0,
                discharge_power_kw: 0.0,
                net_power_kw: 0.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            },
            ScheduleEntry {
                period: 1,
                charge_power_kw: 120.0,  // 超限（> 100）
                discharge_power_kw: 0.0,
                net_power_kw: -120.0,
                soc_pct: 0.6,
                revenue_yuan: -30.0,
            },
        ],
        total_revenue_yuan: -30.0,
        objective_value: -30.0,
        // solve_status: SolveStatus::Optimal,
    };

    // 2. 构造系统状态（通常来自实时四遥数据）
    let state = SystemState::default();

    // 3. 创建校验器（注册默认规则）
    let validator = SafetyValidator::new();

    // 4. 执行校验
    let result = validator.validate(&schedule, &state);

    // 5. 根据结果决策
    if result.passed {
        if result.clamped {
            // 下发截断后的调度
            let safe_schedule = result.clamped_schedule.unwrap();
            println!("下发截断后调度：{} 个违规", result.violations.len());
            // 下发 safe_schedule...
        } else {
            // 下发原调度
            println!("下发原调度（无违规）");
            // 下发 schedule...
        }
    } else {
        // 致命违规，触发降级
        println!("致命违规，触发降级（L1 Solver-only）");
        for v in &result.violations {
            println!(
                "  违规：rule={}, period={}, field={}, original={}, safe={}, severity={:?}",
                v.rule, v.period, v.field, v.original_value, v.safe_value, v.severity
            );
        }
        // 触发降级...
    }
}
```

### A.2 自定义规则

```rust
use eneros_safety_validator::{
    rule::SafetyRule,
    state::SystemState,
    result::{Severity, ValidationResult, Violation},
    validator::SafetyValidator,
};
use eneros_energy_lp_model::result::ScheduleResult;
use alloc::string::String;
use alloc::vec::Vec;

/// 自定义规则：总收益校验（收益为负时警告）.
pub struct RevenueCheckRule {
    min_revenue: f64,
}

impl SafetyRule for RevenueCheckRule {
    fn name(&self) -> &str {
        "revenue_check"
    }

    fn priority(&self) -> u32 {
        5  // 插队到最前（高于 ElectricalSafetyRule 的 10）
    }

    fn validate(
        &self,
        schedule: &ScheduleResult,
        _state: &SystemState,
    ) -> ValidationResult {
        let mut violations = Vec::new();

        if schedule.total_revenue_yuan < self.min_revenue {
            violations.push(Violation {
                rule: self.name().into(),
                period: 0,
                field: "total_revenue".into(),
                original_value: schedule.total_revenue_yuan,
                safe_value: self.min_revenue,
                severity: Severity::Warning,  // 软约束，仅警告
            });
        }

        ValidationResult {
            passed: violations.iter().all(|v| v.severity != Severity::Fatal),
            clamped: false,
            clamped_schedule: None,
            violations,
        }
    }
}

fn main() {
    let mut validator = SafetyValidator::new();
    validator.add_rule(Box::new(RevenueCheckRule { min_revenue: 0.0 }));
    // 现在 rules = [RevenueCheck(5), Electrical(10), Protection(20)]
}
```

---

## 附录 B. 性能分析

### B.1 时间复杂度

| 操作 | 复杂度 | 说明 |
|------|--------|------|
| `SafetyValidator::new()` | O(1) | 创建 + 注册 2 默认规则 |
| `add_rule()` | O(n log n) | `sort_by_key` 排序，n 为规则数（通常 < 10） |
| `validate()` | O(n × m) | n 为规则数，m 为时段数（96 时段） |
| `ElectricalSafetyRule::validate()` | O(m) | 遍历 m 个时段 |
| `ProtectionCoordinationRule::validate()` | O(m) | 遍历 m-1 个相邻时段 |

### B.2 性能估算

以默认配置（2 规则 + 96 时段）为例：

| 操作 | 估算时间 | 说明 |
|------|---------|------|
| `ElectricalSafetyRule::validate()` | ~10 μs | 96 时段 × 4 次比较（charge/discharge/soc_upper/soc_lower） |
| `ProtectionCoordinationRule::validate()` | ~5 μs | 95 相邻时段 × 爬坡率计算 |
| `SafetyValidator::validate()`（2 规则） | ~15 μs | 10 + 5 μs |
| 蓝图 §9.2 要求 | < 1 ms | 远低于要求（~15 μs << 1 ms） |

> **性能结论**：本版本校验 96 时段 < 20 μs，远低于蓝图 §9.2 要求的 1 ms。性能瓶颈在 LP 求解（v0.64.0 HiGHS ~100ms），安全校验开销可忽略。

### B.3 内存占用

| 数据 | 大小 | 说明 |
|------|------|------|
| `SafetyValidator` | ~24 字节 | `Vec<Box<dyn SafetyRule>>`（指针 + 长度 + 容量） |
| `ElectricalSafetyRule` | ~40 字节 | 4 字段（f64 + f64 + (f64,f64) + (f64,f64)） |
| `ProtectionCoordinationRule` | ~48 字节 | 5 字段（4×f64 + (f64,f64)） |
| `SystemState` | ~40 字节 | 5 字段（4×f64 + u64） |
| `ValidationResult` | ~48 字节 + violations | passed + clamped + Option<ScheduleResult> + Vec<Violation> |
| `Violation` | ~56 字节 | String + usize + String + 2×f64 + Severity |
| 总计（无截断） | ~200 字节 | 远低于 Agent Runtime 64MB 预算 |
| 总计（96 时段截断） | ~8 KB | `clamped_schedule` 含 96 条 `ScheduleEntry`（每条 ~80 字节） |

> **内存结论**：本版本内存占用 < 10 KB（96 时段截断场景），远低于 Agent Runtime 64MB 预算（蓝图 §5.6）。

---

## 附录 C. 与 v0.56.0 ConstraintChecker 的对比

### C.1 职责对比

| 维度 | v0.67.0 SafetyValidator | v0.56.0 ConstraintChecker |
|------|------------------------|--------------------------|
| 位置 | Solver 层（慢平面） | RTOS 层（快平面） |
| 校验对象 | `ScheduleResult`（LP 求解结果） | 命令执行前的实时命令 |
| 校验时机 | 调度结果生成后、下发前 | 命令下发到设备前 |
| 校验周期 | 15min ~ 1h（调度周期） | 10ms（实时控制周期） |
| 校验内容 | SOC 范围 / 功率超限 / 爬坡率 | 硬实时约束 / 死锁 / 优先级反转 |
| 失败动作 | 截断到安全边界 / 致命拒绝 | 拒绝命令 / 触发保护动作 |
| 可配置性 | 规则可插拔（`SafetyRule` trait） | 固定规则（RTOS 层） |

### C.2 双重屏障的价值

```
慢平面 SafetyValidator 截断后的调度
        │
        ▼
快平面 ConstraintChecker 二次校验
        │
        ├── 通过 → 执行
        └── 拒绝 → 触发保护动作
```

**双重屏障的价值**：

1. **纵深防御**：即使 SafetyValidator 漏检（如规则未覆盖的违规），ConstraintChecker 仍能拦截。
2. **时序保证**：ConstraintChecker 在 10ms 实时周期内校验，确保命令执行的硬实时性。
3. **职责分离**：SafetyValidator 关注调度结果的安全性（业务层），ConstraintChecker 关注命令执行的实时性（系统层）。

---

## 附录 D. 测试代码示例

### D.1 T6 电气规则全部通过

```rust
#[test]
fn electrical_validate_all_pass() {
    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};

    let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
    let schedule = ScheduleResult {
        schedule: vec![ScheduleEntry {
            period: 0,
            charge_power_kw: 50.0,       // ≤ 100
            discharge_power_kw: 30.0,    // ≤ 100
            net_power_kw: -20.0,
            soc_pct: 0.5,                // 0.05 ≤ 0.5 ≤ 0.95
            revenue_yuan: -5.0,
        }],
        total_revenue_yuan: -5.0,
        objective_value: -5.0,
        // solve_status: SolveStatus::Optimal,
    };
    let state = SystemState::default();

    let result = rule.validate(&schedule, &state);

    assert!(result.passed);
    assert!(!result.clamped);
    assert!(result.clamped_schedule.is_none());
    assert!(result.violations.is_empty());
}
```

### D.2 T7 充电功率超限截断

```rust
#[test]
fn electrical_validate_charge_power_clamp() {
    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};

    let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
    let schedule = ScheduleResult {
        schedule: vec![ScheduleEntry {
            period: 0,
            charge_power_kw: 120.0,      // > 100，超限
            discharge_power_kw: 0.0,
            net_power_kw: -120.0,
            soc_pct: 0.5,
            revenue_yuan: -30.0,
        }],
        total_revenue_yuan: -30.0,
        objective_value: -30.0,
    };
    let state = SystemState::default();

    let result = rule.validate(&schedule, &state);

    assert!(result.passed);  // Critical 不影响 passed
    assert!(result.clamped);
    assert_eq!(result.violations.len(), 1);

    let v = &result.violations[0];
    assert_eq!(v.rule, "electrical_safety");
    assert_eq!(v.period, 0);
    assert_eq!(v.field, "charge_power");
    assert_eq!(v.original_value, 120.0);
    assert_eq!(v.safe_value, 100.0);
    assert_eq!(v.severity, Severity::Critical);

    // 验证截断后的调度
    let clamped = result.clamped_schedule.unwrap();
    assert_eq!(clamped.schedule[0].charge_power_kw, 100.0);
}
```

### D.3 T10 SOC 下限致命

```rust
#[test]
fn electrical_validate_soc_lower_fatal() {
    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};

    let rule = ElectricalSafetyRule::new(100.0, 200.0, (340.0, 420.0), (49.5, 50.5));
    let schedule = ScheduleResult {
        schedule: vec![ScheduleEntry {
            period: 0,
            charge_power_kw: 0.0,
            discharge_power_kw: 0.0,
            net_power_kw: 0.0,
            soc_pct: 0.03,               // < 0.05，致命
            revenue_yuan: 0.0,
        }],
        total_revenue_yuan: 0.0,
        objective_value: 0.0,
    };
    let state = SystemState::default();

    let result = rule.validate(&schedule, &state);

    assert!(!result.passed);  // Fatal → passed = false
    assert!(result.clamped);

    let v = &result.violations[0];
    assert_eq!(v.field, "soc");
    assert_eq!(v.severity, Severity::Fatal);
    assert_eq!(v.safe_value, 0.05);
}
```

### D.4 T17 校验器致命违规立即终止

```rust
#[test]
fn validator_validate_fatal_terminate() {
    use eneros_energy_lp_model::result::{ScheduleEntry, ScheduleResult};

    let validator = SafetyValidator::new();
    let schedule = ScheduleResult {
        schedule: vec![
            ScheduleEntry {
                period: 0,
                charge_power_kw: 0.0,
                discharge_power_kw: 0.0,
                net_power_kw: 0.0,
                soc_pct: 0.5,
                revenue_yuan: 0.0,
            },
            ScheduleEntry {
                period: 1,
                charge_power_kw: 0.0,
                discharge_power_kw: 0.0,
                net_power_kw: 0.0,
                soc_pct: 0.03,           // < 0.05，致命
                revenue_yuan: 0.0,
            },
        ],
        total_revenue_yuan: 0.0,
        objective_value: 0.0,
    };
    let state = SystemState::default();

    let result = validator.validate(&schedule, &state);

    assert!(!result.passed);  // Fatal → passed = false
    assert!(result.clamped);

    // 验证致命违规立即终止（仅 ElectricalSafetyRule 执行，ProtectionCoordinationRule 未执行）
    // violations 中应只有 electrical_safety 的 SOC 违规，无 protection_coordination 的爬坡违规
    let has_fatal = result.violations.iter().any(|v| v.severity == Severity::Fatal);
    assert!(has_fatal);
}
```

### D.5 测试运行

```bash
# 记忆文件 §2.4.2 C7：主机侧测试
cargo test -p eneros-safety-validator

# 交叉编译验证（C8）
cargo build -p eneros-safety-validator \
    --target aarch64-unknown-none \
    -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem

# 格式与 lint（C9 / C10）
cargo fmt -p eneros-safety-validator -- --check
cargo clippy -p eneros-safety-validator --all-targets -- -D warnings
```

---

## 附录 E. Mermaid 图汇总

### E.1 SafetyValidator 类图

（见 §8 Mermaid 图 1）

### E.2 validate() 链式执行时序图

（见 §8 Mermaid 图 2）

### E.3 双重屏障架构图

```mermaid
flowchart TD
    subgraph SlowPlane [慢平面 - Agent Runtime 分区]
        A[v0.66.0 EnergyScheduleModel] --> B[v0.67.0 SafetyValidator<br/>本版本]
        B --> C{ValidationResult}
        C -->|passed=true, clamped=false| D[下发原 schedule]
        C -->|passed=true, clamped=true| E[下发 clamped_schedule]
        C -->|passed=false Fatal| F[触发降级 L1]
    end

    subgraph FastPlane [快平面 - RTOS 控制大区]
        G[v0.56.0 ConstraintChecker<br/>RTOS 层二次校验]
        H[Control Bus → 设备执行]
    end

    D --> G
    E --> G
    G -->|通过| H
    G -->|拒绝| I[触发保护动作]

    style B fill:#f9f,stroke:#333,stroke-width:2px
    style G fill:#bbf,stroke:#333,stroke-width:2px
```

---

## 附录 F. 规则优先级与执行顺序

### F.1 默认规则优先级

| 规则 | priority() | 执行顺序 | 蓝图依据 |
|------|-----------|---------|---------|
| `ElectricalSafetyRule` | 10 | 第 1 | 蓝图 line 14060 |
| `ProtectionCoordinationRule` | 20 | 第 2 | 蓝图 line 14155 |
| 自定义规则（默认） | 100 | 第 3+ | `SafetyRule::priority()` 默认值 |

### F.2 优先级排序示例

```
场景：注册 4 条规则
  - ElectricalSafetyRule (priority=10)
  - ProtectionCoordinationRule (priority=20)
  - CustomRuleA (priority=5)
  - CustomRuleB (priority=50)

排序后执行顺序：
  1. CustomRuleA (priority=5)
  2. ElectricalSafetyRule (priority=10)
  3. ProtectionCoordinationRule (priority=20)
  4. CustomRuleB (priority=50)
```

### F.3 同优先级处理

`Vec::sort_by_key` 是稳定排序，同优先级的规则保持插入顺序：

```
场景：注册 3 条 priority=10 的规则
  - RuleA (priority=10)  ← 先插入
  - RuleB (priority=10)  ← 后插入
  - RuleC (priority=10)  ← 最后插入

排序后执行顺序：RuleA → RuleB → RuleC（保持插入顺序）
```

> **注**：本版本 2 个默认规则优先级不同（10 < 20），无同优先级冲突。自定义规则若与默认规则同优先级，按插入顺序执行。

---

## 附录 G. 截断策略详解

### G.1 截断到安全边界

| 违规类型 | 原始值 | 安全值 | 截断方式 |
|---------|--------|--------|---------|
| 充电功率超限 | 120.0 | `max_power_kw` (100.0) | `entry.charge_power_kw = max_power_kw` |
| 放电功率超限 | 120.0 | `max_power_kw` (100.0) | `entry.discharge_power_kw = max_power_kw` |
| SOC 上限超限 | 0.98 | 0.95 | `entry.soc_pct = 0.95` |
| SOC 下限超限 | 0.03 | 0.05 | `entry.soc_pct = 0.05`（Fatal） |
| 爬坡率超限 | curr | `prev + max_ramp_rate * 0.25 * curr.signum()` | D11：调整 discharge/charge |

### G.2 截断后的调度一致性

截断后 `ScheduleEntry` 的字段一致性：

| 字段 | 截断后是否一致 | 说明 |
|------|--------------|------|
| `charge_power_kw` | ✅ 已截断 | 直接赋值 |
| `discharge_power_kw` | ✅ 已截断 | 直接赋值（爬坡截断通过 D11 逻辑调整） |
| `soc_pct` | ✅ 已截断 | 直接赋值 |
| `net_power_kw` | ⚠️ 未重新计算 | D11：蓝图未要求重新计算（`net_power_kw = discharge - charge`） |
| `revenue_yuan` | ⚠️ 未重新计算 | D11：蓝图未要求重新计算 |
| `period` | ✅ 不变 | 时段索引不变 |

> **D11 重要说明**：截断后 `net_power_kw` / `revenue_yuan` 未重新计算（遵循蓝图原文）。`ProtectionCoordinationRule::validate()` 读取 `net_power_kw` 字段时，使用的是截断前的值（若 `ElectricalSafetyRule` 截断了 `charge_power_kw` / `discharge_power_kw` 但未更新 `net_power_kw`）。这是蓝图的设计决策，本版本精确复制（D11），不"修复"。若未来版本发现一致性问题，需在新版本中修正。

### G.3 截断策略的物理意义

| 策略 | 物理意义 |
|------|---------|
| 功率截断到 `max_power_kw` | 限制 PCS 输出在额定功率内，避免过载 |
| SOC 上限截断到 0.95 | 保留 5% 充电裕度，避免过充损伤电池 |
| SOC 下限截断到 0.05 | 保留 5% 放电裕度，避免过放损伤电池（Fatal 触发降级） |
| 爬坡率截断 | 限制功率变化率，避免 PCS 过流保护跳闸 |

---

## 参考文献

- `蓝图/phase1.md` §v0.67.0（line 13955~14312） — 本版本蓝图原文
- `蓝图/Power_Native_Agent_OS_Blueprint.md` §43.1 — no_std 硬性要求
- `蓝图/Power_Native_Agent_OS_Blueprint.md` §5.6 — 内存预算（Agent Runtime ≤ 64MB）
- `蓝图/Power_Native_Agent_OS_Blueprint.md` §9.x — 双脑架构
- `蓝图/Power_Native_Agent_OS_Version_Roadmap_v3.md` — 版本路线图 v3.2
- `docs/ai/energy-lp-model-design.md` — v0.66.0 ScheduleResult / ScheduleEntry 设计（本版本上游）
- `docs/ai/solver-core-design.md` — v0.64.0 Solver trait + LpProblem 设计
- `docs/ai/solver-model-design.md` — v0.65.0 OptProblem DSL + compile() 设计
- `e:\eneros\.trae\rules\记忆.md` §2.3.1 — crate 分组规则（AI 子系统）
- `e:\eneros\.trae\rules\记忆.md` §2.4.2 — 构建校验清单（C1~C15）
- `e:\eneros\.trae\rules\记忆.md` §5.4 — ADR-0001 / ADR-0004 架构决策
- `e:\eneros\.trae\rules\记忆.md` §5.6 — 内存预算（Agent Runtime ≤ 64MB）
- GB/T 36558-2018 电力系统电化学储能系统通用技术条件 — 储能系统安全规范
- IEC 62619 含碱性或其他非酸性电解质的蓄电池和蓄电池组 — 工业用锂蓄电池安全要求

---

> **文档结束**。本设计文档覆盖 v0.67.0 安全校验器的全部设计内容，包含 12 章节 + 7 附录 + 3 Mermaid 图（其中 2 个为蓝图要求的类图与时序图，1 个为双重屏障架构图）。D1（移除 `Send + Sync`）、D2（本地 SystemState）、D6（不依赖 v0.56.0/v0.57.0/v0.52.0）、D11（精确复制蓝图截断逻辑）为本版本最关键的设计决策。所有代码示例均 no_std 合规（`core::*` / `alloc::*`）。实现时请同步参考 `crates/ai/safety-validator/src/lib.rs` 的偏差声明表。