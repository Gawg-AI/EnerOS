//! EnerOS 四遥数据模型（Telemetry Model，v0.52.0）.
//!
//! 定义电力"四遥"（遥测/遥信/遥控/遥调）的统一数据模型与死区过滤器，
//! 基于 `eneros-upa-model` 的 `PointId`/`DeviceId`/`PointValue` 构建四遥语义层。
//!
//! # 核心类型
//! - [`quality::QualityFlag`] — 数据品质标志（Good/Invalid/Questionable/Substituted/Blocked/Overflow/Outdated）
//! - [`digital::DigitalState`] — 数字量状态（Off/On/Intermediate/Bad）
//! - [`command::ControlCommand`] — 控制命令（Single/Double）
//! - [`command::ControlExecState`] — 控制执行状态机（Idle/Selected/Executing/Done/Failed/Timeout）
//! - [`telemetry::Telemetry`] — 遥测（模拟量，含死区/限值/品质）
//! - [`telesignaling::Telesignaling`] — 遥信（数字量，变化检测）
//! - [`telecontrol::Telecontrol`] — 遥控（控制命令，SBO 状态机）
//! - [`teleadjust::Teleadjust`] — 遥调（设定值，范围校验/偏差）
//! - [`deadband::DeadbandFilter`] — 批量死区过滤器（BTreeMap）
//!
//! # 偏差声明（D1~D7）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 时间戳用 `u64` 毫秒参数注入（蓝图 `MonotonicTime`/`SystemTime` 在 no_std 不存在；与 v0.50.0 D1、v0.51.0 D5 一致） |
//! | **D2** | crate 放入 `crates/protocols/telemetry-model/`（P1-G 四遥数据层，与 upa-model 同级） |
//! | **D3** | 仅依赖 `eneros-upa-model`（复用 PointId/DeviceId/PointValue），不依赖 protocol-abstract（四遥是数据模型，非协议适配器） |
//! | **D4** | 不实现 `DeviceDriver` trait（数据模型非设备驱动，与 v0.48.0~v0.51.0 一致） |
//! | **D5** | 不实现 `PointAccess` trait（四遥是数据定义层；PointAccess 是协议适配层；四遥数据通过 DataPoint 包装访问） |
//! | **D6** | `DeadbandFilter` 使用 `BTreeMap`（no_std 无 HashMap，BTreeMap 友好） |
//! | **D7** | 不要求 `Send + Sync`（no_std 单线程，与 v0.51.0 D2 一致） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，依赖 `eneros-upa-model`（纯数据模型）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod command;
pub mod deadband;
pub mod digital;
pub mod quality;
pub mod teleadjust;
pub mod telecontrol;
pub mod telemetry;
pub mod telesignaling;

pub use command::{ControlCommand, ControlExecState, DoubleCommand, SingleCommand};
pub use deadband::DeadbandFilter;
pub use digital::DigitalState;
pub use quality::QualityFlag;
pub use teleadjust::Teleadjust;
pub use telecontrol::Telecontrol;
pub use telemetry::Telemetry;
pub use telesignaling::Telesignaling;

#[cfg(test)]
mod tests {
    //! 跨模块集成测试 — 覆盖四遥数据模型全链路（T1~T20）。

    use command::{ControlCommand, ControlExecState, DoubleCommand, SingleCommand};
    use digital::DigitalState;
    use quality::QualityFlag;

    use super::*;

    // ===== T1：遥测死区过滤（变化 ≤ 死区 → 不上报；> 死区 → 上报）=====
    #[test]
    fn test_telemetry_deadband_filter() {
        let mut t = Telemetry::new(1, 10, "Ua", 100.0, "V", 1000);
        t.deadband = 1.0;
        // 首次上报
        assert!(t.should_report());
        assert_eq!(t.last_reported, Some(100.0));
        // 变化 0.5 ≤ 死区 1.0 → 不上报
        t.update(100.5, 2000);
        assert!(!t.should_report());
        // 变化 2.0 > 死区 1.0 → 上报
        t.update(102.5, 3000);
        assert!(t.should_report());
        assert_eq!(t.last_reported, Some(102.5));
    }

    // ===== T2：遥测首次上报（last_reported=None → 上报）=====
    #[test]
    fn test_telemetry_first_report() {
        let mut t = Telemetry::new(2, 10, "Ia", 5.0, "A", 100);
        assert!(t.last_reported.is_none());
        assert!(t.should_report());
        assert_eq!(t.last_reported, Some(5.0));
        // 再次调用，值未变且死区为 0，abs(0) > 0 为 false → 不上报
        assert!(!t.should_report());
    }

    // ===== T3：遥测品质检查（越限 → Questionable）=====
    #[test]
    fn test_telemetry_quality_check() {
        let mut t = Telemetry::new(3, 10, "P", 50.0, "kW", 0);
        t.high_limit = Some(100.0);
        t.low_limit = Some(0.0);
        // 正常范围内
        t.check_quality();
        assert_eq!(t.quality, QualityFlag::Good);
        // 超高限
        t.update(150.0, 1000);
        t.check_quality();
        assert_eq!(t.quality, QualityFlag::Questionable);
        // 恢复正常范围（品质不自动恢复，仍为 Questionable）
        t.update(50.0, 2000);
        t.check_quality();
        assert_eq!(t.quality, QualityFlag::Questionable);
    }

    // ===== T4：遥测 force_report =====
    #[test]
    fn test_telemetry_force_report() {
        let mut t = Telemetry::new(4, 10, "f", 50.0, "Hz", 0);
        t.deadband = 10.0;
        // 首次上报
        assert!(t.should_report());
        // 微小变化不上报
        t.update(51.0, 1000);
        assert!(!t.should_report());
        // force_report 后 last_reported 更新为当前值
        t.force_report();
        assert_eq!(t.last_reported, Some(51.0));
    }

    // ===== T5：遥信状态变化立即上报 =====
    #[test]
    fn test_telesignaling_state_change_report() {
        let mut s = Telesignaling::new(5, 10, "breaker", DigitalState::Off, false, 0);
        // 首次上报
        assert!(s.should_report());
        assert_eq!(s.last_reported, Some(DigitalState::Off));
        // 状态变化 → 上报
        s.update(DigitalState::On, 1000);
        assert!(s.should_report());
        assert_eq!(s.last_reported, Some(DigitalState::On));
    }

    // ===== T6：遥信状态未变不上报 =====
    #[test]
    fn test_telesignaling_state_unchanged_no_report() {
        let mut s = Telesignaling::new(6, 10, "alarm", DigitalState::On, true, 0);
        // 首次上报
        assert!(s.should_report());
        // 状态未变 → 不上报
        s.update(DigitalState::On, 1000);
        assert!(!s.should_report());
    }

    // ===== T7：遥控 SBO 流程（select → execute → complete）=====
    #[test]
    fn test_telecontrol_sbo_flow() {
        let mut c = Telecontrol::new(
            7,
            10,
            "cb",
            ControlCommand::Single(SingleCommand::On),
            true,
            0,
        );
        assert_eq!(c.exec_state, ControlExecState::Idle);
        // SBO 步骤 1：select
        assert!(c.select().is_ok());
        assert_eq!(c.exec_state, ControlExecState::Selected);
        // SBO 步骤 2：execute
        assert!(c.execute().is_ok());
        assert_eq!(c.exec_state, ControlExecState::Executing);
        // 执行完成
        c.complete();
        assert_eq!(c.exec_state, ControlExecState::Done);
        assert!(c.is_complete());
    }

    // ===== T8：遥控非 SBO 流程（execute → complete）=====
    #[test]
    fn test_telecontrol_non_sbo_flow() {
        let mut c = Telecontrol::new(
            8,
            10,
            "fan",
            ControlCommand::Double(DoubleCommand::On),
            false,
            0,
        );
        assert_eq!(c.exec_state, ControlExecState::Idle);
        // 非 SBO 模式：select 应失败
        assert!(c.select().is_err());
        // 直接 execute
        assert!(c.execute().is_ok());
        assert_eq!(c.exec_state, ControlExecState::Executing);
        c.complete();
        assert!(c.is_complete());
    }

    // ===== T9：遥控 fail/timeout（execute → fail / timeout）=====
    #[test]
    fn test_telecontrol_fail_and_timeout() {
        // fail 路径
        let mut c1 = Telecontrol::new(
            9,
            10,
            "v1",
            ControlCommand::Single(SingleCommand::Off),
            false,
            0,
        );
        assert!(c1.execute().is_ok());
        c1.fail();
        assert_eq!(c1.exec_state, ControlExecState::Failed);
        assert!(c1.exec_state.is_terminal());
        assert!(!c1.is_complete());

        // timeout 路径
        let mut c2 = Telecontrol::new(
            10,
            10,
            "v2",
            ControlCommand::Single(SingleCommand::On),
            false,
            0,
        );
        assert!(c2.execute().is_ok());
        c2.timeout();
        assert_eq!(c2.exec_state, ControlExecState::Timeout);
        assert!(c2.exec_state.is_terminal());
    }

    // ===== T10：遥控状态机错误（Idle 非选择直接 execute 返回错误）=====
    #[test]
    fn test_telecontrol_sbo_state_error() {
        let mut c = Telecontrol::new(
            11,
            10,
            "cb",
            ControlCommand::Single(SingleCommand::On),
            true,
            0,
        );
        // SBO 模式下 Idle 直接 execute 应失败
        let r = c.execute();
        assert!(r.is_err());
        assert_eq!(c.exec_state, ControlExecState::Idle);
        // select 后再 execute 才成功
        assert!(c.select().is_ok());
        assert!(c.execute().is_ok());
        // 已 Selected 后再次 select 应失败
        let mut c2 = Telecontrol::new(
            12,
            10,
            "cb2",
            ControlCommand::Single(SingleCommand::On),
            true,
            0,
        );
        assert!(c2.select().is_ok());
        assert!(c2.select().is_err());
    }

    // ===== T11：遥调设定值范围校验 =====
    #[test]
    fn test_teleadjust_setpoint_range_validation() {
        let mut a = Teleadjust::new(13, 10, "Vref", 50.0, 0.0, 100.0, 0);
        // 范围内
        assert!(a.validate(50.0));
        assert!(a.validate(0.0));
        assert!(a.validate(100.0));
        // 边界外
        assert!(!a.validate(-0.1));
        assert!(!a.validate(100.1));
        // set 范围内成功
        assert!(a.set(75.0, 1000).is_ok());
        assert_eq!(a.setpoint, 75.0);
        assert_eq!(a.timestamp_ms, 1000);
        // set 范围外失败
        assert!(a.set(150.0, 2000).is_err());
        assert_eq!(a.setpoint, 75.0); // 未变
    }

    // ===== T12：遥调偏差计算 =====
    #[test]
    fn test_teleadjust_deviation() {
        let mut a = Teleadjust::new(14, 10, "Pset", 100.0, 0.0, 200.0, 0);
        a.update_current(95.0, 1000);
        assert_eq!(a.deviation(), -5.0);
        a.update_current(110.0, 2000);
        assert_eq!(a.deviation(), 10.0);
        // is_in_range
        assert!(a.is_in_range());
        a.update_current(250.0, 3000);
        assert!(!a.is_in_range());
    }

    // ===== T13：DeadbandFilter 批量死区过滤 =====
    #[test]
    fn test_deadband_filter_batch() {
        let mut f = DeadbandFilter::new();
        f.configure(1, 1.0);
        f.configure(2, 2.0);
        assert_eq!(f.point_count(), 2);
        // 点 1：首次上报
        assert!(f.should_report(1, 100.0));
        // 点 1：变化 0.5 ≤ 死区 1.0 → 不上报
        assert!(!f.should_report(1, 100.5));
        // 点 1：变化 2.0 > 死区 1.0 → 上报
        assert!(f.should_report(1, 102.5));
        // 点 2：首次上报
        assert!(f.should_report(2, 50.0));
        // 点 2：变化 1.0 ≤ 死区 2.0 → 不上报
        assert!(!f.should_report(2, 51.0));
        // 点 3：未配置 → 直接上报
        assert!(f.should_report(3, 999.0));
    }

    // ===== T14：DeadbandFilter force_report =====
    #[test]
    fn test_deadband_filter_force_report() {
        let mut f = DeadbandFilter::new();
        f.configure(1, 10.0);
        // 首次上报
        assert!(f.should_report(1, 100.0));
        // 微小变化不上报
        assert!(!f.should_report(1, 101.0));
        // force_report 后 last_reported 更新
        f.force_report(1, 101.0);
        // 后续以 101.0 为基准判断
        assert!(!f.should_report(1, 102.0)); // 变化 1.0 ≤ 10.0 → 不上报
        assert!(f.should_report(1, 112.0)); // 变化 11.0 > 10.0 → 上报
    }

    // ===== T15：DeadbandFilter get_stats =====
    #[test]
    fn test_deadband_filter_get_stats() {
        let mut f = DeadbandFilter::new();
        f.configure(1, 1.0);
        // 未上报前 stats 为 (0, 0)
        assert_eq!(f.get_stats(1), Some((0, 0)));
        // 首次上报 → report_count = 1
        f.should_report(1, 100.0);
        assert_eq!(f.get_stats(1), Some((1, 0)));
        // 跳过一次 → skip_count = 1
        f.should_report(1, 100.5);
        assert_eq!(f.get_stats(1), Some((1, 1)));
        // 上报一次 → report_count = 2
        f.should_report(1, 102.0);
        assert_eq!(f.get_stats(1), Some((2, 1)));
        // 未配置点返回 None
        assert_eq!(f.get_stats(999), None);
    }

    // ===== T16：DeadbandFilter remove 点配置 =====
    #[test]
    fn test_deadband_filter_remove() {
        let mut f = DeadbandFilter::new();
        f.configure(1, 1.0);
        f.configure(2, 2.0);
        assert_eq!(f.point_count(), 2);
        // 移除存在的点
        assert!(f.remove(1));
        assert_eq!(f.point_count(), 1);
        // 移除后该点未配置 → should_report 直接返回 true
        assert!(f.should_report(1, 100.0));
        // 移除不存在的点
        assert!(!f.remove(999));
        assert_eq!(f.point_count(), 1);
    }

    // ===== T17：DeadbandFilter 死区 0（全部上报）=====
    #[test]
    fn test_deadband_filter_zero_deadband() {
        let mut f = DeadbandFilter::new();
        f.configure(1, 0.0);
        // 首次上报
        assert!(f.should_report(1, 100.0));
        // 死区 0：abs(任意差值) > 0 当差值不为 0 时为 true
        assert!(f.should_report(1, 100.1));
        assert!(f.should_report(1, 99.9));
        // 差值为 0 → abs(0) > 0 为 false → 不上报
        assert!(!f.should_report(1, 99.9));
    }

    // ===== T18：QualityFlag is_valid/is_error 语义 =====
    #[test]
    fn test_quality_flag_semantics() {
        assert!(QualityFlag::Good.is_valid());
        assert!(!QualityFlag::Good.is_error());

        assert!(!QualityFlag::Invalid.is_valid());
        assert!(QualityFlag::Invalid.is_error());

        assert!(!QualityFlag::Questionable.is_valid());
        assert!(!QualityFlag::Questionable.is_error());

        assert!(!QualityFlag::Substituted.is_valid());
        assert!(!QualityFlag::Substituted.is_error());

        assert!(!QualityFlag::Blocked.is_valid());
        assert!(QualityFlag::Blocked.is_error());

        assert!(!QualityFlag::Overflow.is_valid());
        assert!(QualityFlag::Overflow.is_error());

        assert!(!QualityFlag::Outdated.is_valid());
        assert!(QualityFlag::Outdated.is_error());
    }

    // ===== T19：DigitalState is_on/is_off/is_valid 语义 =====
    #[test]
    fn test_digital_state_semantics() {
        // Off
        assert!(DigitalState::Off.is_off());
        assert!(!DigitalState::Off.is_on());
        assert!(DigitalState::Off.is_valid());
        // On
        assert!(DigitalState::On.is_on());
        assert!(!DigitalState::On.is_off());
        assert!(DigitalState::On.is_valid());
        // Intermediate
        assert!(!DigitalState::Intermediate.is_on());
        assert!(!DigitalState::Intermediate.is_off());
        assert!(!DigitalState::Intermediate.is_valid());
        // Bad
        assert!(!DigitalState::Bad.is_on());
        assert!(!DigitalState::Bad.is_off());
        assert!(!DigitalState::Bad.is_valid());
    }

    // ===== T20：ControlExecState is_terminal/is_active 语义 =====
    #[test]
    fn test_control_exec_state_semantics() {
        // Idle：非终态、非活跃
        assert!(!ControlExecState::Idle.is_terminal());
        assert!(!ControlExecState::Idle.is_active());
        // Selected：非终态、活跃
        assert!(!ControlExecState::Selected.is_terminal());
        assert!(ControlExecState::Selected.is_active());
        // Executing：非终态、活跃
        assert!(!ControlExecState::Executing.is_terminal());
        assert!(ControlExecState::Executing.is_active());
        // Done：终态、非活跃
        assert!(ControlExecState::Done.is_terminal());
        assert!(!ControlExecState::Done.is_active());
        // Failed：终态、非活跃
        assert!(ControlExecState::Failed.is_terminal());
        assert!(!ControlExecState::Failed.is_active());
        // Timeout：终态、非活跃
        assert!(ControlExecState::Timeout.is_terminal());
        assert!(!ControlExecState::Timeout.is_active());
    }
}
