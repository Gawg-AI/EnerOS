//! PID 控制器 — 比例-积分-微分算法实现.
//!
//! [`PidController`] 实现标准 PID 算法，支持积分限幅（anti-windup）和
//! 输出限幅。时间步长 `dt` 由调用方注入（D1：无 `MonotonicTime` 类型）.

/// PID 控制器（9 字段）.
///
/// 积分限幅 `integral_limit` 限制积分累加器的绝对值，防止积分饱和；
/// 输出限幅 `output_limit` 限制最终输出的绝对值，保护执行机构.
///
/// # 偏差 D10
///
/// clamp 用手写 `if-else` 实现（`f64` 不实现 `Ord`，无法用 `core::cmp::min/max`）.
pub struct PidController {
    /// 比例增益.
    pub kp: f64,
    /// 积分增益.
    pub ki: f64,
    /// 微分增益.
    pub kd: f64,
    /// 积分累加器.
    integral: f64,
    /// 上一次误差（用于微分计算）.
    last_error: f64,
    /// 积分限幅（绝对值上限）.
    integral_limit: f64,
    /// 输出限幅（绝对值上限）.
    output_limit: f64,
    /// 设定值（目标值）.
    setpoint: f64,
    /// 过程变量（反馈值）.
    process_variable: f64,
}

impl PidController {
    /// 创建 PID 控制器.
    ///
    /// 积分限幅和输出限幅默认为 `f64::MAX`（无限幅）.
    pub fn new(kp: f64, ki: f64, kd: f64) -> Self {
        Self {
            kp,
            ki,
            kd,
            integral: 0.0,
            last_error: 0.0,
            integral_limit: f64::MAX,
            output_limit: f64::MAX,
            setpoint: 0.0,
            process_variable: 0.0,
        }
    }

    /// 计算 PID 输出.
    ///
    /// 算法步骤：
    /// 1. 误差 = 设定值 - 过程变量
    /// 2. 积分累加 += 误差 * dt，然后限幅（D10 手写 clamp）
    /// 3. 微分 = (误差 - 上次误差) / dt（dt=0 时微分项为 0）
    /// 4. 输出 = kp*误差 + ki*积分 + kd*微分，然后限幅
    /// 5. 更新 last_error
    pub fn compute(&mut self, dt: f64) -> f64 {
        let error = self.setpoint - self.process_variable;

        // 积分累加 + 限幅（D10：手写 clamp，f64 不实现 Ord）
        self.integral += error * dt;
        self.integral = clamp_f64(self.integral, self.integral_limit);

        // 微分项（dt=0 时返回 0，避免除零）
        let derivative = if dt == 0.0 {
            0.0
        } else {
            (error - self.last_error) / dt
        };

        // PID 输出 + 限幅
        let output = self.kp * error + self.ki * self.integral + self.kd * derivative;
        let output = clamp_f64(output, self.output_limit);

        self.last_error = error;
        output
    }

    /// 设置设定值.
    pub fn set_setpoint(&mut self, sp: f64) {
        self.setpoint = sp;
    }

    /// 设置过程变量（反馈值）.
    pub fn set_process_variable(&mut self, pv: f64) {
        self.process_variable = pv;
    }

    /// 重置控制器状态（积分清零、上次误差清零）.
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.last_error = 0.0;
    }

    /// 设置积分限幅（在线调参）.
    pub fn set_integral_limit(&mut self, l: f64) {
        self.integral_limit = l;
    }

    /// 设置输出限幅（在线调参）.
    pub fn set_output_limit(&mut self, l: f64) {
        self.output_limit = l;
    }

    /// 获取当前积分累加值（测试用）.
    pub fn integral(&self) -> f64 {
        self.integral
    }

    /// 获取上次误差（测试用）.
    pub fn last_error(&self) -> f64 {
        self.last_error
    }
}

/// 手写 clamp（D10：f64 不实现 Ord，无法用 `core::cmp::min/max`）.
///
/// 将 `v` 限制在 `[-limit, +limit]` 范围内.
#[allow(clippy::manual_clamp)]
fn clamp_f64(v: f64, limit: f64) -> f64 {
    if v > limit {
        limit
    } else if v < -limit {
        -limit
    } else {
        v
    }
}
