//! 集成测试 — MvpOrchestrator T1~T24.

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use eneros_agent::AgentState;
use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_energy_market_agent::{
    AgentRuntimeError, EnergyAgent, MarketAgent, MarketData, MarketDataSource, MarketSignal,
    MockMarketSource,
};

use crate::error::MvpError;
use crate::orchestrator::{MvpOrchestrator, MvpTickReport};
use crate::revenue::RevenueComparator;
use crate::traditional_ems::TraditionalEms;

// ===== 辅助函数 =====

/// 构造 96 时段电价预测.
fn make_price_forecast() -> Vec<f64> {
    vec![0.5; 96]
}

/// 构造一条 MarketData.
fn make_market_data(price: f64) -> MarketData {
    MarketData {
        timestamp: 1000,
        price_forecast: make_price_forecast(),
        current_price: price,
        load_forecast: None,
        signal_type: MarketSignal::RealtimePrice,
    }
}

// ===== T1: MvpError 变体构造（AgentError + NotRunning）=====
#[test]
fn t1_mvp_error_variants() {
    let _ = MvpError::NotRunning;
    let _ = MvpError::AgentError(AgentRuntimeError::NotRunning);
    let _ = MvpError::AgentError(AgentRuntimeError::ChannelError(
        alloc::string::String::from("ch"),
    ));
    assert!(matches!(MvpError::NotRunning, MvpError::NotRunning));
    assert!(matches!(
        MvpError::AgentError(AgentRuntimeError::NotRunning),
        MvpError::AgentError(_)
    ));
}

// ===== T2: From<AgentRuntimeError> for MvpError 转换 =====
#[test]
fn t2_from_agent_runtime_error() {
    let err: MvpError = AgentRuntimeError::NotRunning.into();
    assert!(matches!(
        err,
        MvpError::AgentError(AgentRuntimeError::NotRunning)
    ));
    let err2: MvpError = MvpError::from(AgentRuntimeError::ChannelError(
        alloc::string::String::from("x"),
    ));
    assert!(matches!(err2, MvpError::AgentError(_)));
}

// ===== T3: RevenueComparator::new 空 =====
#[test]
fn t3_revenue_comparator_new_empty() {
    let rc = RevenueComparator::new();
    assert!(rc.dual_brain_revenue.is_empty());
    assert!(rc.traditional_revenue.is_empty());
    assert_eq!(rc.dual_brain_total(), 0.0);
    assert_eq!(rc.traditional_total(), 0.0);
}

// ===== T4: RevenueComparator::record_dual_brain + dual_brain_total =====
#[test]
fn t4_revenue_comparator_record_dual_brain() {
    let mut rc = RevenueComparator::new();
    rc.record_dual_brain(100.0);
    rc.record_dual_brain(50.0);
    assert_eq!(rc.dual_brain_revenue.len(), 2);
    assert!((rc.dual_brain_total() - 150.0).abs() < 1e-9);
}

// ===== T5: RevenueComparator::record_traditional + traditional_total =====
#[test]
fn t5_revenue_comparator_record_traditional() {
    let mut rc = RevenueComparator::new();
    rc.record_traditional(80.0);
    rc.record_traditional(20.0);
    assert_eq!(rc.traditional_revenue.len(), 2);
    assert!((rc.traditional_total() - 100.0).abs() < 1e-9);
}

// ===== T6: RevenueComparator::improvement_pct 正常计算（dual=100, trad=80 → 25.0）=====
#[test]
fn t6_revenue_comparator_improvement_pct_normal() {
    let mut rc = RevenueComparator::new();
    rc.record_dual_brain(100.0);
    rc.record_traditional(80.0);
    // (100 - 80) / 80 * 100 = 25.0
    assert!((rc.improvement_pct() - 25.0).abs() < 1e-9);
}

// ===== T7: RevenueComparator::improvement_pct traditional=0 返回 INFINITY =====
#[test]
fn t7_revenue_comparator_improvement_pct_trad_zero() {
    let mut rc = RevenueComparator::new();
    rc.record_dual_brain(100.0);
    // traditional_revenue 为空 → traditional_total() = 0.0
    assert!(rc.improvement_pct().is_infinite());
    assert!(rc.improvement_pct() > 0.0);
}

// ===== T8: RevenueComparator::meets_target（improvement ≥ 10%）=====
#[test]
fn t8_revenue_comparator_meets_target() {
    let mut rc_pass = RevenueComparator::new();
    rc_pass.record_dual_brain(110.0);
    rc_pass.record_traditional(100.0);
    // (110 - 100) / 100 * 100 = 10.0 → 刚好达标
    assert!(rc_pass.meets_target());

    let mut rc_fail = RevenueComparator::new();
    rc_fail.record_dual_brain(105.0);
    rc_fail.record_traditional(100.0);
    // (105 - 100) / 100 * 100 = 5.0 → 未达标
    assert!(!rc_fail.meets_target());
}

// ===== T9: RevenueComparator::report 返回非空字符串 =====
#[test]
fn t9_revenue_comparator_report() {
    let mut rc = RevenueComparator::new();
    rc.record_dual_brain(100.0);
    rc.record_traditional(80.0);
    let s = rc.report();
    assert!(!s.is_empty());
    // 报告应包含双脑总收益、传统总收益、提升百分比、达标结果.
    assert!(s.contains("dual_brain_total"));
    assert!(s.contains("traditional_total"));
    assert!(s.contains("improvement_pct"));
    assert!(s.contains("meets_target_10pct"));
    assert!(s.contains("PASS")); // 25% ≥ 10% → PASS
}

// ===== T10: TraditionalEms::new 构造 =====
#[test]
fn t10_traditional_ems_new() {
    let ems = TraditionalEms::new(ScheduleConfig::default());
    assert_eq!(ems.config.num_periods, 96);
    assert_eq!(ems.config.pcs_power_kw, 100.0);
}

// ===== T11: TraditionalEms::schedule 谷时充电（price < 0.3）=====
#[test]
fn t11_traditional_ems_valley_charge() {
    let config = ScheduleConfig {
        price: vec![0.2; 96],
        ..ScheduleConfig::default()
    };
    let ems = TraditionalEms::new(config);
    let result = ems.schedule(0.2, 0.5);
    assert_eq!(result.schedule.len(), 96);
    // 谷时充电：charge = pcs_power_kw, discharge = 0
    for entry in &result.schedule {
        assert!((entry.charge_power_kw - 100.0).abs() < 1e-9);
        assert!((entry.discharge_power_kw - 0.0).abs() < 1e-9);
        // net = discharge - charge = -100
        assert!((entry.net_power_kw - (-100.0)).abs() < 1e-9);
    }
}

// ===== T12: TraditionalEms::schedule 峰时放电（price > 0.8）=====
#[test]
fn t12_traditional_ems_peak_discharge() {
    let config = ScheduleConfig {
        price: vec![0.9; 96],
        ..ScheduleConfig::default()
    };
    let ems = TraditionalEms::new(config);
    let result = ems.schedule(0.9, 0.5);
    assert_eq!(result.schedule.len(), 96);
    // 峰时放电：charge = 0, discharge = pcs_power_kw
    for entry in &result.schedule {
        assert!((entry.charge_power_kw - 0.0).abs() < 1e-9);
        assert!((entry.discharge_power_kw - 100.0).abs() < 1e-9);
        // net = discharge - charge = 100
        assert!((entry.net_power_kw - 100.0).abs() < 1e-9);
    }
}

// ===== T13: TraditionalEms::schedule 平时保持（0.3 ≤ price ≤ 0.8）=====
#[test]
fn t13_traditional_ems_flat_hold() {
    let config = ScheduleConfig {
        price: vec![0.5; 96],
        ..ScheduleConfig::default()
    };
    let ems = TraditionalEms::new(config);
    let result = ems.schedule(0.5, 0.5);
    assert_eq!(result.schedule.len(), 96);
    // 平时保持：charge = 0, discharge = 0
    for entry in &result.schedule {
        assert!((entry.charge_power_kw - 0.0).abs() < 1e-9);
        assert!((entry.discharge_power_kw - 0.0).abs() < 1e-9);
        assert!((entry.net_power_kw - 0.0).abs() < 1e-9);
        // revenue = 0
        assert!((entry.revenue_yuan - 0.0).abs() < 1e-9);
    }
    // 总收益 = 0
    assert!((result.total_revenue_yuan - 0.0).abs() < 1e-9);
}

// ===== T14: TraditionalEms::schedule 全 96 时段 + total_revenue_yuan 求和正确 =====
#[test]
fn t14_traditional_ems_full_schedule_sum() {
    // 混合电价：前 32 谷（0.2）+ 中 32 平（0.5）+ 后 32 峰（0.9）
    let mut prices = vec![0.2; 32];
    prices.extend(vec![0.5; 32]);
    prices.extend(vec![0.9; 32]);
    assert_eq!(prices.len(), 96);
    let config = ScheduleConfig {
        price: prices,
        ..ScheduleConfig::default()
    };
    let ems = TraditionalEms::new(config);
    let result = ems.schedule(0.5, 0.5);
    assert_eq!(result.schedule.len(), 96);
    // 手动求和
    let manual_sum: f64 = result.schedule.iter().map(|e| e.revenue_yuan).sum();
    assert!((result.total_revenue_yuan - manual_sum).abs() < 1e-9);
    // objective_value 应等于 total_revenue_yuan
    assert!((result.objective_value - result.total_revenue_yuan).abs() < 1e-9);
    // 谷时收益：(-100) * 0.2 * 0.25 * 32 = -160.0
    // 平时收益：0 * 0.5 * 0.25 * 32 = 0.0
    // 峰时收益：100 * 0.9 * 0.25 * 32 = 720.0
    // 总收益 = -160 + 0 + 720 = 560.0
    assert!((result.total_revenue_yuan - 560.0).abs() < 1e-9);
}

// ===== T15: MvpOrchestrator::new_default 构造 + 3 Agent + tick_count=0 + running=false =====
#[test]
fn t15_orchestrator_new_default() {
    let orch = MvpOrchestrator::new_default(1000);
    assert_eq!(orch.tick_count, 0);
    assert!(!orch.running);
    // 3 个 Agent 都已构造
    assert_eq!(orch.energy_agent.state, AgentState::Created);
    assert_eq!(orch.market_agent.state, AgentState::Created);
    assert_eq!(orch.device_agent.state, AgentState::Created);
    // 收益对比器为空
    assert!(orch.revenue_comparator.dual_brain_revenue.is_empty());
    assert!(orch.revenue_comparator.traditional_revenue.is_empty());
}

// ===== T16: MvpOrchestrator::start 全部 Agent 转 Running + running=true =====
#[test]
fn t16_orchestrator_start_agents_running() {
    let mut orch = MvpOrchestrator::new_default(0);
    let result = orch.start(1000);
    assert!(result.is_ok());
    assert!(orch.running);
    assert_eq!(orch.energy_agent.state, AgentState::Running);
    assert_eq!(orch.market_agent.state, AgentState::Running);
    assert_eq!(orch.device_agent.state, AgentState::Running);
}

// ===== T17: MvpOrchestrator::tick 未运行返回 NotRunning 错误 =====
#[test]
fn t17_orchestrator_tick_not_running_error() {
    let mut orch = MvpOrchestrator::new_default(0);
    // 未调用 start，running == false
    let result = orch.tick(1000);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), MvpError::NotRunning));
    // tick_count 不应增加
    assert_eq!(orch.tick_count, 0);
}

// ===== T18: MvpOrchestrator::tick 单周期执行 + tick_count += 1 =====
#[test]
fn t18_orchestrator_tick_single_period() {
    let mut orch = MvpOrchestrator::new_default(0);
    orch.start(1000).unwrap();
    let result = orch.tick(2000);
    assert!(result.is_ok());
    let report = result.unwrap();
    assert_eq!(report.tick, 1);
    assert_eq!(orch.tick_count, 1);
    // Energy Agent 应已执行双脑，产生 schedule
    assert!(orch.energy_agent.current_schedule.is_some());
    // 3 个 Agent 自身的 tick_count 也应为 1
    assert_eq!(orch.market_agent.tick_count, 1);
    assert_eq!(orch.energy_agent.tick_count, 1);
    assert_eq!(orch.device_agent.tick_count, 1);
}

// ===== T19: MvpOrchestrator::tick 市场数据从 market → energy 流转（current_price 更新）=====
#[test]
fn t19_orchestrator_market_data_flow() {
    // 用自定义 MarketAgent（带 MockMarketSource 预加载数据）构造 Orchestrator.
    let source: Box<dyn MarketDataSource> =
        Box::new(MockMarketSource::with_data(vec![make_market_data(0.88)]));
    let market = MarketAgent::new("market", source, 0);
    let energy = EnergyAgent::new_default(0);
    let device = eneros_device_agent::DeviceAgent::new_default(0);
    let mut orch = MvpOrchestrator::new(energy, market, device, ScheduleConfig::default(), 0);

    orch.start(1000).unwrap();
    orch.tick(2000).unwrap();

    // 市场数据应从 market_agent 流转到 energy_agent，current_price 更新为 0.88.
    assert!((orch.energy_agent.current_price - 0.88).abs() < 1e-9);
    // market_agent 应已消费 source 中的数据（tick_count == 1）
    assert_eq!(orch.market_agent.tick_count, 1);
    // market_agent.market_channel 应已被清空（数据转发到 energy）
    assert!(orch.market_agent.market_channel.is_empty());
}

// ===== T20: MvpOrchestrator::tick 记录双脑收益 + 传统收益 =====
#[test]
fn t20_orchestrator_tick_records_revenue() {
    let mut orch = MvpOrchestrator::new_default(0);
    orch.start(1000).unwrap();
    let report = orch.tick(2000).unwrap();

    // 双脑收益 + 传统收益各记录 1 条
    assert_eq!(orch.revenue_comparator.dual_brain_revenue.len(), 1);
    assert_eq!(orch.revenue_comparator.traditional_revenue.len(), 1);

    // 双脑收益应等于 current_schedule.total_revenue_yuan
    let expected_dual = orch
        .energy_agent
        .current_schedule
        .as_ref()
        .map(|s| s.total_revenue_yuan)
        .unwrap_or(0.0);
    assert!((orch.revenue_comparator.dual_brain_total() - expected_dual).abs() < 1e-9);

    // report 字段应与 comparator 一致
    assert!((report.dual_brain_revenue - orch.revenue_comparator.dual_brain_total()).abs() < 1e-9);
    assert!(
        (report.traditional_revenue - orch.revenue_comparator.traditional_total()).abs() < 1e-9
    );
}

// ===== T21: MvpOrchestrator 多 tick 累积收益 =====
#[test]
fn t21_orchestrator_multi_tick_accumulate() {
    let mut orch = MvpOrchestrator::new_default(0);
    orch.start(1000).unwrap();

    for i in 1..=3 {
        let report = orch.tick(1000 + i * 1000).unwrap();
        assert_eq!(report.tick, i);
        assert_eq!(orch.tick_count, i);
    }

    // 3 ticks → 3 条收益记录
    assert_eq!(orch.revenue_comparator.dual_brain_revenue.len(), 3);
    assert_eq!(orch.revenue_comparator.traditional_revenue.len(), 3);

    // 累计收益 = 各 tick 收益之和
    let expected_dual: f64 = orch
        .revenue_comparator
        .dual_brain_revenue
        .iter()
        .copied()
        .sum();
    let expected_trad: f64 = orch
        .revenue_comparator
        .traditional_revenue
        .iter()
        .copied()
        .sum();
    assert!((orch.revenue_comparator.dual_brain_total() - expected_dual).abs() < 1e-9);
    assert!((orch.revenue_comparator.traditional_total() - expected_trad).abs() < 1e-9);
}

// ===== T22: MvpOrchestrator::stop 全部 Agent 转 Dead + running=false =====
#[test]
fn t22_orchestrator_stop_agents_dead() {
    let mut orch = MvpOrchestrator::new_default(0);
    orch.start(1000).unwrap();
    orch.tick(2000).unwrap();
    let result = orch.stop(3000);
    assert!(result.is_ok());
    assert!(!orch.running);
    assert_eq!(orch.energy_agent.state, AgentState::Dead);
    assert_eq!(orch.market_agent.state, AgentState::Dead);
    assert_eq!(orch.device_agent.state, AgentState::Dead);
}

// ===== T23: MvpOrchestrator 端到端：start → 3 ticks → stop → report 非空 =====
#[test]
fn t23_orchestrator_end_to_end() {
    let mut orch = MvpOrchestrator::new_default(0);

    // start
    orch.start(1000).unwrap();
    assert!(orch.running);

    // 3 ticks
    for i in 1..=3 {
        let report: MvpTickReport = orch.tick(1000 + i * 1000).unwrap();
        assert_eq!(report.tick, i);
    }
    assert_eq!(orch.tick_count, 3);

    // stop
    orch.stop(5000).unwrap();
    assert!(!orch.running);

    // report 非空
    let report = orch.report();
    assert!(!report.is_empty());
    assert!(report.contains("dual_brain_total"));
    assert!(report.contains("improvement_pct"));
}

// ===== T24: MvpOrchestrator::report 委托 RevenueComparator =====
#[test]
fn t24_orchestrator_report_delegates() {
    let mut orch = MvpOrchestrator::new_default(0);
    orch.start(1000).unwrap();
    orch.tick(2000).unwrap();

    // Orchestrator::report 应等于内部 revenue_comparator.report()
    let orch_report = orch.report();
    let comparator_report = orch.revenue_comparator.report();
    assert_eq!(orch_report.to_string(), comparator_report.to_string());
}
