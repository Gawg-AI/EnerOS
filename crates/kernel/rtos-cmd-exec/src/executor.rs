//! CommandExecutor — RTOS 命令消费与执行（v0.56.0）.

use eneros_controlbus::{
    command_consume, constraint_check, ttl_check, ConstraintResult, ControlAction, DeviceId,
    TtlStatus,
};
use eneros_protocol_abstract::PointAccess;
use eneros_upa_model::PointValue;

use crate::device_map::DevicePointMap;
use crate::state_provider::DeviceStateProvider;
use crate::stats::{ExecutorReport, ExecutorStats};

/// RTOS 命令执行器.
///
/// 泛型 `<P: PointAccess, S: DeviceStateProvider>`（D6），单步 `tick(now_ns)`
/// 消费 controlbus 全局命令环中的所有命令（D2/D5），逐条执行 TTL 检查（D1）、
/// 约束检查（D1）与协议下发.
pub struct CommandExecutor<P: PointAccess, S: DeviceStateProvider> {
    protocol: P,
    state_provider: S,
    device_map: DevicePointMap,
    stats: ExecutorStats,
}

impl<P: PointAccess, S: DeviceStateProvider> CommandExecutor<P, S> {
    /// 创建命令执行器.
    pub fn new(protocol: P, state_provider: S, device_map: DevicePointMap) -> Self {
        Self {
            protocol,
            state_provider,
            device_map,
            stats: ExecutorStats::default(),
        }
    }

    /// 单步执行：消费全局命令环中所有命令并下发.
    ///
    /// 返回本次 tick 的执行报告 [`ExecutorReport`]；内部统计 [`ExecutorStats`]
    /// 跨 tick 累加（D7）.
    pub fn tick(&mut self, now_ns: u64) -> ExecutorReport {
        let mut report = ExecutorReport::default();
        while let Some(cmd) = command_consume() {
            report.total += 1;
            self.stats.total_executed += 1;

            // D8: Emergency 旁路 — 跳过 TTL + 约束，直接下发 0.0
            if cmd.action == ControlAction::Emergency {
                self.write_to_device(cmd.target_device, 0.0, &mut report);
                continue;
            }

            // D1: TTL 检查（复用 v0.22.0 ttl_check）
            if ttl_check(&cmd, now_ns) == TtlStatus::Expired {
                report.expired += 1;
                self.stats.expired_count += 1;
                continue;
            }

            // D1: 约束检查（复用 v0.22.0 constraint_check）
            let state = self.state_provider.device_state(cmd.target_device);
            match constraint_check(&cmd, &state) {
                ConstraintResult::Ok => {
                    // D9: Idle 下发 0.0；D10: setpoint f32→f64→PointValue::Float
                    let value = if cmd.action == ControlAction::Idle {
                        0.0
                    } else {
                        f64::from(cmd.setpoint)
                    };
                    self.write_to_device(cmd.target_device, value, &mut report);
                }
                ConstraintResult::Truncated(safe) => {
                    report.truncated += 1;
                    self.stats.truncated_count += 1;
                    let value = if cmd.action == ControlAction::Idle {
                        0.0
                    } else {
                        f64::from(safe)
                    };
                    self.write_to_device(cmd.target_device, value, &mut report);
                }
                ConstraintResult::Rejected => {
                    report.rejected += 1;
                    self.stats.rejected_count += 1;
                }
            }
        }
        report
    }

    /// 写入设备对应点（D4: DevicePointMap 映射 DeviceId→PointId）.
    fn write_to_device(&mut self, device: DeviceId, value: f64, report: &mut ExecutorReport) {
        if let Some(point_id) = self.device_map.get(device) {
            match self
                .protocol
                .write_point(point_id, PointValue::Float(value))
            {
                Ok(()) => {
                    report.success += 1;
                    self.stats.success_count += 1;
                }
                Err(_) => {
                    report.failed += 1;
                    self.stats.failure_count += 1;
                }
            }
        } else {
            report.unmapped += 1;
            self.stats.unmapped_count += 1;
        }
    }

    /// 返回累计统计（只读引用）.
    pub fn stats(&self) -> &ExecutorStats {
        &self.stats
    }

    /// 返回协议访问层引用（用于写入结果检查）.
    pub fn protocol(&self) -> &P {
        &self.protocol
    }
}
