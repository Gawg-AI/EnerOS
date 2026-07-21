//! EnerOS RTOS 看门狗与端到端降级流程 — Phase 1 (v0.58.0).
//!
//! 本 crate 整合 v0.13.0（分层看门狗）、v0.37.0（心跳模式参考）、v0.56.0（命令执行）、
//! v0.57.0（降级引擎），实现端到端降级流程编排，包括：
//! - [`state::DegradeState`] — 5 态状态机（Normal/Degrading/Degraded/Recovering/Emergency）
//! - [`heartbeat::HeartbeatWatcher`] — 单 Agent 心跳监控器（D2 本地轻量实现）
//! - [`recovery::RecoveryManager`] — 恢复过渡管理器（D10 纯状态，不持有 protocol）
//! - [`flow::WatchdogDegradeFlow`] — 端到端降级流程核心编排器（D6 泛型/D11 单步驱动）
//! - [`config::DegradeConfig`] — 降级流程配置（D5 u64 毫秒/D9 PointId 字段）
//! - [`stats::FlowStats`] / [`stats::FlowReport`] — 统计与报告（D4 不使用 AtomicU64）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 复用 v0.13.0 分层 `Watchdog`，不新建 `WatchdogFeeder`（`register_layer`/`feed_layer`/`check` 已满足） |
//! | **D2** | 本地轻量 `HeartbeatWatcher`（单 Agent），不复用 v0.37.0 `HeartbeatMonitor`（多 Agent + 重依赖 `eneros-agent`） |
//! | **D3** | 注入 `now_ns: u64`，拒绝 `MonotonicTime::now()`（no_std 无系统时钟，与 v0.56.0 D12 / v0.57.0 D5 一致） |
//! | **D4** | 统计计数器 `FlowStats`（普通 `u64`），拒绝 `log_info!`/`log_warn!`/`log_error!`（no_std 无日志框架） |
//! | **D5** | `u64` 毫秒/纳秒，拒绝 `Duration` 类型（与 v0.57.0 D5 一致） |
//! | **D6** | 泛型 `<P: PointAccess, S: DeviceStateProvider>`，拒绝 `Box<dyn PointAccess>`（no_std 友好） |
//! | **D7** | `cmd_executor.tick(now_ns)`，拒绝 `process_commands()`（与 v0.56.0 单步驱动一致） |
//! | **D8** | `degrade_engine.evaluate(ctx, ctx.now_ns)`，拒绝 `evaluate(context)`（与 v0.57.0 API 一致） |
//! | **D9** | `DegradeConfig` 含 `power_setpoint_point`/`power_cmd_point` 字段，拒绝硬编码 `POWER_SETPOINT_ID`/`POWER_CMD_ID` |
//! | **D10** | `RecoveryManager` 为纯状态结构（不持有 protocol），I/O 由 `WatchdogDegradeFlow` 通过 `degrade_engine.protocol_mut()` 执行 |
//! | **D11** | `tick(&mut self, ctx: &DegradeContext) -> FlowReport`，从 `ctx.now_ns` 取时间戳（单步驱动） |
//! | **D12** | Emergency 状态不自动恢复（对应蓝图风险 8.4/8.6），看门狗硬复位后由启动流程恢复 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，不 `use std::*`，不 `panic!`/`todo!`/`unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod config;
pub mod error;
pub mod flow;
pub mod heartbeat;
pub mod recovery;
pub mod state;
pub mod stats;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use eneros_controlbus::{
        command_send, control_bus_init, ConstraintPack, ControlAction, ControlCommand, DeviceId,
        DeviceState,
    };
    use eneros_protocol_abstract::PointAccess;
    use eneros_rtos_cmd_exec::device_map::DevicePointMap;
    use eneros_rtos_cmd_exec::executor::CommandExecutor;
    use eneros_rtos_degrade::context::DegradeContext;
    use eneros_rtos_degrade::engine::DegradeEngine;
    use eneros_rtos_degrade::safe_defaults::SafeDefaults;
    use eneros_upa_model::{PointId, PointValue};
    use eneros_watchdog::{HwWatchdog, Watchdog};

    use crate::config::DegradeConfig;
    use crate::flow::WatchdogDegradeFlow;
    use crate::heartbeat::{HeartbeatStatus, HeartbeatWatcher};
    use crate::mock::{MockDeviceStateProvider, MockPointAccess};
    use crate::recovery::RecoveryManager;
    use crate::state::DegradeState;

    // ===== 测试序列化锁（操作 controlbus 全局 CMD_RING）=====

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ===== controlbus 测试环（256B × 16 槽 = 4096B）=====

    static mut RING_BUF: [u8; 4096] = [0; 4096];

    #[allow(static_mut_refs)]
    fn make_test_ring() -> eneros_ipc::SpscRing {
        // SAFETY: 测试由 TEST_LOCK 序列化，同一时刻仅一个测试访问 RING_BUF。
        unsafe { eneros_ipc::SpscRing::new(&mut RING_BUF, 256, 16) }
    }

    // ===== 通用辅助 =====

    const TEST_DEVICE: DeviceId = DeviceId(1);
    const TEST_POINT: PointId = 1;
    const HEARTBEAT_PERIOD_NS: u64 = 1_000_000_000; // 1s
    const MAX_TIMEOUT: u8 = 3;

    fn make_device_map() -> DevicePointMap {
        let mut m = DevicePointMap::new();
        m.insert(TEST_DEVICE, TEST_POINT);
        m
    }

    fn make_valid_state() -> DeviceState {
        DeviceState {
            soc: 50.0,
            voltage: 300.0,
            frequency: 50.0,
            current_power: 40.0,
        }
    }

    fn make_cmd(
        action: ControlAction,
        setpoint: f32,
        ttl_ms: u32,
        timestamp: u64,
    ) -> ControlCommand {
        ControlCommand {
            cmd_id: [1; 16],
            timestamp,
            ttl_ms,
            target_device: TEST_DEVICE,
            action,
            setpoint,
            constraints: ConstraintPack {
                max_power: 80.0,
                min_power: 20.0,
                soc_limit: (10.0, 80.0),
                voltage_limit: (200.0, 400.0),
                frequency_limit: (49.0, 51.0),
            },
            signature: [0; 64],
        }
    }

    fn make_ctx(now_ns: u64) -> DegradeContext {
        DegradeContext::new().with_now_ns(now_ns)
    }

    /// 创建默认 flow（watchdog hard_timeout=10000ms，default config）.
    fn make_flow() -> WatchdogDegradeFlow<MockPointAccess, MockDeviceStateProvider> {
        let engine = DegradeEngine::new(
            MockPointAccess::new(),
            make_device_map(),
            SafeDefaults::new(),
        );
        let mut sp = MockDeviceStateProvider::new();
        sp.set_state(make_valid_state());
        let executor = CommandExecutor::new(MockPointAccess::new(), sp, make_device_map());
        let watchdog = Watchdog::new(HwWatchdog::new(0), 10000);
        WatchdogDegradeFlow::new(engine, executor, watchdog, DegradeConfig::default())
    }

    /// 喂 3 次 now_ns>1s 的 tick 使心跳 Dead（驱动 Normal→Degrading→Degraded）.
    /// 返回最终状态。
    fn drive_to_degraded(
        flow: &mut WatchdogDegradeFlow<MockPointAccess, MockDeviceStateProvider>,
    ) -> DegradeState {
        // 3 次心跳超时 tick（last_heartbeat=0，elapsed>1s → timeout_count 累加）
        let _ = flow.tick(&make_ctx(1_500_000_000)); // Timeout(1)
        let _ = flow.tick(&make_ctx(2_500_000_000)); // Timeout(2)
        let _ = flow.tick(&make_ctx(3_500_000_000)); // Dead → Degrading → Degraded
        flow.state
    }

    // ===== T1：DegradeState is_degraded =====

    #[test]
    fn test_t1_degrade_state_is_degraded() {
        assert!(!DegradeState::Normal.is_degraded());
        assert!(DegradeState::Degrading.is_degraded());
        assert!(DegradeState::Degraded.is_degraded());
        assert!(DegradeState::Recovering.is_degraded());
        assert!(DegradeState::Emergency.is_degraded());
    }

    // ===== T2：HeartbeatWatcher Alive =====

    #[test]
    fn test_t2_heartbeat_alive() {
        let mut hb = HeartbeatWatcher::new(HEARTBEAT_PERIOD_NS, MAX_TIMEOUT);
        hb.on_heartbeat(0);
        let status = hb.check(500_000_000); // 0.5s < 1s → Alive
        assert_eq!(status, HeartbeatStatus::Alive);
        assert!(hb.is_alive());
    }

    // ===== T3：HeartbeatWatcher Timeout =====

    #[test]
    fn test_t3_heartbeat_timeout() {
        let mut hb = HeartbeatWatcher::new(HEARTBEAT_PERIOD_NS, MAX_TIMEOUT);
        hb.on_heartbeat(0);
        let status = hb.check(1_500_000_000); // 1.5s > 1s → Timeout(1)
        assert_eq!(status, HeartbeatStatus::Timeout(1));
        assert!(hb.is_alive()); // 未达阈值，仍 alive
    }

    // ===== T4：HeartbeatWatcher Dead =====

    #[test]
    fn test_t4_heartbeat_dead() {
        let mut hb = HeartbeatWatcher::new(HEARTBEAT_PERIOD_NS, MAX_TIMEOUT);
        hb.on_heartbeat(0);
        let _ = hb.check(1_500_000_000); // Timeout(1)
        let _ = hb.check(2_500_000_000); // Timeout(2)
        let status = hb.check(3_500_000_000); // Dead (count=3 >= max=3)
        assert_eq!(status, HeartbeatStatus::Dead);
        assert!(!hb.is_alive());
    }

    // ===== T5：HeartbeatWatcher 恢复 =====

    #[test]
    fn test_t5_heartbeat_recovery() {
        let mut hb = HeartbeatWatcher::new(HEARTBEAT_PERIOD_NS, MAX_TIMEOUT);
        hb.on_heartbeat(0);
        let _ = hb.check(1_500_000_000);
        let _ = hb.check(2_500_000_000);
        let _ = hb.check(3_500_000_000); // Dead
        assert!(!hb.is_alive());

        // 恢复心跳
        hb.on_heartbeat(4_000_000_000);
        let status = hb.check(4_100_000_000); // 0.1s < 1s → Alive
        assert_eq!(status, HeartbeatStatus::Alive);
        assert!(hb.is_alive());
    }

    // ===== T6：RecoveryManager 线性插值 =====

    #[test]
    fn test_t6_recovery_linear_interpolation() {
        let mut rm = RecoveryManager::new(100_000_000); // 100ms 过渡
        rm.start_transition(0.0, 100.0, 0); // degraded=0, agent=100

        // 25% 进度
        let v = rm
            .transition_step(25_000_000)
            .expect("25% should return Some");
        assert!((v - 25.0).abs() < 1e-9, "expected 25.0, got {}", v);

        // 50% 进度
        let v = rm
            .transition_step(50_000_000)
            .expect("50% should return Some");
        assert!((v - 50.0).abs() < 1e-9, "expected 50.0, got {}", v);

        // 75% 进度
        let v = rm
            .transition_step(75_000_000)
            .expect("75% should return Some");
        assert!((v - 75.0).abs() < 1e-9, "expected 75.0, got {}", v);
    }

    // ===== T7：RecoveryManager is_complete + complete =====

    #[test]
    fn test_t7_recovery_complete() {
        let mut rm = RecoveryManager::new(100_000_000); // 100ms 过渡
        rm.start_transition(0.0, 100.0, 0);

        // 100% 进度 → transition_step 返回 None
        let v = rm.transition_step(100_000_000);
        assert!(v.is_none(), "100% should return None");
        assert!(rm.is_complete(), "should be complete at 100%");

        // complete() 清理过渡状态
        rm.complete();
        assert!(
            rm.transition_start_ns.is_none(),
            "start_ns should be None after complete"
        );
    }

    // ===== T8：WatchdogDegradeFlow Normal 执行 cmd_executor.tick =====

    #[test]
    fn test_t8_flow_normal_executes_cmd() {
        let _g = lock();
        control_bus_init(make_test_ring());

        // 发送一条命令
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut flow = make_flow();
        let report = flow.tick(&make_ctx(10_000_000)); // 10ms，心跳 Alive

        assert_eq!(report.state, DegradeState::Normal);
        assert_eq!(report.cmd_report.total, 1, "应消费 1 条命令");
        assert_eq!(report.cmd_report.success, 1, "应成功下发 1 条");
    }

    // ===== T9：WatchdogDegradeFlow Normal → Degrading → Degraded =====

    #[test]
    fn test_t9_flow_normal_to_degraded() {
        let _g = lock();
        let mut flow = make_flow();

        // 3 次心跳超时 tick
        let r1 = flow.tick(&make_ctx(1_500_000_000)); // Timeout(1)，仍 Normal
        assert_eq!(r1.state, DegradeState::Normal);

        let r2 = flow.tick(&make_ctx(2_500_000_000)); // Timeout(2)，仍 Normal
        assert_eq!(r2.state, DegradeState::Normal);

        let r3 = flow.tick(&make_ctx(3_500_000_000)); // Dead → Degrading → Degraded
        assert_eq!(
            r3.state,
            DegradeState::Degraded,
            "第 3 次 tick 应到 Degraded"
        );
        assert!(r3.state_changed, "应发生状态转换");
    }

    // ===== T10：WatchdogDegradeFlow Degraded → Recovering → Normal =====

    #[test]
    fn test_t10_flow_degraded_to_normal() {
        let _g = lock();

        // 使用短过渡配置（100ms）+ 区分点 ID
        let config = DegradeConfig {
            recovery_transition_ms: 100,
            power_setpoint_point: 100,
            power_cmd_point: 200,
            ..Default::default()
        };
        let engine = DegradeEngine::new(
            MockPointAccess::new(),
            make_device_map(),
            SafeDefaults::new(),
        );
        let mut sp = MockDeviceStateProvider::new();
        sp.set_state(make_valid_state());
        let executor = CommandExecutor::new(MockPointAccess::new(), sp, make_device_map());
        let watchdog = Watchdog::new(HwWatchdog::new(0), 10000);
        let mut flow = WatchdogDegradeFlow::new(engine, executor, watchdog, config);

        // 预写设定值点（100）和命令点（200）
        let _ = flow
            .degrade_engine
            .protocol_mut()
            .write_point(100, PointValue::Float(50.0));
        let _ = flow
            .degrade_engine
            .protocol_mut()
            .write_point(200, PointValue::Float(10.0));

        // 驱动到 Degraded
        drive_to_degraded(&mut flow);
        assert_eq!(flow.state, DegradeState::Degraded);

        // 心跳恢复 → tick → Recovering
        flow.heartbeat_mut().on_heartbeat(4_000_000_000);
        let r_recover = flow.tick(&make_ctx(4_500_000_000)); // Alive → Recovering
        assert_eq!(
            r_recover.state,
            DegradeState::Recovering,
            "应转为 Recovering"
        );

        // 100ms 后过渡完成 → Normal
        let r_done = flow.tick(&make_ctx(4_600_000_000)); // 过渡完成
        assert_eq!(r_done.state, DegradeState::Normal, "应回到 Normal");
        assert!(r_done.state_changed);
    }

    // ===== T11：WatchdogDegradeFlow Recovering → Degraded（恢复中再次崩溃）=====

    #[test]
    fn test_t11_flow_recovering_to_degraded() {
        let _g = lock();

        // 使用默认配置（30s 过渡，确保过渡期间不会自动完成）
        let mut flow = make_flow();

        // 驱动到 Degraded
        drive_to_degraded(&mut flow);
        assert_eq!(flow.state, DegradeState::Degraded);

        // 心跳恢复 → tick → Recovering
        flow.heartbeat_mut().on_heartbeat(4_000_000_000);
        let _ = flow.tick(&make_ctx(4_500_000_000));
        assert_eq!(flow.state, DegradeState::Recovering);

        // 恢复中再次崩溃：3 次心跳超时 tick
        let _ = flow.tick(&make_ctx(5_500_000_000)); // Timeout(1)
        assert_eq!(
            flow.state,
            DegradeState::Recovering,
            "Timeout(1) 不应离开 Recovering"
        );

        let _ = flow.tick(&make_ctx(6_500_000_000)); // Timeout(2)
        assert_eq!(
            flow.state,
            DegradeState::Recovering,
            "Timeout(2) 不应离开 Recovering"
        );

        let r = flow.tick(&make_ctx(7_500_000_000)); // Dead → Degraded
        assert_eq!(r.state, DegradeState::Degraded, "Dead 应转回 Degraded");
    }

    // ===== T12：WatchdogDegradeFlow Emergency（watchdog HardReset）=====

    #[test]
    fn test_t12_flow_emergency() {
        let _g = lock();

        // 使用 hard_timeout=0 的 watchdog（任何层超时即 HardReset）
        let engine = DegradeEngine::new(
            MockPointAccess::new(),
            make_device_map(),
            SafeDefaults::new(),
        );
        let mut sp = MockDeviceStateProvider::new();
        sp.set_state(make_valid_state());
        let executor = CommandExecutor::new(MockPointAccess::new(), sp, make_device_map());
        let watchdog = Watchdog::new(HwWatchdog::new(0), 0); // hard_timeout=0
        let mut flow =
            WatchdogDegradeFlow::new(engine, executor, watchdog, DegradeConfig::default());

        // tick at 200ms > 100ms kernel period → HardReset → Emergency
        let report = flow.tick(&make_ctx(200_000_000));
        assert_eq!(
            report.state,
            DegradeState::Emergency,
            "HardReset 应触发 Emergency"
        );
        assert_eq!(flow.stats().emergency_count, 1);
    }

    // ===== T13：WatchdogDegradeFlow 喂狗层级（Normal 3 层，Degraded 2 层）=====

    #[test]
    fn test_t13_feed_layers() {
        let _g = lock();
        let mut flow = make_flow();

        // Normal tick at 10ms → 喂 3 层
        let _ = flow.tick(&make_ctx(10_000_000));
        let fed_count_normal = flow
            .watchdog()
            .layers
            .iter()
            .flatten()
            .filter(|l| l.last_feed_ns == 10_000_000)
            .count();
        assert_eq!(fed_count_normal, 3, "Normal 应喂 3 层");

        // 驱动到 Degraded
        drive_to_degraded(&mut flow);
        assert_eq!(flow.state, DegradeState::Degraded);

        // Degraded tick at 4.5s → 喂 2 层（kernel + runtime，跳过 agent）
        let deg_time = 4_500_000_000u64;
        let _ = flow.tick(&make_ctx(deg_time));
        let fed_count_degraded = flow
            .watchdog()
            .layers
            .iter()
            .flatten()
            .filter(|l| l.last_feed_ns == deg_time)
            .count();
        assert_eq!(fed_count_degraded, 2, "Degraded 应喂 2 层");
    }

    // ===== T14：FlowStats 累加 =====

    #[test]
    fn test_t14_flow_stats_accumulation() {
        let _g = lock();
        let mut flow = make_flow();

        // Tick 1 at 10ms：Normal，心跳 Alive
        let _ = flow.tick(&make_ctx(10_000_000));
        assert_eq!(flow.stats().cmds_executed, 1);
        assert_eq!(flow.stats().heartbeat_timeouts, 0);
        assert_eq!(flow.stats().state_transitions, 0);

        // Tick 2 at 1.5s：Normal，心跳 Timeout(1)
        let _ = flow.tick(&make_ctx(1_500_000_000));
        assert_eq!(flow.stats().cmds_executed, 2);
        assert_eq!(flow.stats().heartbeat_timeouts, 1);

        // Tick 3 at 2.5s：Normal，心跳 Timeout(2)
        let _ = flow.tick(&make_ctx(2_500_000_000));
        assert_eq!(flow.stats().heartbeat_timeouts, 2);

        // Tick 4 at 3.5s：Dead → Degrading → Degraded
        let _ = flow.tick(&make_ctx(3_500_000_000));
        assert_eq!(flow.stats().heartbeat_timeouts, 3);
        // Normal→Degrading(1) + Degrading→Degraded(1) = 2 transitions
        assert_eq!(flow.stats().state_transitions, 2);
        assert_eq!(flow.stats().degrade_evaluations, 1);
        assert_eq!(flow.stats().emergency_count, 0);
        assert_eq!(flow.stats().recovery_count, 0);
    }

    // ===== T15：DegradeConfig 默认值 =====

    #[test]
    fn test_t15_config_defaults() {
        let cfg = DegradeConfig::default();
        assert_eq!(cfg.heartbeat_period_ms, 1000, "心跳周期默认 1s");
        assert_eq!(cfg.heartbeat_timeout_count, 3, "超时阈值默认 3 次");
        assert_eq!(cfg.recovery_transition_ms, 30000, "恢复过渡默认 30s");
        assert_eq!(cfg.watchdog_hard_timeout_ms, 10000, "看门狗硬复位默认 10s");
        assert_eq!(cfg.power_setpoint_point, 0, "功率设定值点默认 0");
        assert_eq!(cfg.power_cmd_point, 0, "功率命令点默认 0");
    }
}
