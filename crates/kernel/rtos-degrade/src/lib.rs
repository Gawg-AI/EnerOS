//! EnerOS RTOS 降级规则引擎 — Phase 1 (v0.57.0).
//!
//! 本 crate 实现 RTOS 控制大区的安全降级机制，包括：
//! - [`mode::DegradeMode`] — 五级降级模式（Normal→HoldOutput→StopCharge→SafeDefault→EmergencyStop）
//! - [`context::DegradeContext`] — 规则评估输入（Agent 心跳/总线/通信/电池/温度）
//! - [`rule::DegradeRule`] — 降级规则 trait（名称/优先级/评估）
//! - [`engine::DegradeEngine`] — 降级引擎（规则评估 + 模式切换 + 动作下发）
//! - [`builtin`] — 5 条内置规则（AgentDead/ControlBusDown/DeviceCommFail/LowBattery/OverTemp）
//! - [`safe_defaults::SafeDefaults`] — 安全默认值表
//! - [`stats::DegradeStats`] / [`stats::DegradeReport`] — 统计与报告
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | crate 放入 `crates/kernel/rtos-degrade/`（P1-H RTOS 组件第四层） |
//! | **D2** | 复用 v0.51.0 `PointAccess` trait 下发降级动作 |
//! | **D3** | 复用 v0.56.0 `DevicePointMap` 做 `DeviceId→PointId` 映射 |
//! | **D4** | `DegradeMode` 派生 `Ord` 以支持严重程度比较 |
//! | **D5** | `now_ns: u64` 参数注入（蓝图 `MonotonicTime::now()` 不存在） |
//! | **D6** | 泛型 `<P: PointAccess>`（蓝图 `Box<dyn PointAccess>`）+ `DegradeRule` 不要求 `Send + Sync` |
//! | **D7** | 不使用 `log_warn!`（no_std，用 stats 计数器） |
//! | **D8** | 插入时按 `priority` 降序排序（蓝图 `evaluate` 中 `sort` 性能差） |
//! | **D9** | 复用 v0.56.0 `DevicePointMap`（蓝图 `EMERGENCY_STOP_ID`/`POWER_CMD_ID` 未定义常量） |
//! | **D10** | `DegradeContext` 含 `battery_soc`/`grid_frequency`/`temperature`（蓝图字段） |
//! | **D11** | `EmergencyStop` 不可自动恢复（蓝图风险 8.4）—— 由调用方控制，引擎不自动回切 |
//! | **D12** | 内置 5 规则覆盖常见故障（AgentDead/ControlBusDown/DeviceCommFail/LowBattery/OverTemp） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，不 `use std::*`，不 `panic!`/`todo!`/`unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod builtin;
pub mod context;
pub mod engine;
pub mod error;
pub mod mode;
pub mod rule;
pub mod safe_defaults;
pub mod stats;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_controlbus::DeviceId;
    use eneros_rtos_cmd_exec::device_map::DevicePointMap;
    use eneros_upa_model::{PointId, PointValue};

    use crate::builtin::{
        AgentDeadRule, ControlBusDownRule, DeviceCommFailRule, LowBatteryRule, OverTempRule,
        HEARTBEAT_TIMEOUT_NS,
    };
    use crate::context::DegradeContext;
    use crate::engine::DegradeEngine;
    use crate::mock::MockPointAccess;
    use crate::mode::DegradeMode;
    use crate::rule::DegradeRule;
    use crate::safe_defaults::SafeDefaults;

    // ===== 测试辅助函数 =====

    const TEST_DEVICE: DeviceId = DeviceId(1);
    const TEST_POINT: PointId = 100;

    /// 构造含 TEST_DEVICE→TEST_POINT 的映射.
    fn make_device_map() -> DevicePointMap {
        let mut m = DevicePointMap::new();
        m.insert(TEST_DEVICE, TEST_POINT);
        m
    }

    /// 构造含 TEST_POINT→50.0 的安全默认值.
    fn make_safe_defaults() -> SafeDefaults {
        let mut sd = SafeDefaults::new();
        sd.insert(TEST_POINT, 50.0);
        sd
    }

    /// 构造正常上下文（所有指标健康）.
    fn make_normal_ctx() -> DegradeContext {
        DegradeContext::new()
            .with_now_ns(10_000_000_000)
            .with_agent_alive(true)
            .with_agent_last_heartbeat_ns(9_000_000_000)
            .with_control_bus_active(true)
            .with_device_comm_ok(true)
            .with_battery_soc(80.0)
            .with_grid_frequency(50.0)
            .with_temperature(25.0)
    }

    /// 构造含全部 5 条内置规则的引擎.
    fn make_full_engine() -> DegradeEngine<MockPointAccess> {
        let mut engine = DegradeEngine::new(
            MockPointAccess::new(),
            make_device_map(),
            make_safe_defaults(),
        );
        engine.add_rule(Box::new(AgentDeadRule));
        engine.add_rule(Box::new(ControlBusDownRule));
        engine.add_rule(Box::new(DeviceCommFailRule));
        engine.add_rule(Box::new(LowBatteryRule));
        engine.add_rule(Box::new(OverTempRule));
        engine
    }

    /// 断言 PointValue 为 Float 且近似等于 expected.
    fn assert_float_value(v: &PointValue, expected: f64) {
        assert!(
            matches!(v, PointValue::Float(x) if (x - expected).abs() < 1e-9),
            "expected Float({}), got {:?}",
            expected,
            v
        );
    }

    // ===== T1：DegradeMode Ord 排序 + is_degraded =====
    #[test]
    fn test_t1_mode_ordering() {
        assert!(DegradeMode::Normal < DegradeMode::HoldOutput);
        assert!(DegradeMode::HoldOutput < DegradeMode::StopCharge);
        assert!(DegradeMode::StopCharge < DegradeMode::SafeDefault);
        assert!(DegradeMode::SafeDefault < DegradeMode::EmergencyStop);

        assert!(!DegradeMode::Normal.is_degraded());
        assert!(DegradeMode::HoldOutput.is_degraded());
        assert!(DegradeMode::StopCharge.is_degraded());
        assert!(DegradeMode::SafeDefault.is_degraded());
        assert!(DegradeMode::EmergencyStop.is_degraded());
    }

    // ===== T2：DegradeMode Default 返回 Normal =====
    #[test]
    fn test_t2_mode_default() {
        let mode: DegradeMode = Default::default();
        assert_eq!(mode, DegradeMode::Normal);
        assert!(!mode.is_degraded());
    }

    // ===== T3：DegradeContext builder 模式 =====
    #[test]
    fn test_t3_context_builder() {
        let ctx = DegradeContext::new()
            .with_now_ns(1000)
            .with_agent_alive(true)
            .with_agent_last_heartbeat_ns(500)
            .with_control_bus_active(true)
            .with_device_comm_ok(true)
            .with_battery_soc(75.5)
            .with_grid_frequency(49.8)
            .with_temperature(30.2);

        assert_eq!(ctx.now_ns, 1000);
        assert!(ctx.agent_alive);
        assert_eq!(ctx.agent_last_heartbeat_ns, 500);
        assert!(ctx.control_bus_active);
        assert!(ctx.device_comm_ok);
        assert!((ctx.battery_soc - 75.5).abs() < 1e-9);
        assert!((ctx.grid_frequency - 49.8).abs() < 1e-9);
        assert!((ctx.temperature - 30.2).abs() < 1e-9);
    }

    // ===== T4：SafeDefaults insert/get/iter =====
    #[test]
    fn test_t4_safe_defaults() {
        let mut sd = SafeDefaults::new();
        assert!(sd.is_empty());
        assert_eq!(sd.get(1), None);

        sd.insert(1, 10.0);
        sd.insert(5, 50.0);
        sd.insert(3, 30.0);
        assert_eq!(sd.len(), 3);

        assert_eq!(sd.get(1), Some(10.0));
        assert_eq!(sd.get(5), Some(50.0));
        assert_eq!(sd.get(3), Some(30.0));
        assert_eq!(sd.get(99), None);

        // iter 按 PointId 升序
        let pairs: Vec<(PointId, f64)> = sd.iter().collect();
        assert_eq!(pairs, vec![(1, 10.0), (3, 30.0), (5, 50.0)]);
    }

    // ===== T5：AgentDeadRule — agent_alive=false → SafeDefault =====
    #[test]
    fn test_t5_agent_dead_rule_not_alive() {
        let rule = AgentDeadRule;
        assert_eq!(rule.name(), "agent_dead");
        assert_eq!(rule.priority(), 100);

        let ctx = DegradeContext::new().with_agent_alive(false);
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::SafeDefault));
    }

    // ===== T6：AgentDeadRule — 心跳超时 → SafeDefault =====
    #[test]
    fn test_t6_agent_dead_rule_heartbeat_timeout() {
        let rule = AgentDeadRule;
        let ctx = DegradeContext::new()
            .with_now_ns(10_000_000_000)
            .with_agent_alive(true)
            .with_agent_last_heartbeat_ns(4_000_000_000); // 6s 前 > 5s 超时
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::SafeDefault));

        // 恰好 5s 不超时（边界：elapsed == timeout 不触发）
        let ctx_boundary = DegradeContext::new()
            .with_now_ns(5_000_000_000)
            .with_agent_alive(true)
            .with_agent_last_heartbeat_ns(0);
        assert_eq!(
            ctx_boundary.now_ns - ctx_boundary.agent_last_heartbeat_ns,
            HEARTBEAT_TIMEOUT_NS
        );
        assert_eq!(rule.evaluate(&ctx_boundary), None);
    }

    // ===== T7：AgentDeadRule — 正常 → None =====
    #[test]
    fn test_t7_agent_dead_rule_normal() {
        let rule = AgentDeadRule;
        let ctx = DegradeContext::new()
            .with_now_ns(10_000_000_000)
            .with_agent_alive(true)
            .with_agent_last_heartbeat_ns(9_000_000_000); // 1s 前 < 5s
        assert_eq!(rule.evaluate(&ctx), None);
    }

    // ===== T8：ControlBusDownRule → HoldOutput =====
    #[test]
    fn test_t8_control_bus_down_rule() {
        let rule = ControlBusDownRule;
        assert_eq!(rule.name(), "control_bus_down");
        assert_eq!(rule.priority(), 90);

        let ctx = DegradeContext::new().with_control_bus_active(false);
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::HoldOutput));

        let ctx_ok = DegradeContext::new().with_control_bus_active(true);
        assert_eq!(rule.evaluate(&ctx_ok), None);
    }

    // ===== T9：DeviceCommFailRule → SafeDefault =====
    #[test]
    fn test_t9_device_comm_fail_rule() {
        let rule = DeviceCommFailRule;
        assert_eq!(rule.name(), "device_comm_fail");
        assert_eq!(rule.priority(), 80);

        let ctx = DegradeContext::new().with_device_comm_ok(false);
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::SafeDefault));

        let ctx_ok = DegradeContext::new().with_device_comm_ok(true);
        assert_eq!(rule.evaluate(&ctx_ok), None);
    }

    // ===== T10：LowBatteryRule → StopCharge =====
    #[test]
    fn test_t10_low_battery_rule() {
        let rule = LowBatteryRule;
        assert_eq!(rule.name(), "low_battery");
        assert_eq!(rule.priority(), 70);

        let ctx = DegradeContext::new().with_battery_soc(5.0);
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::StopCharge));

        let ctx_ok = DegradeContext::new().with_battery_soc(80.0);
        assert_eq!(rule.evaluate(&ctx_ok), None);
    }

    // ===== T11：OverTempRule → StopCharge =====
    #[test]
    fn test_t11_over_temp_rule() {
        let rule = OverTempRule;
        assert_eq!(rule.name(), "over_temp");
        assert_eq!(rule.priority(), 60);

        let ctx = DegradeContext::new().with_temperature(90.0);
        assert_eq!(rule.evaluate(&ctx), Some(DegradeMode::StopCharge));

        let ctx_ok = DegradeContext::new().with_temperature(25.0);
        assert_eq!(rule.evaluate(&ctx_ok), None);
    }

    // ===== T12：Engine — 正常上下文 → Normal，无切换 =====
    #[test]
    fn test_t12_engine_normal_no_change() {
        let mut engine = make_full_engine();
        assert_eq!(engine.current_mode(), DegradeMode::Normal);
        assert_eq!(engine.rule_count(), 5);

        let report = engine.evaluate(&make_normal_ctx(), 10_000_000_000);
        assert_eq!(report.new_mode, DegradeMode::Normal);
        assert!(!report.mode_changed);
        assert!(!report.action_taken);
        assert_eq!(engine.current_mode(), DegradeMode::Normal);
    }

    // ===== T13：Engine — Agent 死亡 → SafeDefault，安全默认值已下发 =====
    #[test]
    fn test_t13_engine_agent_dead_safe_default_written() {
        let mut engine = make_full_engine();
        let ctx = make_normal_ctx().with_agent_alive(false);

        let report = engine.evaluate(&ctx, 10_000_000_000);
        assert_eq!(report.new_mode, DegradeMode::SafeDefault);
        assert!(report.mode_changed);
        assert!(report.action_taken);
        assert_eq!(engine.current_mode(), DegradeMode::SafeDefault);
        assert_eq!(engine.previous_mode(), DegradeMode::Normal);

        // 安全默认值 50.0 已下发到 TEST_POINT
        let v = engine
            .protocol()
            .last_write(TEST_POINT)
            .expect("safe default written");
        assert_float_value(v, 50.0);
    }

    // ===== T14：Engine — 多规则触发，最高优先级胜出 =====
    #[test]
    fn test_t14_engine_priority_ordering() {
        let mut engine = make_full_engine();

        // 同时触发 AgentDead(100→SafeDefault)、ControlBusDown(90→HoldOutput)、LowBattery(70→StopCharge)
        let ctx = make_normal_ctx()
            .with_agent_alive(false)
            .with_control_bus_active(false)
            .with_battery_soc(5.0);

        let report = engine.evaluate(&ctx, 10_000_000_000);
        // AgentDead 优先级最高(100) → SafeDefault
        assert_eq!(report.new_mode, DegradeMode::SafeDefault);
        assert!(report.mode_changed);
    }

    // ===== T15：Engine — EmergencyStop 锁定（D11：不自动回切）=====
    #[test]
    fn test_t15_engine_emergency_stop_lockin() {
        let mut engine = make_full_engine();

        // 1. 通过 force_mode 进入 EmergencyStop
        let report = engine.force_mode(DegradeMode::EmergencyStop, 1_000_000_000);
        assert!(report.mode_changed);
        assert!(report.action_taken);
        assert_eq!(engine.current_mode(), DegradeMode::EmergencyStop);

        // 验证 Bool(true) 已下发到 TEST_POINT
        let v = engine
            .protocol()
            .last_write(TEST_POINT)
            .expect("emergency stop written");
        assert!(matches!(v, PointValue::Bool(true)));

        // 2. evaluate 正常上下文 → 应锁定在 EmergencyStop，不回切
        let report2 = engine.evaluate(&make_normal_ctx(), 2_000_000_000);
        assert_eq!(report2.new_mode, DegradeMode::EmergencyStop);
        assert!(!report2.mode_changed);
        assert!(!report2.action_taken);
        assert_eq!(engine.current_mode(), DegradeMode::EmergencyStop);

        // 3. 通过 force_mode 显式恢复到 Normal
        let report3 = engine.force_mode(DegradeMode::Normal, 3_000_000_000);
        assert!(report3.mode_changed);
        assert_eq!(engine.current_mode(), DegradeMode::Normal);
        assert_eq!(engine.previous_mode(), DegradeMode::EmergencyStop);
    }

    // ===== T16：Engine — 统计累加（evaluations_count / mode_switch_count）=====
    #[test]
    fn test_t16_engine_stats_accumulation() {
        let mut engine = make_full_engine();

        // 第 1 次：正常 → Normal，无切换
        let r1 = engine.evaluate(&make_normal_ctx(), 1_000_000_000);
        assert!(!r1.mode_changed);
        assert_eq!(engine.stats().evaluations_count, 1);
        assert_eq!(engine.stats().mode_switch_count, 0);

        // 第 2 次：Agent 死亡 → SafeDefault，切换
        let ctx_dead = make_normal_ctx().with_agent_alive(false);
        let r2 = engine.evaluate(&ctx_dead, 2_000_000_000);
        assert!(r2.mode_changed);
        assert_eq!(engine.stats().evaluations_count, 2);
        assert_eq!(engine.stats().mode_switch_count, 1);
        assert_eq!(engine.stats().last_mode, DegradeMode::SafeDefault);
        assert_eq!(engine.stats().last_mode_switch_ns, 2_000_000_000);

        // 第 3 次：恢复正常 → Normal，切换
        let r3 = engine.evaluate(&make_normal_ctx(), 3_000_000_000);
        assert!(r3.mode_changed);
        assert_eq!(engine.stats().evaluations_count, 3);
        assert_eq!(engine.stats().mode_switch_count, 2);
        assert_eq!(engine.stats().last_mode, DegradeMode::Normal);
        assert_eq!(engine.stats().last_mode_switch_ns, 3_000_000_000);
    }
}
