//! EnerOS v0.103.0 Solver 神经部分热启动（P2-F 第 2 版：MILP 神经热启动）.
//!
//! 日前 MILP 冷启动求解耗时分钟级；本版基于 v0.64.0 `LpProblem` 矩阵格式与
//! v0.102.0 UC MILP 基座，用神经网络启发式生成初始候选解注入 HiGHS 热启动
//! （加速 ≥ 30%，D11 口径）：候选解投影合并 → 置信度阈值判定 → 注入 seam →
//! 冷启动降级，为 v0.104.0 Pareto 提供加速底座。
//!
//! # 核心类型
//!
//! - [`candidate::CandidateSolution`] — 热启动候选解（连续/整数初始值 + 置信度；
//!   可行性投影 D9：连续 clamp 列界；按 `var_types` 合并完整解向量）
//! - [`heuristic_net::WarmError`] — 热启动错误（模型加载/推理/维度）
//! - [`heuristic_net::InferEngine`] — 推理引擎 seam（D6：`MockEngine` 默认可测，
//!   `OnnxEngine` 由 `onnx-ffi` feature 门控）
//! - [`heuristic_net::MockEngine`] — Mock 推理引擎（预设输出 / 模拟失败）
//! - [`heuristic_net::HeuristicNet`] — 神经网络启发式（encode/decode 纯 Rust +
//!   `WarmStartProvider` 实现；D7 泛型后端注入）
//! - `ffi::OnnxEngine` — ONNX Runtime 推理引擎（`onnx-ffi` feature 门控，D6/D7）
//! - [`warm_start::SolveContext`] — 求解上下文（负荷预测 + 电价信号 + 历史日前计划）
//! - [`warm_start::WarmStartProvider`] — 热启动提供者 trait（D5：无 Send + Sync bound）
//! - [`warm_start::WarmStarter`] — 热启动编排器（阈值判定 + 冷启动降级链 + 计数器 D8/D10）
//!
//! # 偏差声明（D1~D12）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/solver_warm/` → `crates/ai/solver-warm/` | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 solver-core/solver-milp 同 AI 子系统 |
//! | **D2** | 蓝图 `docs/phase2/warm_start.md` → `docs/ai/warm-start-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/warm_start_bench.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.102.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 不重定义 `MilpModel`；蓝图 `model.integrality/col_lower/col_upper/num_vars` → 复用 v0.64.0 `LpProblem.var_types/lower_bounds/upper_bounds/variables.len()`（v0.102.0 D4 复用先例） | 避免平行类型体系（Karpathy Simplicity First） |
//! | **D5** | 蓝图 `WarmStartProvider: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver`/v0.59.0 `LlmEngine` 惯例一致；ONNX session 原始指针本非 Send/Sync，bound 与 FFI 设计自相矛盾 |
//! | **D6** | ONNX FFI 独立 feature `onnx-ffi`（默认关闭）；`InferEngine` seam + `MockEngine` 默认可测 | 真实 ONNX C 库编译超出单元测试范围（v0.64.0 D2/D10、v0.102.0 D5 先例）；默认构建零 unsafe 零 C 依赖 |
//! | **D7** | 蓝图 `HeuristicNet::load(path, device)` 返回具体 struct → `HeuristicNet<E: InferEngine>` 泛型 + `OnnxEngine::load`（feature-gated）/ `MockEngine::new` | 推理后端可注入（记录型 stub 验证编码路径）；蓝图 72/96 维度硬编码改为构造参数 `input_dim/output_dim` |
//! | **D8** | 蓝图"置信度过低 → 忽略"未量化 → `confidence_threshold` 构造注入（默认 0.5，配置化） | 判定阈值显式化（D10 参数配置化惯例） |
//! | **D9** | 可行性投影落地为 连续 clamp + 整数 0.5 二值化（即蓝图 decode_output 语义），不做约束级 LP 投影 | 约束级投影需求解 LP，过度工程化；界内投影已满足 HiGHS setSolution 要求 |
//! | **D10** | 加速 metric 落地为 `warm_used_count/warm_rejected_count/cold_fallback_count` 计数器 | no_std 无 log crate，metric 字段化（v0.102.0 D9 先例） |
//! | **D11** | 加速 ≥30% 为硬件集成验证项（真实 ONNX 模型 + HiGHS）；本版测试注入路径正确性（Mock 记录断言），不对加速比实测断言 | v0.102.0 D11 性能口径先例；设计文档声明口径 |
//! | **D12** | `ComputeDevice` 仅保留 `Cpu` 变体（蓝图 `Gpu(String)` 删除）；边缘推理 CPU-only（蓝图 §6.6 GPU 规则不适用 Solver） | 蓝图自相矛盾（§4.1 定义 Gpu 变体 vs §6.6 声明不适用 GPU）；避免死代码 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，默认构建零 unsafe、零 C 依赖（D6），
//! 不调用 `panic!` / `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod candidate;
pub mod heuristic_net;
pub mod warm_start;

#[cfg(feature = "onnx-ffi")]
pub mod ffi;

pub use candidate::CandidateSolution;
#[cfg(feature = "onnx-ffi")]
pub use ffi::OnnxEngine;
pub use heuristic_net::{HeuristicNet, InferEngine, MockEngine, WarmError};
pub use warm_start::{SolveContext, WarmStartProvider, WarmStarter};
