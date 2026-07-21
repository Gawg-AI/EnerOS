//! GridPublisher trait + MockGridPublisher.

use alloc::vec::Vec;

use crate::state::GridState;
use crate::GridError;

/// 电网状态/告警发布器接口.
///
/// 抽象 DDS/总线发布行为：`publish_state` 发布常规状态，`publish_alert` 发布异常告警。
pub trait GridPublisher {
    /// 发布电网状态.
    fn publish_state(&mut self, state: &GridState) -> Result<(), GridError>;
    /// 发布电网告警.
    fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError>;
}

/// Mock 电网发布器（测试用）.
#[derive(Debug, Clone, Default)]
pub struct MockGridPublisher {
    /// 已发布的状态列表
    pub published_states: Vec<GridState>,
    /// 已发布的告警列表
    pub published_alerts: Vec<GridState>,
    /// 是否模拟状态发布失败
    pub fail_state: bool,
    /// 是否模拟告警发布失败
    pub fail_alert: bool,
}

impl MockGridPublisher {
    /// 创建成功路径发布器.
    pub fn new() -> Self {
        MockGridPublisher::default()
    }

    /// 创建状态发布失败发布器.
    pub fn new_failing_state() -> Self {
        MockGridPublisher {
            fail_state: true,
            ..MockGridPublisher::default()
        }
    }

    /// 创建告警发布失败发布器.
    pub fn new_failing_alert() -> Self {
        MockGridPublisher {
            fail_alert: true,
            ..MockGridPublisher::default()
        }
    }
}

impl GridPublisher for MockGridPublisher {
    fn publish_state(&mut self, state: &GridState) -> Result<(), GridError> {
        if self.fail_state {
            return Err(GridError::PublishFailed);
        }
        self.published_states.push(*state);
        Ok(())
    }

    fn publish_alert(&mut self, state: &GridState) -> Result<(), GridError> {
        if self.fail_alert {
            return Err(GridError::PublishFailed);
        }
        self.published_alerts.push(*state);
        Ok(())
    }
}

/// 发布状态辅助函数（委托给 `publisher.publish_state`）.
pub fn publish_state(
    publisher: &mut dyn GridPublisher,
    state: &GridState,
) -> Result<(), GridError> {
    publisher.publish_state(state)
}
