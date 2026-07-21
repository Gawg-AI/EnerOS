//! EnerOS RTOS 控制闭环引擎 — Phase 1 P1-H (v0.54.0).
//!
//! 本 crate 实现 RTOS 控制大区的核心控制闭环引擎，包括：
//! - [`pid::PidController`] — PID 控制器（积分/输出限幅）
//! - [`setpoint::SetpointTracker`] — 设定值斜率限制跟踪器
//! - [`loop_trait::ControlLoop`] — 控制循环 trait
//! - [`engine::ControlLoopEngine`] — 多循环调度引擎（tick 单步驱动）
//! - [`power_loop::PowerControlLoop`] — 功率控制循环示例
//!
//! # 偏差声明（D1~D12）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 时间戳用 `u64` 微秒参数注入（蓝图 `MonotonicTime`/`Duration` 在 no_std 不存在） |
//! | **D2** | crate 放入 `crates/kernel/rtos-control/`（P1-H RTOS 组件，与 controlbus/sched 同属 kernel 子系统） |
//! | **D3** | 不实现阻塞式 `run() -> !`（改为 `tick(now_us, elapsed_us) -> EngineTickReport` 单步驱动） |
//! | **D4** | 不直接依赖 `eneros-time` 的 `Hrtimer`/`MonotonicClock`（时间触发由 v0.19.0 分区调度器负责） |
//! | **D5** | 不要求 `ControlLoop: Send + Sync`（no_std 单线程无需） |
//! | **D6** | 不使用 `Box<dyn PointAccess>`（改为泛型 `<P: PointAccess>`） |
//! | **D7** | 不使用 `ControlBusReader`（直接调用 `command_consume()` 全局函数） |
//! | **D8** | `EngineStats` 不使用 `AtomicU64`（no_std 单线程无需） |
//! | **D9** | `JitterRecord` 不使用 `BTreeMap<&str, u64>`（用 `Vec<(String, JitterStats)>`） |
//! | **D10** | `PidController` 的 `clamp` 用 `core::cmp::min/max` 手写实现 |
//! | **D11** | `SetpointTracker::update` 接受 `(target, dt)` 两参数（蓝图 `set_setpoint` 未考虑 dt） |
//! | **D12** | 不实现 `PowerControlLoop::shutdown` 复杂清理（仅 `pid.reset()`） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，不 `use std::*`，不 `panic!`/`todo!`/`unimplemented!`。

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

extern crate alloc;

pub mod engine;
pub mod error;
pub mod loop_trait;
pub mod pid;
pub mod power_loop;
pub mod setpoint;
pub mod stats;

#[cfg(test)]
pub mod mock;

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_controlbus::{command_consume, command_send, control_bus_init, ControlCommand};
    use eneros_ipc::SpscRing;
    use eneros_protocol_abstract::PointAccess;
    use eneros_upa_model::PointValue;

    use super::*;
    use crate::engine::ControlLoopEngine;
    use crate::error::ControlError;
    use crate::loop_trait::ControlLoop;
    use crate::mock::MockPointAccess;
    use crate::pid::PidController;
    use crate::power_loop::PowerControlLoop;
    use crate::setpoint::SetpointTracker;

    // ===== 测试辅助 =====

    /// 控制总线测试序列化锁（防止并发测试干扰全局 CMD_RING）.
    static BUS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 静态环缓冲区（生命周期与进程一致，避免悬垂指针）.
    static mut RING_BUF: [u8; 4096] = [0; 4096];

    /// 创建测试用 SpscRing（slot_size=256, 16 slots）.
    #[allow(static_mut_refs)]
    fn make_test_ring() -> SpscRing {
        unsafe { SpscRing::new(&mut RING_BUF, 256, 16) }
    }

    /// 浮点近似比较.
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// 模拟控制循环（引擎测试用）.
    struct MockLoop {
        name: &'static str,
        period: u64,
        exec_count: u32,
        should_fail: bool,
    }

    impl ControlLoop for MockLoop {
        fn name(&self) -> &str {
            self.name
        }
        fn period_us(&self) -> u64 {
            self.period
        }
        fn init(&mut self) -> Result<(), ControlError> {
            Ok(())
        }
        fn execute(&mut self, _elapsed_us: u64) -> Result<(), ControlError> {
            self.exec_count += 1;
            if self.should_fail {
                Err(ControlError::LoopPanic)
            } else {
                Ok(())
            }
        }
        fn shutdown(&mut self) {}
    }

    // ===== T1：PidController 阶跃响应 =====
    #[test]
    fn test_t1_pid_step_response() {
        let mut pid = PidController::new(1.0, 0.1, 0.01);
        pid.set_setpoint(100.0);
        pid.set_process_variable(80.0);
        let output = pid.compute(0.01);
        // error=20, integral=0.2, derivative=2000
        // output = 1.0*20 + 0.1*0.2 + 0.01*2000 = 40.02
        assert!(approx_eq(output, 40.02), "expected 40.02, got {}", output);
    }

    // ===== T2：积分限幅 =====
    #[test]
    fn test_t2_integral_limit() {
        let mut pid = PidController::new(0.0, 1.0, 0.0);
        pid.set_integral_limit(10.0);
        pid.set_setpoint(100.0);
        pid.set_process_variable(0.0);
        // dt=0.1: integral += 100*0.1 = 10 each step
        pid.compute(0.1); // integral = 10
        pid.compute(0.1); // integral = 20 → clamped to 10
        assert!(
            approx_eq(pid.integral(), 10.0),
            "expected 10.0, got {}",
            pid.integral()
        );
    }

    // ===== T3：输出限幅 =====
    #[test]
    fn test_t3_output_limit() {
        let mut pid = PidController::new(10.0, 0.0, 0.0);
        pid.set_output_limit(50.0);
        pid.set_setpoint(100.0);
        pid.set_process_variable(0.0);
        // error=100, output = 10*100 = 1000 → clamped to 50
        let output = pid.compute(0.01);
        assert!(approx_eq(output, 50.0), "expected 50.0, got {}", output);
    }

    // ===== T4：reset 清零 =====
    #[test]
    fn test_t4_reset() {
        let mut pid = PidController::new(1.0, 1.0, 1.0);
        pid.set_setpoint(100.0);
        pid.set_process_variable(0.0);
        pid.compute(0.1);
        assert!(pid.integral() != 0.0);
        assert!(pid.last_error() != 0.0);
        pid.reset();
        assert!(approx_eq(pid.integral(), 0.0));
        assert!(approx_eq(pid.last_error(), 0.0));
    }

    // ===== T5：dt=0 不 panic =====
    #[test]
    fn test_t5_dt_zero_no_panic() {
        let mut pid = PidController::new(1.0, 0.1, 0.01);
        pid.set_setpoint(100.0);
        pid.set_process_variable(80.0);
        let output = pid.compute(0.0);
        // dt=0: derivative=0, integral += 0, output = 1.0*20 = 20
        assert!(approx_eq(output, 20.0), "expected 20.0, got {}", output);
    }

    // ===== T6：SetpointTracker 斜率限制 =====
    #[test]
    fn test_t6_setpoint_rate_limit() {
        let mut tracker = SetpointTracker::new(50.0, 10.0);
        tracker.set_target(100.0);
        let current = tracker.update(0.01);
        // max_step = 10*0.01 = 0.1, current = 50 + 0.1 = 50.1
        assert!(approx_eq(current, 50.1), "expected 50.1, got {}", current);
    }

    // ===== T7：SetpointTracker 收敛 =====
    #[test]
    fn test_t7_setpoint_converge() {
        let mut tracker = SetpointTracker::new(50.0, 10.0);
        tracker.set_target(100.0);
        for _ in 0..500 {
            tracker.update(0.01);
        }
        assert!(approx_eq(tracker.current(), 100.0));
        assert!(tracker.is_settled());
    }

    // ===== T8：SetpointTracker 无限制 =====
    #[test]
    fn test_t8_setpoint_unlimited() {
        let mut tracker = SetpointTracker::new(50.0, f64::MAX);
        tracker.set_target(100.0);
        let current = tracker.update(0.01);
        assert!(approx_eq(current, 100.0));
        assert!(tracker.is_settled());
    }

    // ===== T9：SetpointTracker 负方向 =====
    #[test]
    fn test_t9_setpoint_negative_direction() {
        let mut tracker = SetpointTracker::new(100.0, 10.0);
        tracker.set_target(50.0);
        for _ in 0..500 {
            tracker.update(0.01);
        }
        assert!(approx_eq(tracker.current(), 50.0));
        assert!(tracker.is_settled());
    }

    // ===== T10：ControlLoopEngine 注册 + tick =====
    #[test]
    fn test_t10_engine_register_and_tick() {
        let mut engine = ControlLoopEngine::new();
        let ctrl = Box::new(MockLoop {
            name: "test",
            period: 10_000,
            exec_count: 0,
            should_fail: false,
        });
        engine.register(ctrl);
        let report = engine.tick(15_000, 10_000);
        assert_eq!(report.executed_loops, 1);
        assert_eq!(report.errors, 0);
    }

    // ===== T11：多循环不同周期 =====
    #[test]
    fn test_t11_multi_loop_different_periods() {
        let mut engine = ControlLoopEngine::new();
        let ctrl_a = Box::new(MockLoop {
            name: "A",
            period: 10_000,
            exec_count: 0,
            should_fail: false,
        });
        let ctrl_b = Box::new(MockLoop {
            name: "B",
            period: 20_000,
            exec_count: 0,
            should_fail: false,
        });
        engine.register(ctrl_a);
        engine.register(ctrl_b);

        // tick 10000: A runs (10000-0>=10000), B doesn't (10000<20000)
        let r = engine.tick(10_000, 10_000);
        assert_eq!(r.executed_loops, 1);

        // tick 20000: A runs, B runs (20000-0>=20000)
        let r = engine.tick(20_000, 10_000);
        assert_eq!(r.executed_loops, 2);

        // tick 30000: A runs (30000-20000>=10000), B doesn't (30000-20000<20000)
        let r = engine.tick(30_000, 10_000);
        assert_eq!(r.executed_loops, 1); // 仅 A

        // 验证统计：A 执行 3 次，B 执行 1 次
        let stats = engine.stats();
        let a = stats.get("A").expect("A stats");
        let b = stats.get("B").expect("B stats");
        assert_eq!(a.exec_count, 3);
        assert_eq!(b.exec_count, 1);
    }

    // ===== T12：错误隔离 =====
    #[test]
    fn test_t12_error_isolation() {
        let mut engine = ControlLoopEngine::new();
        let ctrl_ok = Box::new(MockLoop {
            name: "ok",
            period: 10_000,
            exec_count: 0,
            should_fail: false,
        });
        let ctrl_err = Box::new(MockLoop {
            name: "err",
            period: 10_000,
            exec_count: 0,
            should_fail: true,
        });
        engine.register(ctrl_ok);
        engine.register(ctrl_err);

        let r = engine.tick(15_000, 10_000);
        assert_eq!(r.executed_loops, 2);
        assert_eq!(r.errors, 1);

        // 验证 ok 循环也执行了
        let stats = engine.stats();
        let ok_stats = stats.get("ok").expect("ok stats");
        assert_eq!(ok_stats.exec_count, 1);
        assert_eq!(ok_stats.error_count, 0);

        let err_stats = stats.get("err").expect("err stats");
        assert_eq!(err_stats.exec_count, 1);
        assert_eq!(err_stats.error_count, 1);
    }

    // ===== T13：抖动统计 =====
    #[test]
    fn test_t13_jitter_stats() {
        let mut engine = ControlLoopEngine::new();
        let ctrl = Box::new(MockLoop {
            name: "jitter_test",
            period: 10_000,
            exec_count: 0,
            should_fail: false,
        });
        engine.register(ctrl);

        // 首次 tick：jitter = 0（首次）
        engine.tick(15_000, 10_000);

        // 第二次 tick：elapsed=15000, period=10000, jitter = 15000-10000 = 5000
        engine.tick(30_000, 15_000);

        let stats = engine.stats();
        let js = stats.get("jitter_test").expect("stats exist");
        assert_eq!(js.exec_count, 2);
        assert_eq!(js.last_jitter_us, 5000);
        assert_eq!(js.max_jitter_us, 5000);
        assert_eq!(js.total_jitter_us, 5000);
    }

    // ===== T14：PowerControlLoop 完整链路 =====
    #[test]
    fn test_t14_power_loop_full_chain() {
        let _g = BUS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ring = make_test_ring();
        control_bus_init(ring);

        // 发送设定值=100.0 的命令
        let cmd = ControlCommand {
            setpoint: 100.0,
            ..Default::default()
        };
        command_send(&cmd).expect("send ok");

        // 创建 mock 协议：反馈点=1（值 80.0），输出点=2（值 0.0）
        let mut protocol = MockPointAccess::new();
        protocol.set_point(1, 80.0);
        protocol.set_point(2, 0.0);

        let pid = PidController::new(1.0, 0.1, 0.01);
        let tracker = SetpointTracker::new(0.0, f64::MAX);
        let mut loop_ = PowerControlLoop::new(pid, tracker, 1, 2, protocol, "power");

        loop_.execute(10_000).expect("execute ok");

        // 验证输出点被写入（非零）
        let output_data = loop_.protocol_mut().read_point(2).expect("read output");
        assert!(
            matches!(output_data.value, PointValue::Float(_)),
            "expected float output"
        );
        if let PointValue::Float(v) = output_data.value {
            // setpoint=100, pv=80, dt=0.01
            // output = 1.0*20 + 0.1*0.2 + 0.01*2000 = 40.02
            assert!(v.abs() > 0.01, "output should be non-zero, got {}", v);
        }
    }

    // ===== T15：无命令保持 =====
    #[test]
    fn test_t15_no_command_holds() {
        let _g = BUS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ring = make_test_ring();
        control_bus_init(ring);

        // 不发送任何命令

        // 创建 mock 协议：反馈点=1（值 50.0），输出点=2（值 0.0）
        let mut protocol = MockPointAccess::new();
        protocol.set_point(1, 50.0);
        protocol.set_point(2, 0.0);

        let pid = PidController::new(1.0, 0.1, 0.01);
        let tracker = SetpointTracker::new(50.0, 10.0); // settled at 50
        let mut loop_ = PowerControlLoop::new(pid, tracker, 1, 2, protocol, "power");

        loop_.execute(10_000).expect("execute ok");

        // 无命令时 tracker 保持 50（current == target == 50，settled）
        assert!(
            approx_eq(loop_.current_setpoint(), 50.0),
            "expected 50.0, got {}",
            loop_.current_setpoint()
        );

        // 验证 command_consume 返回 None
        assert!(command_consume().is_none());
    }

    // ===== T16：PID 反馈收敛 =====
    #[test]
    fn test_t16_pid_feedback_convergence() {
        let _g = BUS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ring = make_test_ring();
        control_bus_init(ring);

        // 发送设定值=100.0
        let cmd = ControlCommand {
            setpoint: 100.0,
            ..Default::default()
        };
        command_send(&cmd).expect("send ok");

        // 创建 mock 协议
        let mut protocol = MockPointAccess::new();
        let mut feedback = 0.0;
        protocol.set_point(1, feedback);
        protocol.set_point(2, 0.0);

        let pid = PidController::new(1.0, 0.5, 0.0);
        let tracker = SetpointTracker::new(0.0, f64::MAX);
        let mut loop_ = PowerControlLoop::new(pid, tracker, 1, 2, protocol, "power");

        let mut last_output = f64::MAX;
        let mut converged = false;

        for i in 0..200 {
            // 更新反馈点（模拟一阶 plant：feedback += output * gain）
            loop_.protocol_mut().set_point(1, feedback);

            loop_.execute(10_000).expect("execute");

            // 读输出
            let out_data = loop_.protocol_mut().read_point(2).expect("read output");
            if let PointValue::Float(v) = out_data.value {
                // plant：feedback 向设定值方向移动
                feedback += v * 0.005;

                // 检查输出是否趋稳（连续两次输出差值很小）
                if i > 10 && (v - last_output).abs() < 0.1 {
                    converged = true;
                }
                last_output = v;
            }
        }

        // 反馈应趋近设定值
        assert!(
            (feedback - 100.0).abs() < 20.0,
            "feedback should approach 100, got {}",
            feedback
        );
        assert!(converged, "output should converge");
    }
}
