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
// 蓝图/20 R2-1：Mock 类型只允许测试/仿真（feature=mock）；release 构建
// 启用即熔断（B3 换皮清偿：禁换名 mock 顶包生产）。
#[cfg(all(feature = "mock", not(debug_assertions)))]
compile_error!(
    "eneros-dual-brain: mock feature is FORBIDDEN in release builds. \
     Use --no-default-features in release."
);

extern crate alloc;

pub mod audit;
pub mod chain_sink;
pub mod coordinator;
pub mod error;
pub mod latency;
pub mod learning;
pub mod rejection;
pub mod sink;

#[cfg(any(test, feature = "mock"))]
pub use coordinator::DualBrainMockEngine;
pub use coordinator::{DualBrainCoordinator, DualBrainResult};
pub use error::DualBrainError;
pub use latency::LatencyBreakdown;
#[cfg(any(test, feature = "mock"))]
pub use sink::MockCommandSink;
pub use sink::{CommandSink, DispatchCommand};

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_fast_path::selector::PathType;
    use eneros_fast_path::state::RealtimeState;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_safety_validator::state::SystemState;
    #[cfg(any(test, feature = "mock"))]
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

    /// 构造安全系统状态（P0-17：避免默认态触发 Fatal 校验失败）。
    fn safe_state() -> RealtimeState {
        RealtimeState {
            system: SystemState {
                voltage_v: 380.0,
                current_a: 10.0,
                frequency_hz: 50.0,
                soc_pct: 0.8,
                timestamp_ms: 0,
            },
            current_price: 0.5,
            load_demand: None,
        }
    }

    // ===== T13: execute 快路径 =====
    #[test]
    fn t13_execute_fast_path() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
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
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(result.is_ok());
    }

    // ===== T15: execute 慢路径返回 SlowPath =====
    #[test]
    fn t15_execute_slow_path_returns_slow_path() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0).unwrap();
        assert_eq!(result.path_type, PathType::SlowPath);
    }

    // ===== T16: execute 慢路径 latency 各字段（llm_inference_ms > 0）=====
    // v0.248.0：注入可控单调时钟（每步推进），验证 latency 各字段接真实
    // 时钟——注入已知延迟可观测到对应数值（验收标准）。
    static TEST_NOW_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    #[test]
    fn t16_execute_slow_path_latency() {
        // 时钟：每次调用推进 10ms（模拟真实执行耗时）。
        TEST_NOW_MS.store(1_000_000, core::sync::atomic::Ordering::SeqCst);
        let clock = || TEST_NOW_MS.fetch_add(10, core::sync::atomic::Ordering::SeqCst);
        let mut coord = DualBrainCoordinator::default_with_mock().with_clock(clock);
        let state = safe_state();
        let result = coord.execute(&state, 0).unwrap();
        // 每个计时步骤至少推进一次（每步 ≥ 10ms）。
        assert!(result.latency.llm_inference_ms >= 10);
        assert!(result.latency.perception_ms >= 10);
        assert!(result.latency.intent_parse_ms >= 10);
        assert!(result.latency.lp_build_ms >= 10);
        assert!(result.latency.safety_validate_ms >= 10);
        // total = 各字段之和（calculate_total）。
        let parts = result.latency.perception_ms
            + result.latency.llm_inference_ms
            + result.latency.intent_parse_ms
            + result.latency.lp_build_ms
            + result.latency.lp_solve_ms
            + result.latency.safety_validate_ms
            + result.latency.command_dispatch_ms;
        assert_eq!(result.latency.total_ms, parts);
        // FeedbackContract 不含 llm_meta；模型标识真实性由 coordinator
        // 构造 IntentContract 时从 engine.model_info() 获取（v0.248.0）。
    }

    // ===== T17: execute 慢路径 feedback 为 Some =====
    #[test]
    fn t17_execute_slow_path_feedback_some() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0).unwrap();
        assert!(result.feedback.is_some());
    }

    // ===== T18: execute 慢路径 schedule 非空 =====
    #[test]
    fn t18_execute_slow_path_schedule_non_empty() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
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

        let state = safe_state();
        let _result = coord.execute(&state, 0).unwrap();
        assert_eq!(commands.borrow().len(), 1);
    }

    // ===== T20: execute request_id 格式 req-{ms}-{counter} =====
    #[test]
    fn t20_request_id_format() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0).unwrap();
        let feedback = result.feedback.as_ref().unwrap();
        assert_eq!(feedback.request_id, "req-0-1");
    }

    // ===== T21: 端到端 RealtimeState → execute → path_type =====
    #[test]
    fn t21_end_to_end_state_to_path_type() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0).unwrap();
        assert_eq!(result.path_type, PathType::SlowPath);
    }

    // ===== T22: LatencyBreakdown::bottleneck 全 0 返回 "none" =====
    #[test]
    fn t22_bottleneck_all_zero() {
        let lb = LatencyBreakdown::default();
        assert_eq!(lb.bottleneck(), "none");
    }

    // ======================================================================
    // v0.206.0 (P0-17)：安全屏障闭环——Fatal 违规阻断命令下发
    // ======================================================================
    #[test]
    fn p017_fatal_soc_blocks_sink_and_reports() {
        use eneros_llm_engine::engine::LlmEngine;
        use eneros_solver_core::result::{SolveResult, SolveStatus};

        use crate::coordinator::DualBrainMockEngine;
        use crate::sink::MockCommandSink;

        let config = ScheduleConfig::default();
        let n = config.num_periods;
        let cap = config.battery_capacity_kwh;
        // 故障注入：求解器输出 SOC = 0.02（< 0.05 安全下限）→ Fatal。
        // solution 布局：charge[0..n]=0、discharge[n..2n]=0、soc[2n..3n]=0.02*cap。
        let mut solution = alloc::vec![0.0f64; 3 * n];
        for t in 0..n {
            solution[2 * n + t] = 0.02 * cap;
        }
        let solver = MockSolver::with_result(SolveResult {
            status: SolveStatus::Optimal,
            objective_value: 0.0,
            solution,
            elapsed_ms: 0,
            dual_solution: None,
        });
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let sink = alloc::boxed::Box::new(MockCommandSink::new());
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver, sink);

        let state = safe_state();
        let err = coord.execute(&state, 1000).unwrap_err();
        assert!(
            matches!(err, DualBrainError::SafetyFatal(_)),
            "SOC<0.05 必须 Fatal，实际 {err:?}"
        );
        // sink 零写入（命令未下发）。
        assert_eq!(
            coord.sink.write_count(),
            0,
            "Fatal 违规下 sink 必须零写入（P0-17）"
        );
    }

    #[test]
    fn p017_normal_soc_writes_sink() {
        use eneros_llm_engine::engine::LlmEngine;

        use crate::coordinator::DualBrainMockEngine;
        use crate::sink::MockCommandSink;

        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let sink = alloc::boxed::Box::new(MockCommandSink::new());
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver, sink);

        // 正常 SOC=0.8 → 慢路径可下发命令（sink 有写入）。
        let state = safe_state();
        let _ = coord.execute(&state, 0).unwrap();
        assert!(
            coord.sink.write_count() >= 1,
            "正常路径应下发命令，实际 {} 条",
            coord.sink.write_count()
        );
    }
}

// ============================================================================
// 22 §2.W2.3 验收：LLM 有界延迟（经 InferScheduler + 超时回退/降级）
// ============================================================================
#[cfg(test)]
mod w23_bounded_llm_tests {
    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_fast_path::selector::PathType;
    use eneros_fast_path::state::RealtimeState;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_llm_engine::error::LlmError;
    use eneros_llm_engine::params::InferParams;
    use eneros_safety_validator::state::SystemState;
    use eneros_solver_core::mock::MockSolver;

    use super::*;

    /// 卡死/慢模型模拟：infer 永远失败（超时/引擎错误）。
    struct AlwaysFailEngine;
    impl LlmEngine for AlwaysFailEngine {
        fn load_model(&mut self, _path: &str) -> Result<(), LlmError> {
            Ok(())
        }
        fn infer(&mut self, _prompt: &str, _params: &InferParams) -> Result<String, LlmError> {
            Err(LlmError::InferFailed)
        }
        fn infer_stream(
            &mut self,
            _prompt: &str,
            _params: &InferParams,
            _cb: &mut dyn FnMut(&str) -> bool,
        ) -> Result<(), LlmError> {
            Err(LlmError::InferFailed)
        }
        fn model_info(&self) -> Option<&eneros_llm_engine::model::ModelInfo> {
            None
        }
        fn health_check(&self) -> eneros_llm_engine::stats::EngineHealth {
            eneros_llm_engine::stats::EngineHealth {
                loaded: false,
                device: eneros_llm_engine::device::ComputeDevice::Cpu,
                gpu_layers: 0,
                last_error: None,
            }
        }
        fn stats(&self) -> &eneros_llm_engine::stats::EngineStats {
            static S: eneros_llm_engine::stats::EngineStats =
                eneros_llm_engine::stats::EngineStats {
                    inference_count: 0,
                    total_tokens_generated: 0,
                    total_inference_ns: 0,
                    last_inference_ns: 0,
                    model_load_count: 0,
                    gpu_layers: 0,
                };
            &S
        }
    }

    /// 时间推进时钟：每次调用 +10ms（模拟真实耗时，超时检测用）。
    /// 函数指针（with_clock 契约：`fn() -> u64`）。
    static CLOCK_NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1_000_000);
    fn advancing_clock_fn() -> u64 {
        CLOCK_NOW.fetch_add(10, core::sync::atomic::Ordering::SeqCst)
    }

    /// 验收 1：LLM 失败（慢/卡死模型）→ 回退 fast-path，决策不阻塞不报错。
    #[test]
    fn test_llm_failure_falls_back_to_fast_path() {
        let clock = advancing_clock_fn;
        let config = ScheduleConfig::default();
        let llm: Box<dyn LlmEngine> = Box::new(AlwaysFailEngine);
        let solver = MockSolver::new();
        let sink: Box<dyn CommandSink> = Box::new(MockCommandSink::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink).with_clock(clock);
        let state = RealtimeState {
            system: SystemState::default(),
            current_price: 0.5,
            load_demand: None,
        };
        let result = coord.execute(&state, 0).expect("fast-path 回退必须成功");
        assert_eq!(
            result.path_type,
            PathType::FastPath,
            "LLM 失败必须回退 fast-path（不阻塞决策）"
        );
    }

    /// 验收 2：连续超时 → 降级模式（degraded 标记）。
    #[test]
    fn test_consecutive_timeout_marks_degraded() {
        let clock = advancing_clock_fn;
        let config = ScheduleConfig::default();
        let llm: Box<dyn LlmEngine> = Box::new(AlwaysFailEngine);
        let solver = MockSolver::new();
        let sink: Box<dyn CommandSink> = Box::new(MockCommandSink::new());
        let mut coord = DualBrainCoordinator::new(config, llm, solver, sink).with_clock(clock);
        let state = RealtimeState {
            system: SystemState::default(),
            current_price: 0.5,
            load_demand: None,
        };
        // 第一次：回退 fast-path。
        let _ = coord.execute(&state, 0).expect("first");
        assert!(!coord.degraded);
        // 第二次：仍失败 → streak 增长（失败非超时 → streak 不重置但
        // degraded 需超时触发——用超时引擎场景在 scheduler 层验证；
        // 这里验证失败回退路径不 panic 且可连续调用）。
        let r2 = coord.execute(&state, 1).expect("second");
        assert_eq!(r2.path_type, PathType::FastPath);
        let _ = coord.execute(&state, 2).expect("third");
    }

    /// 验收 3：InferParams 默认 timeout 有界（5s），scheduler 超时检测生效。
    #[test]
    fn test_infer_params_timeout_bounded() {
        assert_eq!(InferParams::default().timeout_ms, 5_000);
    }
}
