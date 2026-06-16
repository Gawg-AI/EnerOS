use std::sync::Arc;
use parking_lot::RwLock;
use eneros_core::{SystemOperatingState, SeverityLevel};
use eneros_eventbus::{EventBus, Event};
use eneros_eventbus::event::{EventType, EventPayload};
use eneros_constraint::ConstraintEngine;

/// State transition trigger
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateTransitionTrigger {
    /// Critical constraint violation detected
    CriticalViolation,
    /// Multiple cascading violations
    CascadingViolation,
    /// System has stabilized
    Stabilized,
    /// Total system collapse
    SystemCollapse,
    /// Restoration initiated
    RestorationInitiated,
    /// Restoration completed
    RestorationCompleted,
    /// Manual override
    ManualOverride(SystemOperatingState),
}

/// Result of a state transition
#[derive(Debug, Clone)]
pub struct StateTransitionResult {
    /// Previous state
    pub from: SystemOperatingState,
    /// New state
    pub to: SystemOperatingState,
    /// Whether the transition was successful
    pub success: bool,
    /// Reason for the transition
    pub reason: String,
    /// Actions triggered by the transition
    pub triggered_actions: Vec<String>,
}

/// System operating state machine
pub struct SystemStateMachine {
    /// Current state
    state: Arc<RwLock<SystemOperatingState>>,
    /// Event bus for publishing state change events
    event_bus: Option<Arc<EventBus>>,
    /// Constraint engine for threshold adjustment
    constraint_engine: Option<Arc<ConstraintEngine>>,
}

impl SystemStateMachine {
    /// Create a new state machine starting in Normal state
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: None,
            constraint_engine: None,
        }
    }

    /// Create with event bus integration
    pub fn with_event_bus(event_bus: Arc<EventBus>) -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: Some(event_bus),
            constraint_engine: None,
        }
    }

    /// Create with event bus and constraint engine
    pub fn with_event_bus_and_constraints(
        event_bus: Arc<EventBus>,
        constraint_engine: Arc<ConstraintEngine>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(SystemOperatingState::Normal)),
            event_bus: Some(event_bus),
            constraint_engine: Some(constraint_engine),
        }
    }

    /// Get current state
    pub fn current_state(&self) -> SystemOperatingState {
        *self.state.read()
    }

    /// Attempt a state transition
    pub fn transition(&self, trigger: StateTransitionTrigger) -> StateTransitionResult {
        let current = *self.state.read();
        let target_opt = self.determine_target(&trigger, current);

        let mut result = StateTransitionResult {
            from: current,
            to: current,
            success: false,
            reason: format!("{:?}", trigger),
            triggered_actions: Vec::new(),
        };

        let target = match target_opt {
            Some(t) => t,
            None => {
                result.reason = format!("Trigger {:?} not applicable in {:?} state", trigger, current);
                return result;
            }
        };

        if current == target {
            result.to = target;
            result.success = true;
            return result;
        }

        if !current.can_transition_to(target) {
            result.reason = format!("Invalid transition from {:?} to {:?}", current, target);
            return result;
        }

        // Apply the transition
        result.to = target;
        *self.state.write() = target;
        result.success = true;

        // Execute transition actions
        self.on_state_changed(&mut result);

        result
    }

    /// Determine target state based on trigger.
    /// Returns None if the trigger is not applicable in the current state.
    fn determine_target(&self, trigger: &StateTransitionTrigger, current: SystemOperatingState) -> Option<SystemOperatingState> {
        match trigger {
            StateTransitionTrigger::CriticalViolation => {
                match current {
                    SystemOperatingState::Normal => Some(SystemOperatingState::Alert),
                    SystemOperatingState::Alert => Some(SystemOperatingState::Emergency),
                    _ => None,
                }
            }
            StateTransitionTrigger::CascadingViolation => {
                match current {
                    SystemOperatingState::Normal | SystemOperatingState::Alert => Some(SystemOperatingState::Emergency),
                    SystemOperatingState::Emergency => Some(SystemOperatingState::Blackout),
                    _ => None,
                }
            }
            StateTransitionTrigger::Stabilized => {
                match current {
                    SystemOperatingState::Alert => Some(SystemOperatingState::Normal),
                    SystemOperatingState::Emergency => Some(SystemOperatingState::Alert),
                    _ => None,
                }
            }
            StateTransitionTrigger::SystemCollapse => Some(SystemOperatingState::Blackout),
            StateTransitionTrigger::RestorationInitiated => {
                if current == SystemOperatingState::Blackout {
                    Some(SystemOperatingState::Restoration)
                } else {
                    None
                }
            }
            StateTransitionTrigger::RestorationCompleted => {
                if current == SystemOperatingState::Restoration {
                    Some(SystemOperatingState::Normal)
                } else {
                    None
                }
            }
            StateTransitionTrigger::ManualOverride(target) => Some(*target),
        }
    }

    /// Execute actions on state change.
    ///
    /// This runs *after* the new state has already been committed to
    /// `self.state` (see `transition`), so the constraint engine is updated to
    /// match the state the system is now actually operating in.
    fn on_state_changed(&self, result: &mut StateTransitionResult) {
        // Adjust constraint engine thresholds to match the new operating state.
        // The engine is shared (`Arc<ConstraintEngine>`) but `set_emergency_thresholds`
        // takes `&self` via interior mutability, so no `&mut` is needed here.
        if let Some(ref ce) = self.constraint_engine {
            ce.set_emergency_thresholds(result.to);
            result.triggered_actions.push(format!(
                "Constraint thresholds adjusted for {:?} state (multiplier = {})",
                result.to,
                ce.threshold_multiplier()
            ));
        }

        // Publish state change event
        if let Some(ref bus) = self.event_bus {
            let _ = bus.publish(Event::new(
                EventType::SystemAlarm,
                "system_state_machine",
                EventPayload::Message(format!(
                    "System state changed: {:?} -> {:?}",
                    result.from, result.to
                )),
            ));
            result.triggered_actions.push("Published SystemAlarm event".to_string());
        }

        // Authority escalation in emergency
        if result.to.is_emergency() {
            result.triggered_actions.push(
                "Supervisor agents escalated to Emergency authority".to_string()
            );
        }

        // Auto-recovery in Restoration
        if result.to == SystemOperatingState::Restoration {
            result.triggered_actions.push(
                "Auto-recovery procedures initiated".to_string()
            );
        }
    }

    /// Check if a severity level should trigger a state transition
    pub fn should_escalate(&self, severity: SeverityLevel) -> Option<StateTransitionTrigger> {
        match severity {
            SeverityLevel::Critical => Some(StateTransitionTrigger::CriticalViolation),
            SeverityLevel::Major => {
                // Major violations in Alert state should escalate
                if *self.state.read() == SystemOperatingState::Alert {
                    Some(StateTransitionTrigger::CascadingViolation)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get the shared state reference (for use in AgentContext)
    pub fn state_ref(&self) -> Arc<RwLock<SystemOperatingState>> {
        self.state.clone()
    }
}

impl Default for SystemStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_normal() {
        let sm = SystemStateMachine::new();
        assert_eq!(sm.current_state(), SystemOperatingState::Normal);
    }

    #[test]
    fn test_normal_to_alert() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Alert);
    }

    #[test]
    fn test_alert_to_emergency() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
    }

    #[test]
    fn test_emergency_to_blackout() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::CascadingViolation);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Blackout);
    }

    #[test]
    fn test_blackout_to_restoration() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CascadingViolation);
        let result = sm.transition(StateTransitionTrigger::RestorationInitiated);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Restoration);
    }

    #[test]
    fn test_restoration_to_normal() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CriticalViolation);
        sm.transition(StateTransitionTrigger::CascadingViolation);
        sm.transition(StateTransitionTrigger::RestorationInitiated);
        let result = sm.transition(StateTransitionTrigger::RestorationCompleted);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Normal);
    }

    #[test]
    fn test_stabilized_alert_to_normal() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        let result = sm.transition(StateTransitionTrigger::Stabilized);
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Normal);
    }

    #[test]
    fn test_invalid_transition() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::RestorationInitiated);
        assert!(!result.success);
        assert_eq!(result.to, SystemOperatingState::Normal); // No change
    }

    #[test]
    fn test_manual_override() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
    }

    #[test]
    fn test_should_escalate_critical() {
        let sm = SystemStateMachine::new();
        assert!(sm.should_escalate(SeverityLevel::Critical).is_some());
    }

    #[test]
    fn test_should_escalate_major_in_alert() {
        let sm = SystemStateMachine::new();
        sm.transition(StateTransitionTrigger::CriticalViolation);
        assert!(sm.should_escalate(SeverityLevel::Major).is_some());
    }

    #[test]
    fn test_should_not_escalate_minor() {
        let sm = SystemStateMachine::new();
        assert!(sm.should_escalate(SeverityLevel::Minor).is_none());
    }

    #[test]
    fn test_emergency_triggers_authority_escalation() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::ManualOverride(SystemOperatingState::Emergency));
        assert!(result.triggered_actions.iter().any(|a| a.contains("escalated")));
    }

    #[test]
    fn test_default() {
        let sm = SystemStateMachine::default();
        assert_eq!(sm.current_state(), SystemOperatingState::Normal);
    }

    // ===== BUG-9 收尾：状态机 ↔ 约束引擎联动 =====
    //
    // 以下测试证明：系统状态切换会真正调用 ConstraintEngine::set_emergency_thresholds，
    // 让约束限值随运行模式自动放宽/收紧。此前 on_state_changed 只是 push 一条字符串，
    // 从未调用引擎（BUG-9 的直接断点）。

    /// 进入 Emergency 后，约束引擎的阈值乘数应变 1.5，且 triggered_actions 有记录。
    #[test]
    fn test_state_transition_adjusts_threshold_multiplier() {
        let engine = Arc::new(ConstraintEngine::new());
        let sm = SystemStateMachine::with_event_bus_and_constraints(
            Arc::new(EventBus::new(64)),
            engine.clone(),
        );

        assert!(
            (engine.threshold_multiplier() - 1.0).abs() < 1e-9,
            "Normal 状态乘数应为 1.0"
        );

        // Normal -> Alert -> Emergency
        sm.transition(StateTransitionTrigger::CriticalViolation); // Normal -> Alert
        assert!(
            (engine.threshold_multiplier() - 1.0).abs() < 1e-9,
            "Alert 状态乘数仍应为 1.0"
        );

        let result = sm.transition(StateTransitionTrigger::CriticalViolation); // Alert -> Emergency
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
        assert!(
            (engine.threshold_multiplier() - 1.5).abs() < 1e-9,
            "Emergency 状态乘数应为 1.5，实际 {}",
            engine.threshold_multiplier()
        );
        assert!(
            result
                .triggered_actions
                .iter()
                .any(|a| a.contains("adjusted") && a.contains("Emergency")),
            "triggered_actions 应记录约束阈值调整: {:?}",
            result.triggered_actions
        );
    }

    /// 状态从 Emergency 恢复到 Normal 后，乘数应回到 1.0（无残留放宽）。
    #[test]
    fn test_recovery_restores_normal_thresholds() {
        let engine = Arc::new(ConstraintEngine::new());
        let sm = SystemStateMachine::with_event_bus_and_constraints(
            Arc::new(EventBus::new(64)),
            engine.clone(),
        );

        // 推到 Emergency
        sm.transition(StateTransitionTrigger::CriticalViolation); // Normal -> Alert
        sm.transition(StateTransitionTrigger::CriticalViolation); // Alert -> Emergency
        assert!((engine.threshold_multiplier() - 1.5).abs() < 1e-9);

        // Emergency --Stabilized--> Alert --Stabilized--> Normal
        sm.transition(StateTransitionTrigger::Stabilized); // Emergency -> Alert
        assert!(
            (engine.threshold_multiplier() - 1.0).abs() < 1e-9,
            "Alert 乘数应为 1.0"
        );
        let r = sm.transition(StateTransitionTrigger::Stabilized); // Alert -> Normal
        assert!(r.success);
        assert_eq!(r.to, SystemOperatingState::Normal);
        assert!(
            (engine.threshold_multiplier() - 1.0).abs() < 1e-9,
            "恢复 Normal 后乘数应回到 1.0，无残留放宽，实际 {}",
            engine.threshold_multiplier()
        );
    }

    /// 没有挂载 ConstraintEngine 时（SystemStateMachine::new），状态切换应正常、不 panic，
    /// 且 triggered_actions 不应包含 "adjusted"（降级路径）。
    #[test]
    fn test_no_constraint_engine_still_works() {
        let sm = SystemStateMachine::new();
        let result = sm.transition(StateTransitionTrigger::ManualOverride(
            SystemOperatingState::Emergency,
        ));
        assert!(result.success);
        assert_eq!(result.to, SystemOperatingState::Emergency);
        assert!(
            !result
                .triggered_actions
                .iter()
                .any(|a| a.contains("adjusted")),
            "未挂载引擎时不应有 adjusted 动作: {:?}",
            result.triggered_actions
        );
    }

    /// 端到端硬证据：同一组读数，Normal 下越限、Emergency 下（经状态机联动放宽后）合规。
    /// 这证明联动真正改变了 check_all 的结果，而非仅改了一个内部数字。
    #[test]
    fn test_emergency_relaxation_changes_check_result() {
        use eneros_constraint::{Constraint, ConstraintType};

        let engine = Arc::new(ConstraintEngine::new());

        // 电压约束 0.95-1.05 pu：Emergency ×1.5 后放宽为 (0.925, 1.075)
        let mut v = Constraint::new(
            "v".to_string(),
            "Voltage".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        v.element_ids = vec![1];
        engine.register(v);

        // 热约束 0-100%：Emergency ×1.5 后上限变 150
        let mut t = Constraint::new(
            "t".to_string(),
            "Thermal".to_string(),
            ConstraintType::Thermal,
            0.0,
            100.0,
        );
        t.element_ids = vec![10];
        engine.register(t);

        let sm = SystemStateMachine::with_event_bus_and_constraints(
            Arc::new(EventBus::new(64)),
            engine.clone(),
        );

        // 读数：电压 0.93 pu（低于 0.95 但高于放宽后的 0.925）、载荷 120%（高于 100 但低于放宽后的 150）
        // Normal 状态：应同时检出电压 + 热两个越限
        let violations_normal = engine.check_all(&[(1, 0.93)], &[(10, 120.0)], 50.0);
        assert_eq!(
            violations_normal.len(),
            2,
            "Normal 下 0.93pu/120% 应有 2 个越限（电压+热）"
        );

        // 联动：状态机进 Emergency，引擎阈值自动放宽
        sm.transition(StateTransitionTrigger::CriticalViolation); // Normal -> Alert
        sm.transition(StateTransitionTrigger::CriticalViolation); // Alert -> Emergency
        assert!((engine.threshold_multiplier() - 1.5).abs() < 1e-9);

        // 同样读数，Emergency 下应全部合规
        let violations_emergency = engine.check_all(&[(1, 0.93)], &[(10, 120.0)], 50.0);
        assert!(
            violations_emergency.is_empty(),
            "Emergency 放宽后 0.93pu/120% 应全部合规，但仍有 {:?}",
            violations_emergency
        );

        // 恢复 Normal，越限应再次出现（证明放宽可逆，不是一次性副作用）
        sm.transition(StateTransitionTrigger::Stabilized); // Emergency -> Alert
        sm.transition(StateTransitionTrigger::Stabilized); // Alert -> Normal
        let violations_restored = engine.check_all(&[(1, 0.93)], &[(10, 120.0)], 50.0);
        assert_eq!(
            violations_restored.len(),
            2,
            "恢复 Normal 后越限应再次出现，实际 {:?}",
            violations_restored
        );
    }
}
