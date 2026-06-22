//! 插件生命周期状态机
//!
//! 定义插件的状态及合法的状态转换。
//!
//! 状态转换规则：
//! - `Loaded` -> `Initialized` | `Failed`
//! - `Initialized` -> `Starting` | `Stopped` | `Failed`
//! - `Starting` -> `Running` | `Failed`
//! - `Running` -> `Stopping` | `Crashed` | `Failed`
//! - `Stopping` -> `Stopped` | `Failed`
//! - `Stopped` -> 终态（可 unload）
//! - `Crashed` -> `Stopped` | `Failed`
//! - `Failed` -> 终态（不可恢复）

use crate::error::PluginError;

/// 插件状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginState {
    /// 库已加载，未初始化
    Loaded,
    /// init() 完成
    Initialized,
    /// 正在启动
    Starting,
    /// start() 完成，运行中
    Running,
    /// 正在停止
    Stopping,
    /// stop() 完成，已停止
    Stopped,
    /// 崩溃
    Crashed,
    /// 失败（不可恢复）
    Failed,
}

impl PluginState {
    /// 判断从当前状态到目标状态的转换是否合法
    fn can_transition_to(self, target: PluginState) -> bool {
        use PluginState::*;
        matches!(
            (self, target),
            (Loaded, Initialized)
                | (Loaded, Failed)
                | (Initialized, Starting)
                | (Initialized, Stopped)
                | (Initialized, Failed)
                | (Starting, Running)
                | (Starting, Failed)
                | (Running, Stopping)
                | (Running, Crashed)
                | (Running, Failed)
                | (Stopping, Stopped)
                | (Stopping, Failed)
                | (Crashed, Stopped)
                | (Crashed, Failed)
        )
    }
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// 插件生命周期管理器
#[derive(Debug)]
pub struct PluginLifecycle {
    state: PluginState,
}

impl PluginLifecycle {
    /// 创建新的生命周期管理器，初始状态为 `Loaded`
    pub fn new() -> Self {
        Self {
            state: PluginState::Loaded,
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> PluginState {
        self.state
    }

    /// 执行状态转换，非法转换返回错误
    pub fn transition(&mut self, target: PluginState) -> Result<(), PluginError> {
        if self.state.can_transition_to(target) {
            self.state = target;
            Ok(())
        } else {
            Err(PluginError::InvalidStateTransition(format!(
                "{} -> {}",
                self.state, target
            )))
        }
    }
}

impl Default for PluginLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_is_loaded() {
        let lc = PluginLifecycle::new();
        assert_eq!(lc.state(), PluginState::Loaded);
    }

    #[test]
    fn test_valid_full_lifecycle() {
        let mut lc = PluginLifecycle::new();
        assert!(lc.transition(PluginState::Initialized).is_ok());
        assert_eq!(lc.state(), PluginState::Initialized);
        assert!(lc.transition(PluginState::Starting).is_ok());
        assert_eq!(lc.state(), PluginState::Starting);
        assert!(lc.transition(PluginState::Running).is_ok());
        assert_eq!(lc.state(), PluginState::Running);
        assert!(lc.transition(PluginState::Stopping).is_ok());
        assert_eq!(lc.state(), PluginState::Stopping);
        assert!(lc.transition(PluginState::Stopped).is_ok());
        assert_eq!(lc.state(), PluginState::Stopped);
    }

    #[test]
    fn test_loaded_to_failed() {
        let mut lc = PluginLifecycle::new();
        assert!(lc.transition(PluginState::Failed).is_ok());
        assert_eq!(lc.state(), PluginState::Failed);
    }

    #[test]
    fn test_running_to_crashed_to_stopped() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Initialized).unwrap();
        lc.transition(PluginState::Starting).unwrap();
        lc.transition(PluginState::Running).unwrap();
        assert!(lc.transition(PluginState::Crashed).is_ok());
        assert_eq!(lc.state(), PluginState::Crashed);
        assert!(lc.transition(PluginState::Stopped).is_ok());
        assert_eq!(lc.state(), PluginState::Stopped);
    }

    #[test]
    fn test_initialized_to_failed() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Initialized).unwrap();
        assert!(lc.transition(PluginState::Failed).is_ok());
        assert_eq!(lc.state(), PluginState::Failed);
    }

    #[test]
    fn test_initialized_to_stopped() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Initialized).unwrap();
        assert!(lc.transition(PluginState::Stopped).is_ok());
        assert_eq!(lc.state(), PluginState::Stopped);
    }

    #[test]
    fn test_invalid_transition_loaded_to_running() {
        let mut lc = PluginLifecycle::new();
        let result = lc.transition(PluginState::Running);
        assert!(result.is_err());
        assert_eq!(lc.state(), PluginState::Loaded);
        // 验证错误类型为 InvalidStateTransition
        match result {
            Err(PluginError::InvalidStateTransition(msg)) => {
                assert!(msg.contains("Loaded"));
                assert!(msg.contains("Running"));
            }
            other => panic!("expected InvalidStateTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_transition_stopped_to_running() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Initialized).unwrap();
        lc.transition(PluginState::Stopped).unwrap();
        let result = lc.transition(PluginState::Running);
        assert!(result.is_err());
        assert_eq!(lc.state(), PluginState::Stopped);
        // 验证错误类型为 InvalidStateTransition
        match result {
            Err(PluginError::InvalidStateTransition(msg)) => {
                assert!(msg.contains("Stopped"));
                assert!(msg.contains("Running"));
            }
            other => panic!("expected InvalidStateTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_failed_is_terminal() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Failed).unwrap();
        // Failed 是终态，不能转换到任何状态
        assert!(lc.transition(PluginState::Loaded).is_err());
        assert!(lc.transition(PluginState::Initialized).is_err());
        assert!(lc.transition(PluginState::Running).is_err());
        assert!(lc.transition(PluginState::Stopped).is_err());
        assert_eq!(lc.state(), PluginState::Failed);
        // 验证错误类型为 InvalidStateTransition
        let result = lc.transition(PluginState::Running);
        match result {
            Err(PluginError::InvalidStateTransition(msg)) => {
                assert!(msg.contains("Failed"));
                assert!(msg.contains("Running"));
            }
            other => panic!("expected InvalidStateTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_stopped_is_terminal() {
        let mut lc = PluginLifecycle::new();
        lc.transition(PluginState::Initialized).unwrap();
        lc.transition(PluginState::Stopped).unwrap();
        assert!(lc.transition(PluginState::Running).is_err());
        assert!(lc.transition(PluginState::Loaded).is_err());
        assert_eq!(lc.state(), PluginState::Stopped);
        // 验证错误类型为 InvalidStateTransition
        let result = lc.transition(PluginState::Running);
        match result {
            Err(PluginError::InvalidStateTransition(msg)) => {
                assert!(msg.contains("Stopped"));
                assert!(msg.contains("Running"));
            }
            other => panic!("expected InvalidStateTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_transition_same_state() {
        let mut lc = PluginLifecycle::new();
        // 同状态转换不合法（除显式定义外）
        let result = lc.transition(PluginState::Loaded);
        assert!(result.is_err());
        // 验证错误类型为 InvalidStateTransition
        match result {
            Err(PluginError::InvalidStateTransition(msg)) => {
                assert!(msg.contains("Loaded"));
            }
            other => panic!("expected InvalidStateTransition, got {:?}", other),
        }
    }

    #[test]
    fn test_default_impl() {
        let lc = PluginLifecycle::default();
        assert_eq!(lc.state(), PluginState::Loaded);
    }

    #[test]
    fn test_state_display() {
        assert_eq!(PluginState::Running.to_string(), "Running");
        assert_eq!(PluginState::Loaded.to_string(), "Loaded");
    }
}
