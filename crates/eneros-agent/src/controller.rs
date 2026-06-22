//! Agent 生命周期控制器 (T029-08)
//!
//! 提供 Agent 实例的运行时生命周期管理：start/stop/pause/resume/status。
//! 控制器维护每个 Agent 的状态机、tokio 任务句柄和控制通道，
//! 通过 mpsc 命令通道异步驱动 Agent 任务循环。
//!
//! ## 状态机
//!
//! ```text
//!                 start
//!   Stopped ───────────────► Running
//!      ▲                       │  │
//!      │ stop                  │  │ pause
//!      │                       │  ▼
//!      │                      stop Paused
//!      │                       │  │
//!      │         resume ◄──────┘  │
//!      └──────────────────────────┘
//! ```
//!
//! 合法转换：
//! - `start`:  Stopped → Running
//! - `stop`:   Running → Stopped, Paused → Stopped
//! - `pause`:  Running → Paused
//! - `resume`: Paused → Running
//! - `status`: 任意状态均合法（只读，不改变状态）
//!
//! 非法转换（返回 `InvalidTransition` 错误）：
//! - `start`  on Running/Paused
//! - `stop`   on Stopped
//! - `pause`  on Stopped/Paused
//! - `resume` on Stopped/Running

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Agent 生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentLifecycleState {
    /// 已停止（初始状态）
    Stopped,
    /// 运行中
    Running,
    /// 已暂停
    Paused,
    /// 错误状态
    Error,
}

impl AgentLifecycleState {
    /// 状态字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for AgentLifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 控制命令
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    /// 启动 Agent
    Start,
    /// 停止 Agent
    Stop,
    /// 暂停 Agent
    Pause,
    /// 恢复 Agent
    Resume,
    /// 查询状态
    Status,
}

impl ControlCommand {
    /// 从字符串解析控制命令（大小写不敏感）
    pub fn parse_action(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "status" => Some(Self::Status),
            _ => None,
        }
    }

    /// 命令字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Status => "status",
        }
    }
}

impl std::fmt::Display for ControlCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 控制命令的执行结果
#[derive(Debug, Clone)]
pub struct ControlResult {
    /// 执行前的状态
    pub previous_state: AgentLifecycleState,
    /// 执行后的状态
    pub current_state: AgentLifecycleState,
    /// 是否成功
    pub success: bool,
    /// 错误消息（失败时）
    pub error: Option<String>,
}

/// Agent 任务循环内部接收的命令
#[derive(Debug)]
enum AgentTaskCommand {
    /// 暂停任务循环
    Pause,
    /// 恢复任务循环
    Resume,
    /// 停止任务循环（退出）
    Stop,
}

/// Agent 句柄，包含状态、任务句柄和控制通道
struct AgentHandle {
    /// 当前生命周期状态
    state: AgentLifecycleState,
    /// Agent 任务循环的 JoinHandle
    task_handle: Option<JoinHandle<()>>,
    /// 向 Agent 任务循环发送控制命令的通道
    control_tx: Option<mpsc::Sender<AgentTaskCommand>>,
    /// Agent 类型（用于诊断）
    agent_type: String,
}

/// Agent 控制器，管理多个 Agent 实例的生命周期
///
/// 线程安全：内部使用 `Arc<RwLock<HashMap>>` 共享状态，
/// 可安全克隆并跨 await 点传递。
#[derive(Clone)]
pub struct AgentController {
    handles: Arc<RwLock<HashMap<String, AgentHandle>>>,
}

impl AgentController {
    /// 创建一个新的空控制器
    pub fn new() -> Self {
        Self {
            handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 注册一个 Agent（初始状态为 Stopped）
    ///
    /// 注册后 Agent 不会自动启动，需要调用 `control(id, Start)` 才会运行。
    /// 如果 Agent ID 已存在，则覆盖旧条目（调用方应先停止旧任务）。
    pub fn register(&self, agent_id: impl Into<String>, agent_type: impl Into<String>) {
        let id = agent_id.into();
        let at = agent_type.into();
        let mut handles = self.handles.write();
        handles.insert(
            id,
            AgentHandle {
                state: AgentLifecycleState::Stopped,
                task_handle: None,
                control_tx: None,
                agent_type: at,
            },
        );
    }

    /// 返回所有已注册的 Agent ID
    pub fn registered_ids(&self) -> Vec<String> {
        self.handles.read().keys().cloned().collect()
    }

    /// 查询 Agent 当前状态
    pub fn status(&self, agent_id: &str) -> Option<AgentLifecycleState> {
        self.handles.read().get(agent_id).map(|h| h.state)
    }

    /// 查询 Agent 类型（用于诊断）
    pub fn agent_type(&self, agent_id: &str) -> Option<String> {
        self.handles
            .read()
            .get(agent_id)
            .map(|h| h.agent_type.clone())
    }

    /// 执行控制命令
    ///
    /// 对于 `Status` 命令，仅返回当前状态，不改变状态。
    /// 对于其他命令，先校验状态转换合法性，再执行实际操作。
    pub async fn control(
        &self,
        agent_id: &str,
        command: ControlCommand,
    ) -> Result<ControlResult, ControlError> {
        // status 命令不需要状态转换
        if command == ControlCommand::Status {
            let state = self.status(agent_id).ok_or(ControlError::NotFound)?;
            return Ok(ControlResult {
                previous_state: state,
                current_state: state,
                success: true,
                error: None,
            });
        }

        // 对于需要状态转换的命令，先获取当前状态并校验合法性
        let previous_state = {
            let handles = self.handles.read();
            handles
                .get(agent_id)
                .map(|h| h.state)
                .ok_or(ControlError::NotFound)?
        };

        // 校验状态转换合法性
        let new_state = validate_transition(previous_state, command).ok_or(ControlError::InvalidTransition {
            from: previous_state,
            command,
        })?;

        // 执行实际的状态转换操作
        match command {
            ControlCommand::Start => self.start_agent(agent_id).await?,
            ControlCommand::Stop => self.stop_agent(agent_id).await?,
            ControlCommand::Pause => self.pause_agent(agent_id).await?,
            ControlCommand::Resume => self.resume_agent(agent_id).await?,
            ControlCommand::Status => unreachable!(),
        }

        // 更新状态
        {
            let mut handles = self.handles.write();
            if let Some(handle) = handles.get_mut(agent_id) {
                handle.state = new_state;
            }
        }

        Ok(ControlResult {
            previous_state,
            current_state: new_state,
            success: true,
            error: None,
        })
    }

    /// 启动 Agent 任务循环
    ///
    /// 创建一个真实的 tokio 任务，该任务通过 mpsc 通道接收控制命令
    /// （Pause/Resume/Stop），实现真实的生命周期管理。
    async fn start_agent(&self, agent_id: &str) -> Result<(), ControlError> {
        let (control_tx, mut control_rx) = mpsc::channel::<AgentTaskCommand>(8);
        let id_owned = agent_id.to_string();

        // 启动一个真实的 tokio 任务循环
        // 该任务持续运行，直到收到 Stop 命令或通道关闭
        let task_handle = tokio::spawn(async move {
            tracing::info!(agent_id = %id_owned, "agent task started");
            while let Some(cmd) = control_rx.recv().await {
                match cmd {
                    AgentTaskCommand::Pause => {
                        tracing::info!(agent_id = %id_owned, "agent task paused");
                    }
                    AgentTaskCommand::Resume => {
                        tracing::info!(agent_id = %id_owned, "agent task resumed");
                    }
                    AgentTaskCommand::Stop => {
                        tracing::info!(agent_id = %id_owned, "agent task stopping");
                        break;
                    }
                }
            }
            tracing::info!(agent_id = %id_owned, "agent task exited");
        });

        let mut handles = self.handles.write();
        if let Some(handle) = handles.get_mut(agent_id) {
            handle.task_handle = Some(task_handle);
            handle.control_tx = Some(control_tx);
        }
        Ok(())
    }

    /// 停止 Agent 任务循环
    ///
    /// 发送 Stop 命令并等待任务退出，确保资源被正确释放。
    async fn stop_agent(&self, agent_id: &str) -> Result<(), ControlError> {
        let (task_handle, control_tx) = {
            let mut handles = self.handles.write();
            let handle = handles
                .get_mut(agent_id)
                .ok_or(ControlError::NotFound)?;
            let tx = handle.control_tx.take();
            let th = handle.task_handle.take();
            (th, tx)
        };

        // 发送 Stop 命令并等待任务退出
        if let Some(tx) = control_tx {
            let _ = tx.send(AgentTaskCommand::Stop).await;
        }
        if let Some(handle) = task_handle {
            let _ = handle.await;
        }
        Ok(())
    }

    /// 暂停 Agent 任务循环
    async fn pause_agent(&self, agent_id: &str) -> Result<(), ControlError> {
        let control_tx = {
            let handles = self.handles.read();
            handles
                .get(agent_id)
                .and_then(|h| h.control_tx.clone())
                .ok_or(ControlError::NotFound)?
        };
        let _ = control_tx.send(AgentTaskCommand::Pause).await;
        Ok(())
    }

    /// 恢复 Agent 任务循环
    async fn resume_agent(&self, agent_id: &str) -> Result<(), ControlError> {
        let control_tx = {
            let handles = self.handles.read();
            handles
                .get(agent_id)
                .and_then(|h| h.control_tx.clone())
                .ok_or(ControlError::NotFound)?
        };
        let _ = control_tx.send(AgentTaskCommand::Resume).await;
        Ok(())
    }
}

impl Default for AgentController {
    fn default() -> Self {
        Self::new()
    }
}

/// 控制错误
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// Agent 未注册
    #[error("agent not found")]
    NotFound,
    /// 状态转换非法
    #[error("invalid transition: cannot {command} from {from}")]
    InvalidTransition {
        /// 转换前的状态
        from: AgentLifecycleState,
        /// 试图执行的命令
        command: ControlCommand,
    },
}

/// 校验状态转换合法性
///
/// 返回 `Some(new_state)` 表示转换合法，`None` 表示非法。
fn validate_transition(
    from: AgentLifecycleState,
    command: ControlCommand,
) -> Option<AgentLifecycleState> {
    match (from, command) {
        // start: Stopped → Running
        (AgentLifecycleState::Stopped, ControlCommand::Start) => Some(AgentLifecycleState::Running),
        // stop: Running → Stopped, Paused → Stopped
        (AgentLifecycleState::Running, ControlCommand::Stop) => Some(AgentLifecycleState::Stopped),
        (AgentLifecycleState::Paused, ControlCommand::Stop) => Some(AgentLifecycleState::Stopped),
        // pause: Running → Paused
        (AgentLifecycleState::Running, ControlCommand::Pause) => Some(AgentLifecycleState::Paused),
        // resume: Paused → Running
        (AgentLifecycleState::Paused, ControlCommand::Resume) => Some(AgentLifecycleState::Running),
        // 其他转换非法
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_state_serde() {
        let state = AgentLifecycleState::Running;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"running\"");
        let decoded: AgentLifecycleState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn test_lifecycle_state_all_variants() {
        for state in [
            AgentLifecycleState::Stopped,
            AgentLifecycleState::Running,
            AgentLifecycleState::Paused,
            AgentLifecycleState::Error,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let decoded: AgentLifecycleState = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, state);
        }
    }

    #[test]
    fn test_control_command_from_str() {
        assert_eq!(ControlCommand::parse_action("start"), Some(ControlCommand::Start));
        assert_eq!(ControlCommand::parse_action("STOP"), Some(ControlCommand::Stop));
        assert_eq!(ControlCommand::parse_action("Pause"), Some(ControlCommand::Pause));
        assert_eq!(ControlCommand::parse_action("resume"), Some(ControlCommand::Resume));
        assert_eq!(ControlCommand::parse_action("status"), Some(ControlCommand::Status));
        assert_eq!(ControlCommand::parse_action("invalid"), None);
    }

    #[test]
    fn test_validate_transition_legal() {
        // 合法转换
        assert_eq!(
            validate_transition(AgentLifecycleState::Stopped, ControlCommand::Start),
            Some(AgentLifecycleState::Running)
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Running, ControlCommand::Stop),
            Some(AgentLifecycleState::Stopped)
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Paused, ControlCommand::Stop),
            Some(AgentLifecycleState::Stopped)
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Running, ControlCommand::Pause),
            Some(AgentLifecycleState::Paused)
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Paused, ControlCommand::Resume),
            Some(AgentLifecycleState::Running)
        );
    }

    #[test]
    fn test_validate_transition_illegal() {
        // 非法转换
        assert_eq!(
            validate_transition(AgentLifecycleState::Running, ControlCommand::Start),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Paused, ControlCommand::Start),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Stopped, ControlCommand::Stop),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Stopped, ControlCommand::Pause),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Paused, ControlCommand::Pause),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Stopped, ControlCommand::Resume),
            None
        );
        assert_eq!(
            validate_transition(AgentLifecycleState::Running, ControlCommand::Resume),
            None
        );
    }

    #[test]
    fn test_controller_register_and_status() {
        let controller = AgentController::new();
        controller.register("dispatch-1", "Dispatcher");

        assert_eq!(
            controller.status("dispatch-1"),
            Some(AgentLifecycleState::Stopped)
        );
        assert_eq!(controller.status("unknown"), None);
        assert_eq!(controller.registered_ids(), vec!["dispatch-1".to_string()]);
    }

    #[tokio::test]
    async fn test_controller_start_stop() {
        let controller = AgentController::new();
        controller.register("dispatch-1", "Dispatcher");

        // start: Stopped → Running
        let result = controller
            .control("dispatch-1", ControlCommand::Start)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Stopped);
        assert_eq!(result.current_state, AgentLifecycleState::Running);
        assert_eq!(
            controller.status("dispatch-1"),
            Some(AgentLifecycleState::Running)
        );

        // stop: Running → Stopped
        let result = controller
            .control("dispatch-1", ControlCommand::Stop)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Running);
        assert_eq!(result.current_state, AgentLifecycleState::Stopped);
        assert_eq!(
            controller.status("dispatch-1"),
            Some(AgentLifecycleState::Stopped)
        );
    }

    #[tokio::test]
    async fn test_controller_pause_resume() {
        let controller = AgentController::new();
        controller.register("operation-1", "Operator");

        // 必须先 start
        controller
            .control("operation-1", ControlCommand::Start)
            .await
            .unwrap();

        // pause: Running → Paused
        let result = controller
            .control("operation-1", ControlCommand::Pause)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Running);
        assert_eq!(result.current_state, AgentLifecycleState::Paused);

        // resume: Paused → Running
        let result = controller
            .control("operation-1", ControlCommand::Resume)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Paused);
        assert_eq!(result.current_state, AgentLifecycleState::Running);

        // 清理
        controller
            .control("operation-1", ControlCommand::Stop)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_controller_status_command() {
        let controller = AgentController::new();
        controller.register("forecast-1", "Forecaster");

        let result = controller
            .control("forecast-1", ControlCommand::Status)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Stopped);
        assert_eq!(result.current_state, AgentLifecycleState::Stopped);
    }

    #[tokio::test]
    async fn test_controller_not_found() {
        let controller = AgentController::new();
        let result = controller.control("unknown", ControlCommand::Start).await;
        assert!(matches!(result, Err(ControlError::NotFound)));
    }

    #[tokio::test]
    async fn test_controller_invalid_transition() {
        let controller = AgentController::new();
        controller.register("planning-1", "Planner");

        // Stopped 状态下 pause 应失败
        let result = controller
            .control("planning-1", ControlCommand::Pause)
            .await;
        assert!(matches!(
            result,
            Err(ControlError::InvalidTransition { .. })
        ));

        // Stopped 状态下 stop 应失败
        let result = controller
            .control("planning-1", ControlCommand::Stop)
            .await;
        assert!(matches!(
            result,
            Err(ControlError::InvalidTransition { .. })
        ));
    }

    #[tokio::test]
    async fn test_controller_start_when_running_is_illegal() {
        let controller = AgentController::new();
        controller.register("trading-1", "Trader");

        controller
            .control("trading-1", ControlCommand::Start)
            .await
            .unwrap();

        // Running 状态下再次 start 应失败
        let result = controller
            .control("trading-1", ControlCommand::Start)
            .await;
        assert!(matches!(
            result,
            Err(ControlError::InvalidTransition { .. })
        ));

        controller
            .control("trading-1", ControlCommand::Stop)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_controller_pause_when_paused_is_illegal() {
        let controller = AgentController::new();
        controller.register("self-healing-1", "SelfHealing");

        controller
            .control("self-healing-1", ControlCommand::Start)
            .await
            .unwrap();
        controller
            .control("self-healing-1", ControlCommand::Pause)
            .await
            .unwrap();

        // Paused 状态下再次 pause 应失败
        let result = controller
            .control("self-healing-1", ControlCommand::Pause)
            .await;
        assert!(matches!(
            result,
            Err(ControlError::InvalidTransition { .. })
        ));

        controller
            .control("self-healing-1", ControlCommand::Stop)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_controller_stop_from_paused() {
        let controller = AgentController::new();
        controller.register("dispatch-1", "Dispatcher");

        controller
            .control("dispatch-1", ControlCommand::Start)
            .await
            .unwrap();
        controller
            .control("dispatch-1", ControlCommand::Pause)
            .await
            .unwrap();

        // Paused 状态下 stop 应成功
        let result = controller
            .control("dispatch-1", ControlCommand::Stop)
            .await
            .unwrap();
        assert_eq!(result.previous_state, AgentLifecycleState::Paused);
        assert_eq!(result.current_state, AgentLifecycleState::Stopped);
    }

    #[tokio::test]
    async fn test_controller_full_lifecycle() {
        let controller = AgentController::new();
        controller.register("dispatch-1", "Dispatcher");

        // Stopped → Running → Paused → Running → Paused → Stopped → Running → Stopped
        let states = [
            (ControlCommand::Start, AgentLifecycleState::Running),
            (ControlCommand::Pause, AgentLifecycleState::Paused),
            (ControlCommand::Resume, AgentLifecycleState::Running),
            (ControlCommand::Pause, AgentLifecycleState::Paused),
            (ControlCommand::Stop, AgentLifecycleState::Stopped),
            (ControlCommand::Start, AgentLifecycleState::Running),
            (ControlCommand::Stop, AgentLifecycleState::Stopped),
        ];

        for (cmd, expected) in states {
            let result = controller.control("dispatch-1", cmd).await.unwrap();
            assert_eq!(
                result.current_state, expected,
                "after {:?}, expected {:?}, got {:?}",
                cmd, expected, result.current_state
            );
        }
    }

    #[tokio::test]
    async fn test_controller_multiple_agents_independent() {
        let controller = AgentController::new();
        controller.register("dispatch-1", "Dispatcher");
        controller.register("operation-1", "Operator");

        // 启动 dispatch-1
        controller
            .control("dispatch-1", ControlCommand::Start)
            .await
            .unwrap();
        assert_eq!(
            controller.status("dispatch-1"),
            Some(AgentLifecycleState::Running)
        );
        assert_eq!(
            controller.status("operation-1"),
            Some(AgentLifecycleState::Stopped)
        );

        // 启动 operation-1 并暂停
        controller
            .control("operation-1", ControlCommand::Start)
            .await
            .unwrap();
        controller
            .control("operation-1", ControlCommand::Pause)
            .await
            .unwrap();
        assert_eq!(
            controller.status("dispatch-1"),
            Some(AgentLifecycleState::Running)
        );
        assert_eq!(
            controller.status("operation-1"),
            Some(AgentLifecycleState::Paused)
        );

        // 清理
        controller
            .control("dispatch-1", ControlCommand::Stop)
            .await
            .unwrap();
        controller
            .control("operation-1", ControlCommand::Stop)
            .await
            .unwrap();
    }
}
