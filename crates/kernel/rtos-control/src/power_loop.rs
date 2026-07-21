//! 功率控制循环 — PID + 设定值跟踪 + 协议读写.
//!
//! [`PowerControlLoop`] 是 [`crate::loop_trait::ControlLoop`] 的示例实现，
//! 将 PID 控制器、设定值跟踪器与协议抽象层组合为完整的功率控制闭环.

use eneros_controlbus::command_consume;
use eneros_protocol_abstract::PointAccess;
use eneros_upa_model::{PointId, PointValue};

use crate::error::ControlError;
use crate::loop_trait::ControlLoop;
use crate::pid::PidController;
use crate::setpoint::SetpointTracker;

/// 功率控制循环（泛型，D6：不使用 `Box<dyn PointAccess>`）.
///
/// 10ms 周期（10_000 μs），从控制总线消费命令、跟踪设定值、读反馈、
/// 计算 PID 输出、写执行机构.
pub struct PowerControlLoop<P: PointAccess> {
    /// PID 控制器.
    pid: PidController,
    /// 设定值跟踪器.
    setpoint_tracker: SetpointTracker,
    /// 反馈点 ID.
    feedback_point_id: PointId,
    /// 输出点 ID.
    output_point_id: PointId,
    /// 当前设定值（来自 tracker）.
    current_setpoint: f64,
    /// 协议访问接口.
    protocol: P,
    /// 循环名称.
    name: &'static str,
}

impl<P: PointAccess> PowerControlLoop<P> {
    /// 创建功率控制循环.
    pub fn new(
        pid: PidController,
        tracker: SetpointTracker,
        feedback_pid: PointId,
        output_pid: PointId,
        protocol: P,
        name: &'static str,
    ) -> Self {
        Self {
            pid,
            setpoint_tracker: tracker,
            feedback_point_id: feedback_pid,
            output_point_id: output_pid,
            current_setpoint: 0.0,
            protocol,
            name,
        }
    }

    /// 获取当前设定值（测试用）.
    pub fn current_setpoint(&self) -> f64 {
        self.current_setpoint
    }

    /// 获取协议引用（测试用）.
    pub fn protocol(&self) -> &P {
        &self.protocol
    }

    /// 获取协议可变引用（测试用）.
    pub fn protocol_mut(&mut self) -> &mut P {
        &mut self.protocol
    }
}

impl<P: PointAccess> ControlLoop for PowerControlLoop<P> {
    fn name(&self) -> &str {
        self.name
    }

    fn period_us(&self) -> u64 {
        10_000
    }

    fn init(&mut self) -> Result<(), ControlError> {
        Ok(())
    }

    fn execute(&mut self, elapsed_us: u64) -> Result<(), ControlError> {
        let dt = elapsed_us as f64 / 1_000_000.0;

        // 1. 消费控制命令（D7：直接调全局函数）
        if let Some(cmd) = command_consume() {
            self.setpoint_tracker.set_target(cmd.setpoint as f64);
        }

        // 2. 设定值跟踪
        let tracked = self.setpoint_tracker.update(dt);
        self.current_setpoint = tracked;

        // 3. 设置 PID 设定值
        self.pid.set_setpoint(tracked);

        // 4. 读反馈点
        let feedback = self
            .protocol
            .read_point(self.feedback_point_id)
            .map_err(|_| ControlError::FeedbackReadFailed)?;
        if let PointValue::Float(v) = feedback.value {
            self.pid.set_process_variable(v);
        } else {
            return Err(ControlError::FeedbackReadFailed);
        }

        // 5. 计算 PID 输出
        let output = self.pid.compute(dt);

        // 6. 写输出点
        self.protocol
            .write_point(self.output_point_id, PointValue::Float(output))
            .map_err(|_| ControlError::OutputWriteFailed)?;

        Ok(())
    }

    fn shutdown(&mut self) {
        // D12：仅 pid.reset()，不做复杂清理
        self.pid.reset();
    }
}
