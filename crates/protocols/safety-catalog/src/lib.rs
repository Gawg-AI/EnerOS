//! EnerOS 安全工程目录 (CTL-001)。
//!
//! MASTER G5 §9.4 要求：建立资产、能量流、危险源、事故、控制约束清单；对每个设备
//! 类型执行 FMEA/FMECA 与 STPA 或等价分析；定义 safe state；将安全需求追踪到代码、
//! 测试、现场验收和操作手册。
//!
//! 本 crate 提供安全工程的**数据模型**与**能源行业目录**：
//! - [`hazard::Hazard`] — 资产 / 能量流 / 危险源 / 事故 / 严重度
//! - [`fmea::FmeaEntry`] — 失效模式与影响分析（severity × occurrence × detection → RPN）
//! - [`stpa::StpaEntry`] — 系统理论事故模型（不安全控制动作 UCA + 安全约束）
//! - [`safe_state::SafeState`] — 设备类别的安全方向（掉网/掉电/传感器坏/Agent 失控…）
//! - [`trace::SafetyRequirement`] — 安全需求 → 代码 / 测试 / 验收 追溯
//! - [`catalog`] — EnerOS 能源域（电池 / 储能变流器 / 并网点 / 继电器 / 母线）冻结目录
//!
//! # 设计原则
//!
//! - 目录是 `const` 数据，编译期固化，运行期不可变；不依赖文件系统或网络。
//! - 每条目有稳定 `id`，供代码、测试、审计和操作手册引用。
//! - safe state 是 fail-safe 方向：任何不确定性都导向已定义的安全侧，而非继续动作。
//! - 追溯表把 hazard/FMEA/STPA → 需求 → 实现/测试 串联成可审计链。
//!
//! # no_std
//!
//! `#![cfg_attr(not(test), no_std)]`。目录为静态切片，无 `alloc` 依赖。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

pub mod catalog;
pub mod fmea;
pub mod hazard;
pub mod safe_state;
pub mod stpa;
pub mod trace;

pub use fmea::{Detection, FmeaEntry, Occurrence, Severity};
pub use hazard::{EnergyFlow, Hazard, HazardCategory};
pub use safe_state::SafeState;
pub use stpa::{ControlAction, StpaEntry, UcaDirection, UnsafeControlAction};
pub use trace::{RequirementSource, SafetyRequirement, TraceTarget};
