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
// 49（WP-49.4）：coordinator 为 host 消费面模块（feature 门控）——
// target 闭包只装 decision/gate/audit 子集，五重依赖不入闭包。
#[cfg(feature = "coordinator")]
pub mod coordinator;
pub mod decision;
pub mod error;
pub mod gate;
pub mod latency;
pub mod learning;
pub mod rejection;
pub mod sink;
pub mod tick_policy;

#[cfg(any(test, feature = "mock"))]
pub use coordinator::DualBrainMockEngine;
#[cfg(feature = "coordinator")]
pub use coordinator::{DualBrainCoordinator, DualBrainResult};
pub use decision::{
    DecisionOutcome, DecisionPath, DecisionRejection, NoActionReason, ProposedControlPlan,
    RejectionReason,
};
pub use error::DualBrainError;
pub use latency::LatencyBreakdown;
#[cfg(any(test, feature = "mock"))]
pub use sink::MockCommandSink;
pub use sink::{CommandSink, DispatchCommand};
pub use tick_policy::{TickOutcome, TickPolicy, TickVerdict};

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_fast_path::state::RealtimeState;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_safety_validator::state::SystemState;
    #[cfg(any(test, feature = "mock"))]
    use eneros_solver_core::mock::MockSolver;
    use eneros_solver_core::result::{SolveResult, SolveStatus};

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
            ttl_secs: 300,
            timestamp: 1000,
        };
        assert_eq!(cmd.target_device, "pcs");
        assert!((cmd.power_kw - 50.0).abs() < 1e-9);
        assert_eq!(cmd.ttl_secs, 300);
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
            ttl_secs: 300,
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
        // 蓝图 40 WP-40.3：协调器不持 sink（决策只产 Proposal）。
        let _coord = DualBrainCoordinator::new(config, llm_engine, solver);
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
        assert!(matches!(
            coord.execute(&state, 0),
            DecisionOutcome::Proposal(_)
        ));
        // 第二次：状态未变，间隔内 → FastPath
        let result = coord.execute(&state, 1000);
        assert!(
            matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Fast),
            "间隔内必须走快路径（Proposal, Fast）：{result:?}"
        );
    }

    // ===== T14: execute 慢路径端到端 =====
    #[test]
    fn t14_execute_slow_path_end_to_end() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(
            matches!(result, DecisionOutcome::Proposal(_)),
            "慢路径端到端必须产出 Proposal（WP-40.3）"
        );
    }

    // ===== T15: execute 慢路径返回 SlowPath =====
    #[test]
    fn t15_execute_slow_path_returns_slow_path() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Slow));
    }

    // ===== 32 AG-04 判据：audit_wired — 慢路径决策入审计链 =====
    /// 验收：with_audit 注入后，慢路径决策产生审计记录（含
    /// path/validation/gate 字段），全链 verify 绿；未注入时零记录。
    #[test]
    fn audit_wired_slow_path_records_decision() {
        let mut coord = DualBrainCoordinator::default_with_mock()
            .with_audit(crate::audit::DecisionAudit::new([0x32; 32]));
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Slow));
        assert_eq!(
            coord
                .audit
                .as_ref()
                .expect("audit 已注入")
                .chain_state()
                .entry_count,
            1,
            "慢路径决策必须入链"
        );
        assert_eq!(coord.audit_drop_count, 0, "写入不得失败");
        assert!(
            coord
                .audit
                .as_ref()
                .expect("audit")
                .verify()
                .expect("verify")
                .valid,
            "审计链自洽"
        );
        // 蓝图 40 WP-40.3：所有路径（含快路径）都入审计链。
        let _ = coord.execute(&state, 1000);
        assert_eq!(
            coord
                .audit
                .as_ref()
                .expect("audit")
                .chain_state()
                .entry_count,
            2,
            "快路径也必须入审计链（WP-40.3 全路径审计）"
        );

        // 31-review（P1 AG-04）：拒绝/失败也必须入审计链（旧实现仅成功路径
        // 记录——篡改链漏掉恰好最关键的决策）。蓝图 40 WP-40.3：gate 预算
        // 在 dispatch 层（control-dispatch 测试覆盖 gate 拒绝审计）；
        // 协调器侧以 SafetyFatal 拒绝验证"拒绝也入链"。
        let _coord2 = DualBrainCoordinator::default_with_mock()
            .with_audit(crate::audit::DecisionAudit::new([0x33; 32]));
        // 故障注入：求解器输出 SOC=0.02（< 0.05 Fatal 阈值）→ SafetyFatal。
        let config = ScheduleConfig::default();
        let n = config.num_periods;
        let cap = config.battery_capacity_kwh;
        let mut solution = alloc::vec![0.0f64; 4 * n + 1];
        for t in 0..n {
            solution[3 * n + t] = 0.02 * cap;
        }
        let solver = MockSolver::with_result(SolveResult {
            status: SolveStatus::Optimal,
            objective_value: 0.0,
            solution,
            elapsed_ms: 0,
            dual_solution: None,
        });
        let mut coord3 =
            DualBrainCoordinator::new(config, Box::new(DualBrainMockEngine::new()), solver)
                .with_audit(crate::audit::DecisionAudit::new([0x33; 32]));
        let rejected = coord3.execute(&state, 0);
        assert!(
            matches!(&rejected, DecisionOutcome::Rejected(r) if matches!(r.reason, RejectionReason::SafetyFatal(_))),
            "Fatal 必须拒（Rejected）"
        );
        assert_eq!(
            coord3
                .audit
                .as_ref()
                .expect("audit")
                .chain_state()
                .entry_count,
            1,
            "拒绝也必须入审计链（AG-04 全结果）"
        );
    }

    /// 验收：未注入审计时慢路径正常决策且不 panic（可选装配）。
    #[test]
    fn audit_absent_slow_path_still_works() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Slow));
        assert!(coord.audit.is_none());
    }

    // ===== T16: execute 慢路径 latency 各字段（llm_inference_ms > 0）=====
    // v0.248.0：注入可控单调时钟（每步推进），验证 latency 各字段接真实
    // 时钟——注入已知延迟可观测到对应数值（验收标准）。
    static TEST_NOW_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn t16_execute_slow_path_latency() {
        // 时钟：每次调用推进 10ms（模拟真实执行耗时）。
        TEST_NOW_MS.store(1_000_000, core::sync::atomic::Ordering::SeqCst);
        let clock = || TEST_NOW_MS.fetch_add(10, core::sync::atomic::Ordering::SeqCst);
        let mut coord = DualBrainCoordinator::default_with_mock().with_clock(clock);
        let state = safe_state();
        let result = coord.execute(&state, 0);
        let DecisionOutcome::Proposal(plan) = result else {
            panic!("慢路径必须产出 Proposal");
        };
        // 每个计时步骤至少推进一次（每步 ≥ 10ms）。
        assert!(plan.latency.llm_inference_ms >= 10);
        assert!(plan.latency.perception_ms >= 10);
        assert!(plan.latency.intent_parse_ms >= 10);
        assert!(plan.latency.lp_build_ms >= 10);
        assert!(plan.latency.safety_validate_ms >= 10);
        // total = 各字段之和（calculate_total）。
        let parts = plan.latency.perception_ms
            + plan.latency.llm_inference_ms
            + plan.latency.intent_parse_ms
            + plan.latency.lp_build_ms
            + plan.latency.lp_solve_ms
            + plan.latency.safety_validate_ms
            + plan.latency.command_dispatch_ms;
        assert_eq!(plan.latency.total_ms, parts);
        // FeedbackContract 不含 llm_meta；模型标识真实性由 coordinator
        // 构造 IntentContract 时从 engine.model_info() 获取（v0.248.0）。
    }

    // ===== T17: execute 慢路径 feedback 为 Some =====
    #[test]
    fn t17_execute_slow_path_feedback_some() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(
            matches!(&result, DecisionOutcome::Proposal(p) if p.feedback.is_some()),
            "慢路径 Proposal 必须带预测反馈"
        );
    }

    // ===== T18b/c/d（蓝图 40 WP-40.3）：决策闸移入唯一 ControlDispatch =====
    // 速率/能量预算的 check+record 现在只发生在 dispatch 层（confirmed
    // dispatch 记账；拒绝/失败不消耗成功预算）——协调器只产 Proposal，
    // 不再持有/消费决策闸。对应 gate 行为测试移到 control-dispatch crate
    //（`DispatchGate` 单测 + dispatch 集成测试）。此处保留回归：Proposal
    // 可无限产出（协调器无副作用），预算语义由 dispatch 保证。
    #[test]
    fn t18_coordinator_proposals_have_no_gate_side_effects() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        for now in [0u64, 1000, 2000, 3000] {
            let r = coord.execute(&state, now);
            assert!(
                matches!(r, DecisionOutcome::Proposal(_)),
                "协调器只产 Proposal（无预算副作用）：{r:?}"
            );
        }
    }

    // ===== T18: execute 慢路径 schedule 非空 =====
    #[test]
    fn t18_execute_slow_path_schedule_non_empty() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(matches!(&result, DecisionOutcome::Proposal(p) if !p.schedule.schedule.is_empty()));
    }

    // ===== T19: execute 只产 Proposal，不写任何设备（WP-40.3） =====
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn t19_execute_produces_proposal_without_write() {
        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(coordinator::DualBrainMockEngine::new());
        let solver = MockSolver::new();
        // 协调器不再持有 sink——编译期保证"决策路径不写设备"。
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver);

        let state = safe_state();
        let result = coord.execute(&state, 0);
        let DecisionOutcome::Proposal(plan) = result else {
            panic!("慢路径必须产出 Proposal");
        };
        // Proposal 携带执行目标（提交给 ControlDispatch 后才可能产生设备写）。
        assert_eq!(plan.target_device, "pcs");
        assert_eq!(plan.ttl_secs, 300);
        assert!(plan.power_kw.is_finite());
        // mark_committed 登记后降级基线可用。
        coord.mark_committed(&plan);
        assert!(coord.last_committed_schedule.is_some());
    }

    // ===== T20: execute request_id 格式 req-{ms}-{counter} =====
    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn t20_request_id_format() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        let DecisionOutcome::Proposal(plan) = result else {
            panic!("慢路径必须产出 Proposal");
        };
        let feedback = plan.feedback.as_ref().unwrap();
        assert_eq!(feedback.request_id, "req-0-1");
    }

    // ===== T21: 端到端 RealtimeState → execute → path_type =====
    #[test]
    fn t21_end_to_end_state_to_path_type() {
        let mut coord = DualBrainCoordinator::default_with_mock();
        let state = safe_state();
        let result = coord.execute(&state, 0);
        assert!(matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Slow));
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

        let config = ScheduleConfig::default();
        let n = config.num_periods;
        let cap = config.battery_capacity_kwh;
        // 故障注入：求解器输出 SOC = 0.02（< 0.05 安全下限）→ Fatal。
        // solution 布局（31-review AI-05 增二元变量后 4n+1）：
        // charge[0..n]=0、discharge[n..2n]=0、y[2n..3n]=0、soc[3n..4n+1]=0.02*cap。
        let mut solution = alloc::vec![0.0f64; 4 * n + 1];
        for t in 0..n {
            solution[3 * n + t] = 0.02 * cap;
        }
        let solver = MockSolver::with_result(SolveResult {
            status: SolveStatus::Optimal,
            objective_value: 0.0,
            solution,
            elapsed_ms: 0,
            dual_solution: None,
        });
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver);

        let state = safe_state();
        let err = coord.execute(&state, 1000);
        assert!(
            matches!(
                &err,
                DecisionOutcome::Rejected(r) if matches!(r.reason, RejectionReason::SafetyFatal(_))
            ),
            "SOC<0.05 必须 Fatal（Rejected），实际 {err:?}"
        );
        // 零 Proposal = 零设备写（协调器不持 sink，P0-17 语义保留）。
        assert!(
            !matches!(err, DecisionOutcome::Proposal(_)),
            "Fatal 违规不得产出 Proposal"
        );
    }

    #[test]
    fn p017_normal_soc_writes_sink() {
        use eneros_llm_engine::engine::LlmEngine;

        use crate::coordinator::DualBrainMockEngine;

        let config = ScheduleConfig::default();
        let llm_engine: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let mut coord = DualBrainCoordinator::new(config, llm_engine, solver);

        // 正常 SOC=0.8 → 慢路径产出 Proposal（dispatch 阶段才写设备）。
        let state = safe_state();
        let r = coord.execute(&state, 0);
        assert!(
            matches!(&r, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Slow),
            "正常路径必须产出 Proposal"
        );
    }
}

// ============================================================================
// 22 §2.W2.3 验收：LLM 有界延迟（经 InferScheduler + 超时回退/降级）
// ============================================================================
#[cfg(test)]
mod w23_bounded_llm_tests {
    use eneros_energy_lp_model::config::ScheduleConfig;
    use eneros_fast_path::state::RealtimeState;
    use eneros_llm_engine::engine::LlmEngine;
    use eneros_llm_engine::error::LlmError;
    use eneros_llm_engine::params::InferParams;
    use eneros_safety_validator::state::SystemState;
    use eneros_safety_validator::TestSystemStateBuilder;
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
        let mut coord = DualBrainCoordinator::new(config, llm, solver).with_clock(clock);
        let state = RealtimeState {
            system: TestSystemStateBuilder::new(380.0, 0.0, 50.0, 0.5, 0).build(),
            current_price: 0.5,
            load_demand: None,
        };
        let result = coord.execute(&state, 0);
        assert!(
            matches!(&result, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Fallback),
            "LLM 失败必须回退 fast-path（Proposal, Fallback）：{result:?}"
        );
    }

    /// 验收 2：连续超时 → 降级模式（degraded 标记 + 复用上次成功决策）。
    ///
    /// 31-review（P2）：旧版用 AlwaysFailEngine（InferFailed 非 Timeout，
    /// streak 永不涨）且从不断言 degraded——假测试。本版：① 阶段 1 成功慢路径
    /// 记录 last_successful；② 时钟切跳变模式（每读 +6s > 5s 超时窗）使 LLM
    /// 每次请求真超时；③ 慢路径间隔（300s）用 now_ms 拉开——两次慢路径超时
    /// → streak=2 → PathType::Degraded 且复用上次成功调度。
    static TIMEOUT_MODE: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    fn timeout_jump_clock_fn() -> u64 {
        if TIMEOUT_MODE.load(core::sync::atomic::Ordering::SeqCst) {
            CLOCK_NOW.fetch_add(6_000, core::sync::atomic::Ordering::SeqCst)
        } else {
            CLOCK_NOW.fetch_add(10, core::sync::atomic::Ordering::SeqCst)
        }
    }

    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn test_consecutive_timeout_marks_degraded() {
        use crate::coordinator::DualBrainMockEngine;
        CLOCK_NOW.store(1_000_000, core::sync::atomic::Ordering::SeqCst);
        TIMEOUT_MODE.store(false, core::sync::atomic::Ordering::SeqCst);
        let config = ScheduleConfig::default();
        let llm: Box<dyn LlmEngine> = Box::new(DualBrainMockEngine::new());
        let solver = MockSolver::new();
        let mut coord =
            DualBrainCoordinator::new(config, llm, solver).with_clock(timeout_jump_clock_fn);
        // 安全系统状态（默认态 SOC=0 会触发 Fatal 校验失败——同另一测试模块
        // safe_state 的取值，此处内联）。
        let state = RealtimeState {
            system: SystemState {
                voltage_v: 380.0,
                current_a: 10.0,
                frequency_hz: 50.0,
                soc_pct: 0.8,
                timestamp_ms: 0,
            },
            current_price: 0.5,
            load_demand: None,
        };
        // 阶段 1：成功慢路径 → Proposal，并经 mark_committed 登记基线
        //（蓝图 40 WP-40.3：降级只回放已提交计划）。
        let r1 = coord.execute(&state, 0);
        let DecisionOutcome::Proposal(p1) = &r1 else {
            panic!("slow path 必须产出 Proposal: {r1:?}");
        };
        assert_eq!(p1.path, DecisionPath::Slow);
        coord.mark_committed(p1);
        assert!(
            coord.last_committed_schedule.is_some(),
            "mark_committed 必须登记降级基线"
        );
        // 阶段 2：时钟跳变 → LLM 真超时。慢路径间隔 300s 用 now_ms 拉开。
        TIMEOUT_MODE.store(true, core::sync::atomic::Ordering::SeqCst);
        let r2 = coord.execute(&state, 310_000);
        assert!(
            matches!(&r2, DecisionOutcome::Proposal(p) if p.path == DecisionPath::Fallback),
            "首次超时应回退 fast-path（Fallback）：{r2:?}"
        );
        let r3 = coord.execute(&state, 620_000);
        let DecisionOutcome::Proposal(p3) = &r3 else {
            panic!("连续超时必须降级产出 Proposal: {r3:?}");
        };
        assert_eq!(p3.path, DecisionPath::Degraded, "连续超时必须降级（AI-02）");
        assert!(coord.degraded, "degraded 标记必须置位（供上层告警）");
        let same_sched = p1.schedule.schedule.len() == p3.schedule.schedule.len()
            && p1
                .schedule
                .schedule
                .iter()
                .zip(&p3.schedule.schedule)
                .all(|(a, b)| {
                    a.period == b.period && (a.net_power_kw - b.net_power_kw).abs() < 1e-9
                });
        assert!(same_sched, "降级必须复用上次已提交调度（AI-02 + WP-40.3）");
        TIMEOUT_MODE.store(false, core::sync::atomic::Ordering::SeqCst);
    }

    /// 验收 3：InferParams 默认 timeout 有界（5s），scheduler 超时检测生效。
    #[test]
    fn test_infer_params_timeout_bounded() {
        assert_eq!(InferParams::default().timeout_ms, 5_000);
    }
}
