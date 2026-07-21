//! 设定值跟踪器 — 斜率限制设定值渐变.
//!
//! [`SetpointTracker`] 按最大变化率（单位/秒）限制设定值的单步变化，
//! 防止设定值突变对控制系统造成冲击.

/// 设定值跟踪器（3 字段）.
///
/// `current` 从 `initial` 出发，按 `max_rate_per_s * dt` 的步长向 `target`
/// 收敛。当 `max_rate_per_s = f64::MAX` 时无限制，直接跳变到 `target`.
pub struct SetpointTracker {
    /// 当前设定值.
    current: f64,
    /// 目标设定值.
    target: f64,
    /// 最大变化率（单位/秒）.
    max_rate_per_s: f64,
}

impl SetpointTracker {
    /// 创建设定值跟踪器.
    ///
    /// 初始 `current = initial`，`target = initial`（已稳定）.
    pub fn new(initial: f64, max_rate_per_s: f64) -> Self {
        Self {
            current: initial,
            target: initial,
            max_rate_per_s,
        }
    }

    /// 设置目标设定值.
    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// 推进一步，返回更新后的当前设定值.
    ///
    /// 按 `max_rate_per_s * dt` 限制单步变化量，向 `target` 收敛.
    /// 当 `max_rate_per_s = f64::MAX` 时直接返回 `target`.
    pub fn update(&mut self, dt: f64) -> f64 {
        if self.max_rate_per_s == f64::MAX {
            self.current = self.target;
            return self.current;
        }

        let max_step = self.max_rate_per_s * dt;
        let diff = self.target - self.current;

        if diff.abs() <= max_step {
            self.current = self.target;
        } else if diff > 0.0 {
            self.current += max_step;
        } else {
            self.current -= max_step;
        }

        self.current
    }

    /// 是否已稳定（current ≈ target，浮点容差 1e-9）.
    pub fn is_settled(&self) -> bool {
        (self.current - self.target).abs() < 1e-9
    }

    /// 获取当前设定值.
    pub fn current(&self) -> f64 {
        self.current
    }

    /// 获取目标设定值.
    pub fn target(&self) -> f64 {
        self.target
    }
}
