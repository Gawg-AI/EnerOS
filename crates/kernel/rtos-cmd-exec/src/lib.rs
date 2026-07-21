//! EnerOS RTOS 命令消费与执行 — Phase 1 (v0.56.0).
//!
//! 本 crate 实现 RTOS 控制大区的命令消费与执行，包括：
//! - [`error::ExecutorError`] — 执行器错误
//! - [`state_provider::DeviceStateProvider`] — 设备状态来源抽象（D3）
//! - [`device_map::DevicePointMap`] — controlbus::DeviceId→PointId 映射（D4）
//! - [`stats::ExecutorStats`] / [`stats::ExecutorReport`] — 累计统计与单次报告（D7）
//! - [`executor::CommandExecutor`] — 命令执行器（泛型 `<P, S>`，单步 `tick(now_ns)`）
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 复用 v0.22.0 的 `ttl_check`/`constraint_check`/`command_consume`（不重新实现 TtlChecker/ConstraintChecker） |
//! | **D2** | 使用全局 `command_consume()`（蓝图 ControlBusReader 不存在） |
//! | **D3** | `DeviceStateProvider` trait 抽象设备状态来源（蓝图未定义） |
//! | **D4** | `DevicePointMap` 做 `controlbus::DeviceId`→`upa_model::PointId` 映射（蓝图 `cmd.to_point_writes()` 不存在） |
//! | **D5** | 单步 `tick(now_ns)`（蓝图 `process_commands()` 阻塞，匹配 v0.54.0/v0.55.0） |
//! | **D6** | 泛型 `<P: PointAccess, S: DeviceStateProvider>`（蓝图 `Box<dyn>`） |
//! | **D7** | 不使用 `log_warn!`/`log_error!`（no_std，用 stats 计数） |
//! | **D8** | Emergency 旁路：跳过 TTL + 约束，直接下发 0.0 |
//! | **D9** | Idle 下发 0.0（setpoint 被忽略） |
//! | **D10** | setpoint f32→f64→`PointValue::Float` 转换 |
//! | **D11** | crate 放入 `crates/kernel/rtos-cmd-exec/` |
//! | **D12** | `now_ns: u64` 注入（蓝图 `MonotonicTime::now()` 不存在） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，不 `use std::*`，不 `panic!`/`todo!`/`unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod device_map;
pub mod error;
pub mod executor;
pub mod state_provider;
pub mod stats;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use eneros_controlbus::{
        command_send, control_bus_init, ConstraintPack, ControlAction, ControlCommand, DeviceId,
        DeviceState,
    };
    use eneros_upa_model::{PointId, PointValue};

    use crate::device_map::DevicePointMap;
    use crate::executor::CommandExecutor;
    use crate::mock::{MockDeviceStateProvider, MockPointAccess};

    /// 测试序列化锁（操作 controlbus 全局 CMD_RING，外部 crate 无法访问 controlbus 的 TEST_LOCK）.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 测试用环缓冲区（4096 字节，16 槽 × 256 字节）.
    static mut RING_BUF: [u8; 4096] = [0; 4096];

    #[allow(static_mut_refs)]
    fn make_test_ring() -> eneros_ipc::SpscRing {
        // SAFETY: 测试由 TEST_LOCK 序列化，同一时刻仅一个测试访问 RING_BUF。
        // 每次 new 重置 head/tail 为 0，提供空环。
        unsafe { eneros_ipc::SpscRing::new(&mut RING_BUF, 256, 16) }
    }

    const TEST_DEVICE: DeviceId = DeviceId(1);
    const TEST_POINT: PointId = 1;

    /// 构造测试命令（max_power=80, min_power=20, soc_limit=(10,80) 等）.
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

    /// 构造含 TEST_DEVICE→TEST_POINT 的映射.
    fn make_default_map() -> DevicePointMap {
        let mut m = DevicePointMap::new();
        m.insert(TEST_DEVICE, TEST_POINT);
        m
    }

    /// 构造通过约束检查的设备状态.
    fn make_valid_state() -> DeviceState {
        DeviceState {
            soc: 50.0,
            voltage: 300.0,
            frequency: 50.0,
            current_power: 40.0,
        }
    }

    /// 构造含有效状态的 state provider.
    fn make_valid_state_provider() -> MockDeviceStateProvider {
        let mut sp = MockDeviceStateProvider::new();
        sp.set_state(TEST_DEVICE, make_valid_state());
        sp
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

    // ===== T1：正常命令下发 =====
    #[test]
    fn test_t1_normal_command() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.success, 1);
        assert_eq!(report.expired, 0);
        let v = exec
            .protocol()
            .last_write(TEST_POINT)
            .expect("write happened");
        assert_float_value(v, 50.0);
    }

    // ===== T2：TTL 过期 =====
    #[test]
    fn test_t2_ttl_expired() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(200_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.expired, 1);
        assert_eq!(report.success, 0);
    }

    // ===== T3：截断下发 =====
    #[test]
    fn test_t3_truncated() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 100.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.truncated, 1);
        assert_eq!(report.success, 1);
        let v = exec
            .protocol()
            .last_write(TEST_POINT)
            .expect("write happened");
        assert_float_value(v, 80.0);
    }

    // ===== T4：约束拒绝（soc=90 超出 10~80）=====
    #[test]
    fn test_t4_rejected() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut sp = MockDeviceStateProvider::new();
        sp.set_state(
            TEST_DEVICE,
            DeviceState {
                soc: 90.0,
                ..make_valid_state()
            },
        );

        let mut exec = CommandExecutor::new(MockPointAccess::new(), sp, make_default_map());
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.rejected, 1);
        assert_eq!(report.success, 0);
    }

    // ===== T5：Emergency 旁路（TTL 过期仍下发 0.0）=====
    #[test]
    fn test_t5_emergency_bypass() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Emergency, 0.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            MockDeviceStateProvider::new(),
            make_default_map(),
        );
        let report = exec.tick(200_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.success, 1);
        assert_eq!(report.expired, 0);
        let v = exec
            .protocol()
            .last_write(TEST_POINT)
            .expect("write happened");
        assert_float_value(v, 0.0);
    }

    // ===== T6：Idle 下发 0.0（setpoint 被忽略）=====
    #[test]
    fn test_t6_idle_writes_zero() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Idle, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.success, 1);
        let v = exec
            .protocol()
            .last_write(TEST_POINT)
            .expect("write happened");
        assert_float_value(v, 0.0);
    }

    // ===== T7：写入失败 =====
    #[test]
    fn test_t7_write_failure() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut protocol = MockPointAccess::new();
        protocol.fail_next_write(TEST_POINT);

        let mut exec =
            CommandExecutor::new(protocol, make_valid_state_provider(), make_default_map());
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.success, 0);
    }

    // ===== T8：未映射设备 =====
    #[test]
    fn test_t8_unmapped_device() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let empty_map = DevicePointMap::new();
        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            empty_map,
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 1);
        assert_eq!(report.unmapped, 1);
        assert_eq!(report.success, 0);
    }

    // ===== T9：多命令批量消费 =====
    #[test]
    fn test_t9_multiple_commands() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));
        assert_eq!(command_send(&cmd), Ok(()));
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 3);
        assert_eq!(report.success, 3);
    }

    // ===== T10：空队列 =====
    #[test]
    fn test_t10_empty_queue() {
        let _g = lock();
        control_bus_init(make_test_ring());

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let report = exec.tick(10_000_000);

        assert_eq!(report.total, 0);
        assert_eq!(report.success, 0);
        assert_eq!(report.expired, 0);
        assert_eq!(report.failed, 0);
    }

    // ===== T11：DevicePointMap 增查 =====
    #[test]
    fn test_t11_device_point_map() {
        let mut m = DevicePointMap::new();
        assert_eq!(m.get(DeviceId(1)), None);
        m.insert(DeviceId(1), 100);
        assert_eq!(m.get(DeviceId(1)), Some(100));
        assert_eq!(m.get(DeviceId(2)), None);
    }

    // ===== T12：ExecutorStats 跨 tick 累加 =====
    #[test]
    fn test_t12_stats_accumulation() {
        let _g = lock();
        control_bus_init(make_test_ring());
        let cmd = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd), Ok(()));

        let mut exec = CommandExecutor::new(
            MockPointAccess::new(),
            make_valid_state_provider(),
            make_default_map(),
        );
        let r1 = exec.tick(10_000_000);
        assert_eq!(r1.total, 1);
        assert_eq!(exec.stats().total_executed, 1);
        assert_eq!(exec.stats().success_count, 1);

        // 第二次 tick：重新初始化环并再发一条命令
        control_bus_init(make_test_ring());
        let cmd2 = make_cmd(ControlAction::Charge, 50.0, 100, 0);
        assert_eq!(command_send(&cmd2), Ok(()));
        let r2 = exec.tick(10_000_000);
        assert_eq!(r2.total, 1);
        assert_eq!(exec.stats().total_executed, 2);
        assert_eq!(exec.stats().success_count, 2);
    }
}
