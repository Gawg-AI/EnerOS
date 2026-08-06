//! 生命周期管理（SDK v1.0 稳定 API）。
//!
//! 定义 Agent 生命周期状态与状态机。

/// Agent 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// 已创建（未初始化）
    Created,
    /// 已初始化（可启动）
    Initialized,
    /// 运行中
    Running,
    /// 已停止
    Stopped,
    /// 已销毁
    Destroyed,
    /// 错误状态
    Error,
}

impl LifecycleState {
    /// 获取状态名称。
    pub fn name(&self) -> &'static str {
        match self {
            LifecycleState::Created => "Created",
            LifecycleState::Initialized => "Initialized",
            LifecycleState::Running => "Running",
            LifecycleState::Stopped => "Stopped",
            LifecycleState::Destroyed => "Destroyed",
            LifecycleState::Error => "Error",
        }
    }

    /// 检查是否可以转换到目标状态。
    ///
    /// 合法转换：
    /// - Created → Initialized, Error, Destroyed
    /// - Initialized → Running, Error, Destroyed
    /// - Running → Stopped, Error
    /// - Stopped → Running, Destroyed
    /// - Error → Initialized, Destroyed
    /// - Destroyed → (终态，不可转换)
    pub fn can_transition_to(&self, target: LifecycleState) -> bool {
        matches!(
            (self, target),
            (LifecycleState::Created, LifecycleState::Initialized)
                | (LifecycleState::Created, LifecycleState::Error)
                | (LifecycleState::Created, LifecycleState::Destroyed)
                | (LifecycleState::Initialized, LifecycleState::Running)
                | (LifecycleState::Initialized, LifecycleState::Error)
                | (LifecycleState::Initialized, LifecycleState::Destroyed)
                | (LifecycleState::Running, LifecycleState::Stopped)
                | (LifecycleState::Running, LifecycleState::Error)
                | (LifecycleState::Stopped, LifecycleState::Running)
                | (LifecycleState::Stopped, LifecycleState::Destroyed)
                | (LifecycleState::Error, LifecycleState::Initialized)
                | (LifecycleState::Error, LifecycleState::Destroyed)
        )
    }
}

/// 生命周期管理器。
///
/// 跟踪 Agent 当前状态并验证状态转换合法性。
pub struct LifecycleManager {
    /// 当前状态
    state: LifecycleState,
    /// 状态转换计数
    transition_count: u64,
}

impl LifecycleManager {
    /// 创建新的生命周期管理器（初始状态 Created）。
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Created,
            transition_count: 0,
        }
    }

    /// 获取当前状态。
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// 尝试转换到目标状态。
    ///
    /// 非法转换返回 `false`，合法转换返回 `true` 并更新状态。
    pub fn transition_to(&mut self, target: LifecycleState) -> bool {
        if self.state.can_transition_to(target) {
            self.state = target;
            self.transition_count += 1;
            true
        } else {
            false
        }
    }

    /// 获取状态转换计数。
    pub fn transition_count(&self) -> u64 {
        self.transition_count
    }

    /// 检查是否处于运行状态。
    pub fn is_running(&self) -> bool {
        self.state == LifecycleState::Running
    }

    /// 检查是否已销毁。
    pub fn is_destroyed(&self) -> bool {
        self.state == LifecycleState::Destroyed
    }

    /// 检查是否处于错误状态。
    pub fn is_error(&self) -> bool {
        self.state == LifecycleState::Error
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// LCM1：初始状态。
    #[test]
    fn lcm1_initial_state() {
        let lcm = LifecycleManager::new();
        assert_eq!(lcm.state(), LifecycleState::Created);
        assert_eq!(lcm.transition_count(), 0);
    }

    /// LCM2：正常生命周期流程。
    #[test]
    fn lcm2_normal_lifecycle() {
        let mut lcm = LifecycleManager::new();
        assert!(lcm.transition_to(LifecycleState::Initialized));
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.is_running());
        assert!(lcm.transition_to(LifecycleState::Stopped));
        assert!(lcm.transition_to(LifecycleState::Destroyed));
        assert!(lcm.is_destroyed());
        assert_eq!(lcm.transition_count(), 4);
    }

    /// LCM3：非法转换拒绝。
    #[test]
    fn lcm3_invalid_transition() {
        let mut lcm = LifecycleManager::new();
        // Created 不能直接到 Running
        assert!(!lcm.transition_to(LifecycleState::Running));
        assert_eq!(lcm.state(), LifecycleState::Created);
        assert_eq!(lcm.transition_count(), 0);
    }

    /// LCM4：错误恢复。
    #[test]
    fn lcm4_error_recovery() {
        let mut lcm = LifecycleManager::new();
        assert!(lcm.transition_to(LifecycleState::Initialized));
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.transition_to(LifecycleState::Error));
        assert!(lcm.is_error());
        // 从错误恢复到 Initialized
        assert!(lcm.transition_to(LifecycleState::Initialized));
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.is_running());
    }

    /// LCM5：Destroyed 是终态。
    #[test]
    fn lcm5_destroyed_terminal() {
        let mut lcm = LifecycleManager::new();
        assert!(lcm.transition_to(LifecycleState::Destroyed));
        // 不能从 Destroyed 转换到任何状态
        assert!(!lcm.transition_to(LifecycleState::Created));
        assert!(!lcm.transition_to(LifecycleState::Initialized));
        assert!(!lcm.transition_to(LifecycleState::Running));
    }

    /// LCM6：状态名称。
    #[test]
    fn lcm6_state_names() {
        assert_eq!(LifecycleState::Created.name(), "Created");
        assert_eq!(LifecycleState::Initialized.name(), "Initialized");
        assert_eq!(LifecycleState::Running.name(), "Running");
        assert_eq!(LifecycleState::Stopped.name(), "Stopped");
        assert_eq!(LifecycleState::Destroyed.name(), "Destroyed");
        assert_eq!(LifecycleState::Error.name(), "Error");
    }

    /// LCM7：停止后重启。
    #[test]
    fn lcm7_restart_after_stop() {
        let mut lcm = LifecycleManager::new();
        assert!(lcm.transition_to(LifecycleState::Initialized));
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.transition_to(LifecycleState::Stopped));
        // 停止后可以重新启动
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.is_running());
    }

    /// LCM8：can_transition_to 全覆盖。
    #[test]
    fn lcm8_can_transition_coverage() {
        // Created 可以到 Initialized, Error, Destroyed
        assert!(LifecycleState::Created.can_transition_to(LifecycleState::Initialized));
        assert!(LifecycleState::Created.can_transition_to(LifecycleState::Error));
        assert!(LifecycleState::Created.can_transition_to(LifecycleState::Destroyed));
        assert!(!LifecycleState::Created.can_transition_to(LifecycleState::Running));
        assert!(!LifecycleState::Created.can_transition_to(LifecycleState::Stopped));
    }
}
