//! EnerOS v0.71.0 双脑协同联调.
//!
//! Phase 1 核心里程碑 P1-K：双脑架构首次端到端跑通。打通
//! "感知 → LLM 推理 → 意图解析 → LP 求解 → 安全校验 → 命令下发" 完整链路，
//! 实现 `DualBrainCoordinator` 统一编排与 `LatencyBreakdown` 延迟分解测量，
//! 目标端到端 < 2s。
//!
//! # 核心类型
//!
//! - [`DualBrainCoordinator`] — 端到端协调器（泛型 Solver，快/慢路径切换）
//! - [`DualBrainResult`] — 双脑结果（路径类型 + 调度 + 延迟 + 反馈契约）
//! - [`LatencyBreakdown`] — 7 环节延迟分解测量
//! - [`DualBrainError`] — 错误枚举
//! - [`DispatchCommand`] / [`CommandSink`] / [`MockCommandSink`] — 命令下发抽象
//!
//! # 偏差声明（D1~D12，Karpathy "Think Before Coding"）
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | **D1** | `Instant::now()` / `SystemTime::now()` | `now_ms: u64` 参数 | no_std 合规：`Instant`/`SystemTime` 不可用；与 v0.57/v0.64/v0.70 一致 |
//! | **D2** | `uuid::Uuid::new_v4().to_string()` | `format!("req-{}-{}", now_ms, counter)` | no_std 无 uuid crate；计数器确定性可测试 |
//! | **D3** | `LlamaCppEngine::new(...)` | `Box<dyn LlmEngine>` + 默认 `DualBrainMockEngine` | v0.59.0 `LlamaCppEngine` feature-gated；蓝图字段类型已是 `Box<dyn LlmEngine>` |
//! | **D4** | `solver: HighsSolver` | `DualBrainCoordinator<S: Solver>` 泛型 | v0.64.0 `HighsSolver` feature-gated；与 v0.70.0 一致 |
//! | **D5** | 蓝图 `SystemState` 含 `soc`/`current_power`/`current_price`/`current_period`/`device_status`/`alarms` | 输入用 `RealtimeState`（v0.70.0），内部构建 `SystemContext`（v0.69.0） | v0.67.0 `SystemState` 仅含电气字段；v0.70.0 `RealtimeState` 已包装电价/负荷 |
//! | **D6** | `ControlBusHandle` + `self.control_bus.write(command)` | 本地定义 `DispatchCommand` + `CommandSink` trait + `MockCommandSink` | `ControlBusHandle` 不存在；v0.22.0 `command_send` 是全局函数需 ring 初始化；本地抽象保持 crate 自包含可测试 |
//! | **D7** | 蓝图 `ControlCommand` 字段（`command_id`/`target_device`/`power_kw`） | `DispatchCommand` 字段（`target_device`/`power_kw`/`ttl_ms`/`timestamp`） | v0.22.0 `ControlCommand` 字段差异大（`cmd_id: [u8;16]`/`DeviceId`/`setpoint: f32`）；本地类型匹配蓝图语义 |
//! | **D8** | `solver.set_time_limit(0.5)` + `solver.solve(&problem)` | `solver.set_param("time_limit", "0.5")` + `solver.solve(&problem, now_ms)` | v0.64.0 `Solver` trait API：`set_param` 非 `set_time_limit`；`solve` 需 `now_ms` 参数 |
//! | **D9** | `prompt_template.render(&context)` / `llm_engine.infer(&prompt)` / `validator.validate(&contract)` | `prompt_template.build(&TemplateContext)` / `llm_engine.infer(&prompt, &InferParams)` / `contract_validator.validate(&contract)` 返回 `Result<(), ContractError>` | v0.63.0/v0.59.0/v0.69.0 实际 API 签名 |
//! | **D10** | `log::warn!(...)` / `log::info!(...)` | 移除日志；`DualBrainResult.latency` 携带延迟数据 | no_std 无 `log` crate；caller 自行检查 `latency.is_within_target()` |
//! | **D11** | crate 位置未明确 | `crates/ai/dual-brain/` | 项目规则 §2.3.1：AI 子系统 |
//! | **D12** | `DualBrainError` 派生 `Debug + Clone` | 仅 `Debug` | Karpathy 简化原则，与 v0.68/v0.69/v0.70 一致 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` / `core::*`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod coordinator;
pub mod error;
pub mod latency;
pub mod sink;

pub use coordinator::{DualBrainCoordinator, DualBrainResult};
pub use error::DualBrainError;
pub use latency::LatencyBreakdown;
pub use sink::{CommandSink, DispatchCommand, MockCommandSink};

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_fast_path::selector::PathType;
    use eneros_fast_path::state::RealtimeState;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_solver_core::mock::MockSolver;

    use super::*;

    // ===== T1: LatencyBreakdown::default 全 0 =====
    #[test]
    fn t1_latency_default_all_zero() {
        let lb = LatencyBreakdown::default();
        assert_eq!(lb.perception_ms, 0);
        assert_eq!(lb.llm_inference_ms, 0);
        assert_eq!(lb.intent_parse_ms, 0);
        assert_eq!(lb.lp_build_ms, 0);
        assert_eq!(lb.lp_solve_ms, 0);
        assert_eq!(lb.safety_validate_ms, 0);
        assert_eq!(lb.command_dispatch_ms, 0);
        assert_eq!(lb.total_ms, 0);
    }

    // ===== T2: LatencyBreakdown::calculate_total 累加正确 =====
    #[test]
    fn t2_calculate_total() {
        let mut lb = LatencyBreakdown {
            perception_ms: 10,
            llm_inference_ms: 1200,
            intent_parse_ms: 5,
            lp_build_ms: 20,
            lp_solve_ms: 100,
            safety_validate_ms: 15,
            command_dispatch_ms: 5,
            ..Default::default()
        };
        lb.calculate_total();
        assert_eq!(lb.total_ms, 10 + 1200 + 5 + 20 + 100 + 15 + 5);
        assert_eq!(lb.total_ms, 1355);
    }

    // ===== T3: LatencyBreakdown::is_within_target 达标 =====
    #[test]
    fn t3_is_within_target_true() {
        let mut lb = LatencyBreakdown {
            llm_inference_ms: 1200,
            lp_solve_ms: 100,
            ..Default::default()
        };
        lb.calculate_total();
        assert!(lb.is_within_target());
    }

    // ===== T4: LatencyBreakdown::is_within_target 超标 =====
    #[test]
    fn t4_is_within_target_false() {
        let mut lb = LatencyBreakdown {
            llm_inference_ms: 1500,
            lp_solve_ms: 600,
            ..Default::default()
        };
        lb.calculate_total();
        assert!(!lb.is_within_target());
    }

    // ===== T5: LatencyBreakdown::bottleneck 返回最长环节 =====
    #[test]
    fn t5_bottleneck() {
        let lb = LatencyBreakdown {
            llm_inference_ms: 1200,
            lp_solve_ms: 100,
            ..Default::default()
        };
        assert_eq!(lb.bottleneck(), "llm_inference");
    }

    // ===== T6: LatencyBreakdown::to_table 包含环节名 =====
    #[test]
    fn t6_to_table() {
        let mut lb = LatencyBreakdown {
            llm_inference_ms: 1200,
            ..Default::default()
        };
        lb.calculate_total();
        let table = lb.to_table();
        assert!(table.contains("perception"));
        assert!(table.contains("llm_inference"));
        assert!(table.contains("lp_solve"));
        assert!(table.contains("total"));
    }

    // ===== T7: DispatchCommand 构造 =====
    #[test]
    fn t7_dispatch_command_construction() {
        let cmd = DispatchCommand {
            target_device: String::from("pcs"),
            power_kw: 50.0,
            ttl_ms: 300_000,
            timestamp: 1000,
        };
        assert_eq!(cmd.target_device, "pcs");
        assert!((cmd.power_kw - 50.0).abs() < 1e-9);
        assert_eq!(cmd.ttl_ms, 300_000);
        assert_eq!(cmd.timestamp, 1000);
    }

    // ===== T8: MockCommandSink::new 空 =====
    #[test]
    fn t8_mock_command_sink_new() {
        let sink = MockCommandSink::new();
        assert_eq!(sink.commands().len(), 0);
    }

    // ===== T9: MockCommandSink::write 收集命令 =====
    #[test]
    fn t9_mock_command_sink_write() {
        let mut sink = MockCommandSink::new();
        let cmd = DispatchCommand {
            target_device: String::from("pcs"),
            power_kw: 50.0,
            ttl_ms: 300_000,
            timestamp: 1000,
        };
        sink.write(cmd).unwrap();
        assert_eq!(sink.commands().len(), 1);
        assert_eq!(sink.commands()[0].target_device, "pcs");
    }

    // ===== T10: DualBrainError 变体构造 =====
    #[test]
    fn t10_dual_brain_error_variants() {
        let _ = DualBrainError::LlmError(String::from("llm"));
        let _ = DualBrainError::ParseError(String::from("parse"));
        let _ = DualBrainError::ContractError(String::from("contract"));
        let _ = DualBrainError::SolveError(String::from("solve"));
        let _ = DualBrainError::DispatchError(String::from("dispatch"));
    }

    // ===== T11: DualBrainCoordinator::new 构造 =====
    #[test]
    fn t11_coordinator_new() {
        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(coordinator::DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let sink: Box<dyn CommandSink> = Box::new(MockCommandSink::new());
        let _coord = DualBrainCoordinator::new(config, llm_engine, solver, sink);
    }

    // ===== T12: DualBrainCoordinator::default_with_mock 构造 =====
    #[test]
    fn t12_default_with_mock() {
        let _coord = DualBrainCoordinator::default_with_mock();
    }

    // ===== T13: execute 快路径 =====
    #[test]
    fn t13_execute_fast_path() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        // 第一次：SlowPath（初始化基线）
        let _ = coord.execute(&state, 0).unwrap();
        // 第二次：状态未变，间隔内 → FastPath
        let result = coord.execute(&state, 1000).unwrap();
        assert_eq!(result.path_type, PathType::FastPath);
    }

    // ===== T14: execute 慢路径端到端 =====
    #[test]
    fn t14_execute_slow_path_end_to_end() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0);
        assert!(result.is_ok());
    }

    // ===== T15: execute 慢路径返回 SlowPath =====
    #[test]
    fn t15_execute_slow_path_returns_slow_path() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        assert_eq!(result.path_type, PathType::SlowPath);
    }

    // ===== T16: execute 慢路径 latency 各字段（llm_inference_ms > 0）=====
    #[test]
    fn t16_execute_slow_path_latency() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        assert!(result.latency.llm_inference_ms > 0);
    }

    // ===== T17: execute 慢路径 feedback 为 Some =====
    #[test]
    fn t17_execute_slow_path_feedback_some() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        assert!(result.feedback.is_some());
    }

    // ===== T18: execute 慢路径 schedule 非空 =====
    #[test]
    fn t18_execute_slow_path_schedule_non_empty() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        assert!(!result.schedule.schedule.is_empty());
    }

    // ===== T19: execute 命令下发到 sink =====
    #[test]
    fn t19_command_dispatched_to_sink() {
        use std::cell::RefCell;
        use std::rc::Rc;

        struct RecordingSink {
            commands: Rc<RefCell<Vec<DispatchCommand>>>,
        }

        impl CommandSink for RecordingSink {
            fn write(&mut self, cmd: DispatchCommand) -> Result<(), DualBrainError> {
                self.commands.borrow_mut().push(cmd);
                Ok(())
            }
        }

        let commands = Rc::new(RefCell::new(Vec::new()));
        let sink: Box<dyn CommandSink> = Box::new(RecordingSink {
            commands: commands.clone(),
        });

        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(coordinator::DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver, sink);

        let state = RealtimeState::default();
        let _result = coord.execute(&state, 0).unwrap();
        assert_eq!(commands.borrow().len(), 1);
    }

    // ===== T20: execute request_id 格式 req-{ms}-{counter} =====
    #[test]
    fn t20_request_id_format() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        let feedback = result.feedback.as_ref().unwrap();
        assert_eq!(feedback.request_id, "req-0-1");
    }

    // ===== T21: 端到端 RealtimeState → execute → path_type =====
    #[test]
    fn t21_end_to_end_state_to_path_type() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = RealtimeState::default();
        let result = coord.execute(&state, 0).unwrap();
        assert_eq!(result.path_type, PathType::SlowPath);
    }

    // ===== T22: LatencyBreakdown::bottleneck 全 0 返回 "none" =====
    #[test]
    fn t22_bottleneck_all_zero() {
        let lb = LatencyBreakdown::default();
        assert_eq!(lb.bottleneck(), "none");
    }
}
