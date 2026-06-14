use eneros_core::{EmergencyResponsePlan, EmergencyTriggerCondition, SystemOperatingState, StructuredAction};
use crate::action_mapping::{ActionMapper, EmergencyAction};
use crate::agent::AgentAction;
use serde::{Deserialize, Serialize};

/// Emergency response execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyResponseResult {
    /// Plan that was executed
    pub plan_id: String,
    /// Whether the response was successfully initiated
    pub success: bool,
    /// Actions that were executed (human-readable descriptions)
    pub executed_actions: Vec<String>,
    /// Safety checks that were bypassed
    pub bypassed_checks: Vec<String>,
    /// Reason for the response
    pub reason: String,
}

/// Emergency response pipeline — executes predefined emergency plans
pub struct EmergencyResponsePipeline {
    /// Registered emergency response plans
    plans: Vec<EmergencyResponsePlan>,
}

impl EmergencyResponsePipeline {
    /// Create a new pipeline with built-in emergency plans
    pub fn new() -> Self {
        let mut pipeline = Self { plans: Vec::new() };
        pipeline.add_builtin_plans();
        pipeline
    }

    /// Create an empty pipeline
    pub fn empty() -> Self {
        Self { plans: Vec::new() }
    }

    /// Add a custom emergency response plan
    pub fn add_plan(&mut self, plan: EmergencyResponsePlan) {
        self.plans.push(plan);
    }

    /// Check if any emergency plan should be triggered given the current conditions
    pub fn check_triggers(
        &self,
        frequency_hz: f64,
        branches_tripped: usize,
        min_voltage_pu: f64,
        buses_below_voltage: usize,
        _system_state: SystemOperatingState,
    ) -> Vec<&EmergencyResponsePlan> {
        self.plans.iter().filter(|plan| {
            match &plan.trigger_condition {
                EmergencyTriggerCondition::FrequencyBelow { threshold_hz } => {
                    frequency_hz < *threshold_hz
                }
                EmergencyTriggerCondition::CascadingFailure { min_branches_tripped } => {
                    branches_tripped >= *min_branches_tripped
                }
                EmergencyTriggerCondition::VoltageCollapse { threshold_pu, min_buses } => {
                    min_voltage_pu < *threshold_pu && buses_below_voltage >= *min_buses
                }
                EmergencyTriggerCondition::Custom(_) => false, // Custom conditions need manual evaluation
            }
        }).collect()
    }

    /// Execute an emergency response plan (backward-compatible, returns human-readable action descriptions)
    pub fn execute(&self, plan: &EmergencyResponsePlan) -> EmergencyResponseResult {
        let executed_actions: Vec<String> = plan.actions.iter().map(|a| format!("{:?}", a)).collect();
        EmergencyResponseResult {
            plan_id: plan.plan_id.clone(),
            success: true,
            executed_actions,
            bypassed_checks: plan.bypass_checks.clone(),
            reason: format!("Emergency plan '{}' triggered", plan.name),
        }
    }

    /// Execute an emergency response plan using ActionMapper to convert StructuredAction → EmergencyAction → AgentAction
    /// This implements the execution closed loop: StructuredAction → EmergencyAction → AgentAction
    pub fn execute_with_mapper(&self, plan: &EmergencyResponsePlan) -> Vec<AgentAction> {
        let emergency_actions: Vec<EmergencyAction> = plan.actions.iter().map(|a| a.into()).collect();
        ActionMapper::map_emergency_actions(&emergency_actions)
    }

    /// Find and execute plans matching current conditions
    pub fn auto_respond(
        &self,
        frequency_hz: f64,
        branches_tripped: usize,
        min_voltage_pu: f64,
        buses_below_voltage: usize,
        system_state: SystemOperatingState,
    ) -> Vec<EmergencyResponseResult> {
        let triggered = self.check_triggers(
            frequency_hz,
            branches_tripped,
            min_voltage_pu,
            buses_below_voltage,
            system_state,
        );

        triggered.iter().map(|plan| self.execute(plan)).collect()
    }

    /// Get all registered plans
    pub fn plans(&self) -> &[EmergencyResponsePlan] {
        &self.plans
    }

    fn add_builtin_plans(&mut self) {
        // Plan 1: Frequency collapse
        self.plans.push(EmergencyResponsePlan {
            plan_id: "BUILTIN_FREQUENCY_COLLAPSE".to_string(),
            name: "频率崩溃紧急响应".to_string(),
            trigger_condition: EmergencyTriggerCondition::FrequencyBelow { threshold_hz: 49.5 },
            actions: vec![
                StructuredAction::ShedLoad { zone_id: 0, amount_mw: 50.0 },
                StructuredAction::StartGenerator { gen_id: 0, target_mw: 100.0 },
                StructuredAction::NotifyAgent { agent_id: "dispatch".to_string(), message: "频率崩溃，已自动切负荷".to_string() },
            ],
            bypass_checks: vec!["approval_flow".to_string(), "jurisdiction_check".to_string()],
            requires_approval: false,
        });

        // Plan 2: Cascading failure
        self.plans.push(EmergencyResponsePlan {
            plan_id: "BUILTIN_CASCADING_FAILURE".to_string(),
            name: "级联故障紧急响应".to_string(),
            trigger_condition: EmergencyTriggerCondition::CascadingFailure { min_branches_tripped: 3 },
            actions: vec![
                StructuredAction::IsolateFault { upstream_switch: 0, downstream_switch: 0 },
                StructuredAction::CloseTieSwitch { switch_id: 0 },
                StructuredAction::NotifyAgent { agent_id: "control_center".to_string(), message: "级联故障，已隔离故障区域".to_string() },
            ],
            bypass_checks: vec!["approval_flow".to_string()],
            requires_approval: false,
        });

        // Plan 3: Voltage collapse
        self.plans.push(EmergencyResponsePlan {
            plan_id: "BUILTIN_VOLTAGE_COLLAPSE".to_string(),
            name: "电压崩溃紧急响应".to_string(),
            trigger_condition: EmergencyTriggerCondition::VoltageCollapse {
                threshold_pu: 0.85,
                min_buses: 3,
            },
            actions: vec![
                StructuredAction::ExecuteDevice { device_id: 0, operation: "activate_reactive_compensation".to_string(), value: 1.0 },
                StructuredAction::ShedLoad { zone_id: 0, amount_mw: 30.0 },
                StructuredAction::ExecuteDevice { device_id: 0, operation: "tap_transformer".to_string(), value: 1.05 },
            ],
            bypass_checks: vec!["approval_flow".to_string(), "jurisdiction_check".to_string()],
            requires_approval: false,
        });
    }
}

impl Default for EmergencyResponsePipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_plans_loaded() {
        let pipeline = EmergencyResponsePipeline::new();
        assert_eq!(pipeline.plans().len(), 3);
    }

    #[test]
    fn test_frequency_collapse_trigger() {
        let pipeline = EmergencyResponsePipeline::new();
        let triggered = pipeline.check_triggers(49.0, 0, 1.0, 0, SystemOperatingState::Emergency);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].plan_id, "BUILTIN_FREQUENCY_COLLAPSE");
    }

    #[test]
    fn test_no_trigger_normal_frequency() {
        let pipeline = EmergencyResponsePipeline::new();
        let triggered = pipeline.check_triggers(50.0, 0, 1.0, 0, SystemOperatingState::Normal);
        assert!(triggered.is_empty());
    }

    #[test]
    fn test_cascading_failure_trigger() {
        let pipeline = EmergencyResponsePipeline::new();
        let triggered = pipeline.check_triggers(50.0, 5, 1.0, 0, SystemOperatingState::Emergency);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].plan_id, "BUILTIN_CASCADING_FAILURE");
    }

    #[test]
    fn test_voltage_collapse_trigger() {
        let pipeline = EmergencyResponsePipeline::new();
        let triggered = pipeline.check_triggers(50.0, 0, 0.80, 5, SystemOperatingState::Emergency);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].plan_id, "BUILTIN_VOLTAGE_COLLAPSE");
    }

    #[test]
    fn test_execute_plan() {
        let pipeline = EmergencyResponsePipeline::new();
        let plan = &pipeline.plans()[0];
        let result = pipeline.execute(plan);
        assert!(result.success);
        assert_eq!(result.executed_actions.len(), 3);
        assert_eq!(result.bypassed_checks.len(), 2);
    }

    #[test]
    fn test_auto_respond() {
        let pipeline = EmergencyResponsePipeline::new();
        let results = pipeline.auto_respond(49.0, 0, 1.0, 0, SystemOperatingState::Emergency);
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[test]
    fn test_auto_respond_no_trigger() {
        let pipeline = EmergencyResponsePipeline::new();
        let results = pipeline.auto_respond(50.0, 0, 1.0, 0, SystemOperatingState::Normal);
        assert!(results.is_empty());
    }

    #[test]
    fn test_custom_plan() {
        let mut pipeline = EmergencyResponsePipeline::empty();
        pipeline.add_plan(EmergencyResponsePlan {
            plan_id: "CUSTOM_1".to_string(),
            name: "Custom Plan".to_string(),
            trigger_condition: EmergencyTriggerCondition::FrequencyBelow { threshold_hz: 49.0 },
            actions: vec![StructuredAction::ShedLoad { zone_id: 1, amount_mw: 20.0 }],
            bypass_checks: vec![],
            requires_approval: true,
        });
        assert_eq!(pipeline.plans().len(), 1);
        let triggered = pipeline.check_triggers(48.5, 0, 1.0, 0, SystemOperatingState::Emergency);
        assert_eq!(triggered.len(), 1);
    }

    #[test]
    fn test_default_pipeline() {
        let pipeline = EmergencyResponsePipeline::default();
        assert_eq!(pipeline.plans().len(), 3);
    }

    #[test]
    fn test_multiple_triggers() {
        let pipeline = EmergencyResponsePipeline::new();
        // Both frequency and cascading failure
        let triggered = pipeline.check_triggers(49.0, 5, 1.0, 0, SystemOperatingState::Emergency);
        assert_eq!(triggered.len(), 2);
    }

    #[test]
    fn test_execute_with_mapper_frequency_collapse() {
        let pipeline = EmergencyResponsePipeline::new();
        let plan = &pipeline.plans()[0]; // BUILTIN_FREQUENCY_COLLAPSE
        let agent_actions = pipeline.execute_with_mapper(plan);
        assert_eq!(agent_actions.len(), 3);
        // ShedLoad → ExecuteCommand
        assert!(matches!(agent_actions[0], AgentAction::ExecuteCommand(_)));
        // StartGenerator → ExecuteCommand
        assert!(matches!(agent_actions[1], AgentAction::ExecuteCommand(_)));
        // NotifyAgent → DelegateTask
        assert!(matches!(agent_actions[2], AgentAction::DelegateTask { .. }));
    }

    #[test]
    fn test_execute_with_mapper_cascading_failure() {
        let pipeline = EmergencyResponsePipeline::new();
        let plan = &pipeline.plans()[1]; // BUILTIN_CASCADING_FAILURE
        let agent_actions = pipeline.execute_with_mapper(plan);
        assert_eq!(agent_actions.len(), 3);
        // IsolateFault → EmergencyOverride
        assert!(matches!(agent_actions[0], AgentAction::EmergencyOverride { .. }));
        // CloseTieSwitch → ExecuteCommand
        assert!(matches!(agent_actions[1], AgentAction::ExecuteCommand(_)));
        // NotifyAgent → DelegateTask
        assert!(matches!(agent_actions[2], AgentAction::DelegateTask { .. }));
    }

    #[test]
    fn test_execute_with_mapper_voltage_collapse() {
        let pipeline = EmergencyResponsePipeline::new();
        let plan = &pipeline.plans()[2]; // BUILTIN_VOLTAGE_COLLAPSE
        let agent_actions = pipeline.execute_with_mapper(plan);
        assert_eq!(agent_actions.len(), 3);
        // ExecuteDevice → ExecuteCommand
        assert!(matches!(agent_actions[0], AgentAction::ExecuteCommand(_)));
        // ShedLoad → ExecuteCommand
        assert!(matches!(agent_actions[1], AgentAction::ExecuteCommand(_)));
        // ExecuteDevice → ExecuteCommand
        assert!(matches!(agent_actions[2], AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_builtin_plans_use_structured_actions() {
        let pipeline = EmergencyResponsePipeline::new();
        for plan in pipeline.plans() {
            assert!(!plan.actions.is_empty(), "Plan {} should have structured actions", plan.plan_id);
        }
        // Verify frequency collapse plan specifically
        let freq_plan = &pipeline.plans()[0];
        assert!(matches!(freq_plan.actions[0], StructuredAction::ShedLoad { .. }));
        assert!(matches!(freq_plan.actions[1], StructuredAction::StartGenerator { .. }));
        assert!(matches!(freq_plan.actions[2], StructuredAction::NotifyAgent { .. }));
    }
}
