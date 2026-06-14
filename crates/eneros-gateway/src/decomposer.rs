use eneros_core::StructuredAction;
use crate::pipeline_types::{
    ActionStep, DecomposedAction, RollbackPlan, RollbackStep,
};
#[cfg(test)]
use crate::pipeline_types::RollbackStrategy;

/// Action decomposer — breaks composite actions into ordered sequences of atomic steps.
///
/// Some StructuredActions are inherently multi-step (e.g., IsolateFault requires
/// opening two switches in sequence). The decomposer produces an ordered list
/// of steps and a corresponding rollback plan.
pub struct ActionDecomposer;

impl ActionDecomposer {
    /// Decompose a StructuredAction into ordered steps.
    /// Returns a DecomposedAction with a single step if no decomposition is needed.
    pub fn decompose(action: &StructuredAction) -> DecomposedAction {
        match action {
            StructuredAction::IsolateFault {
                upstream_switch,
                downstream_switch,
            } => Self::decompose_isolate_fault(*upstream_switch, *downstream_switch),

            StructuredAction::StartGenerator { gen_id, target_mw } => {
                DecomposedAction::single(StructuredAction::StartGenerator {
                    gen_id: *gen_id,
                    target_mw: *target_mw,
                })
            }

            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                // If amount is large, decompose into stages
                if *amount_mw > 100.0 {
                    Self::decompose_shed_load(*zone_id, *amount_mw)
                } else {
                    DecomposedAction::single(StructuredAction::ShedLoad {
                        zone_id: *zone_id,
                        amount_mw: *amount_mw,
                    })
                }
            }

            StructuredAction::ExecuteDevice { device_id, operation, value } => {
                DecomposedAction::single(StructuredAction::ExecuteDevice {
                    device_id: *device_id,
                    operation: operation.clone(),
                    value: *value,
                })
            }

            StructuredAction::CloseTieSwitch { switch_id } => {
                DecomposedAction::single(StructuredAction::CloseTieSwitch {
                    switch_id: *switch_id,
                })
            }

            StructuredAction::NotifyAgent { agent_id, message } => {
                DecomposedAction::single(StructuredAction::NotifyAgent {
                    agent_id: agent_id.clone(),
                    message: message.clone(),
                })
            }
        }
    }

    /// Generate a rollback plan for a decomposed action
    pub fn rollback_plan(decomposed: &DecomposedAction) -> RollbackPlan {
        if !decomposed.atomic && decomposed.steps.len() == 1 {
            // Single-step, non-atomic — simple rollback
            return RollbackPlan::manual_only();
        }

        let mut rollback_steps = Vec::new();

        for step in decomposed.steps.iter().rev() {
            if let Some(undo) = Self::inverse_action(&step.action) {
                rollback_steps.push(RollbackStep {
                    undo_action: undo,
                    description: format!("Rollback step {}", step.step_index),
                    for_step_index: step.step_index,
                });
            }
        }

        if rollback_steps.is_empty() {
            RollbackPlan::manual_only()
        } else {
            RollbackPlan::full_rollback(rollback_steps)
        }
    }

    /// Decompose IsolateFault into two sequential switch operations
    fn decompose_isolate_fault(upstream: u64, downstream: u64) -> DecomposedAction {
        let original = StructuredAction::IsolateFault {
            upstream_switch: upstream,
            downstream_switch: downstream,
        };

        DecomposedAction {
            original,
            steps: vec![
                ActionStep {
                    step_index: 0,
                    action: StructuredAction::ExecuteDevice {
                        device_id: upstream,
                        operation: "open".to_string(),
                        value: 0.0,
                    },
                    description: format!("Open upstream switch {}", upstream),
                    critical: true,
                    estimated_duration_ms: 50,
                },
                ActionStep {
                    step_index: 1,
                    action: StructuredAction::ExecuteDevice {
                        device_id: downstream,
                        operation: "open".to_string(),
                        value: 0.0,
                    },
                    description: format!("Open downstream switch {}", downstream),
                    critical: true,
                    estimated_duration_ms: 50,
                },
            ],
            atomic: true,
            description: format!(
                "Fault isolation: open upstream switch {} then downstream switch {}",
                upstream, downstream
            ),
        }
    }

    /// Decompose large load shedding into stages (50% first, then remainder)
    fn decompose_shed_load(zone_id: u32, amount_mw: f64) -> DecomposedAction {
        let first_stage = (amount_mw * 0.5).max(1.0);
        let second_stage = (amount_mw - first_stage).max(0.0);

        let original = StructuredAction::ShedLoad { zone_id, amount_mw };

        let mut steps = vec![ActionStep {
            step_index: 0,
            action: StructuredAction::ShedLoad {
                zone_id,
                amount_mw: first_stage,
            },
            description: format!("Stage 1: shed {:.1}MW from zone {}", first_stage, zone_id),
            critical: true,
            estimated_duration_ms: 200,
        }];

        if second_stage > 0.0 {
            steps.push(ActionStep {
                step_index: 1,
                action: StructuredAction::ShedLoad {
                    zone_id,
                    amount_mw: second_stage,
                },
                description: format!("Stage 2: shed {:.1}MW from zone {}", second_stage, zone_id),
                critical: false,
                estimated_duration_ms: 200,
            });
        }

        DecomposedAction {
            original,
            steps,
            atomic: false,
            description: format!(
                "Staged load shedding: {:.1}MW in {} stages from zone {}",
                amount_mw,
                if second_stage > 0.0 { 2 } else { 1 },
                zone_id
            ),
        }
    }

    /// Compute the inverse action for rollback
    fn inverse_action(action: &StructuredAction) -> Option<StructuredAction> {
        match action {
            StructuredAction::ExecuteDevice { device_id, operation, .. } => {
                let inverse_op = if operation == "open" || operation == "分闸" {
                    "close"
                } else if operation == "close" || operation == "合闸" {
                    "open"
                } else {
                    return None;
                };
                Some(StructuredAction::ExecuteDevice {
                    device_id: *device_id,
                    operation: inverse_op.to_string(),
                    value: 0.0,
                })
            }
            StructuredAction::StartGenerator { gen_id, .. } => {
                // Rollback: set generator to 0 MW
                Some(StructuredAction::StartGenerator {
                    gen_id: *gen_id,
                    target_mw: 0.0,
                })
            }
            StructuredAction::ShedLoad { zone_id: _, .. } => {
                // Cannot "un-shed" load — this is a manual operation
                None
            }
            StructuredAction::CloseTieSwitch { switch_id } => {
                // Rollback: open the tie switch
                Some(StructuredAction::ExecuteDevice {
                    device_id: *switch_id,
                    operation: "open".to_string(),
                    value: 0.0,
                })
            }
            StructuredAction::IsolateFault { .. } => {
                // Cannot auto-rollback fault isolation
                None
            }
            StructuredAction::NotifyAgent { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_single_action() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let decomposed = ActionDecomposer::decompose(&action);
        assert!(!decomposed.is_multi_step());
        assert_eq!(decomposed.step_count(), 1);
    }

    #[test]
    fn test_decompose_isolate_fault() {
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let decomposed = ActionDecomposer::decompose(&action);
        assert!(decomposed.is_multi_step());
        assert_eq!(decomposed.step_count(), 2);
        assert!(decomposed.atomic);
        assert_eq!(decomposed.steps[0].step_index, 0);
        assert_eq!(decomposed.steps[1].step_index, 1);

        // Verify step actions
        match &decomposed.steps[0].action {
            StructuredAction::ExecuteDevice { device_id, operation, .. } => {
                assert_eq!(*device_id, 10);
                assert_eq!(operation, "open");
            }
            _ => panic!("Expected ExecuteDevice"),
        }
        match &decomposed.steps[1].action {
            StructuredAction::ExecuteDevice { device_id, operation, .. } => {
                assert_eq!(*device_id, 20);
                assert_eq!(operation, "open");
            }
            _ => panic!("Expected ExecuteDevice"),
        }
    }

    #[test]
    fn test_decompose_large_shed_load() {
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 200.0 };
        let decomposed = ActionDecomposer::decompose(&action);
        assert!(decomposed.is_multi_step());
        assert_eq!(decomposed.step_count(), 2);
        assert!(!decomposed.atomic);
    }

    #[test]
    fn test_decompose_small_shed_load() {
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let decomposed = ActionDecomposer::decompose(&action);
        assert!(!decomposed.is_multi_step());
        assert_eq!(decomposed.step_count(), 1);
    }

    #[test]
    fn test_rollback_plan_isolate_fault() {
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let decomposed = ActionDecomposer::decompose(&action);
        let rollback = ActionDecomposer::rollback_plan(&decomposed);
        assert!(rollback.can_auto_rollback());
        assert_eq!(rollback.steps.len(), 2);
        assert_eq!(rollback.strategy, RollbackStrategy::FullRollback);

        // Rollback steps should be in reverse order
        assert_eq!(rollback.steps[0].for_step_index, 1); // downstream first
        assert_eq!(rollback.steps[1].for_step_index, 0); // upstream second
    }

    #[test]
    fn test_rollback_plan_single_action() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 };
        let decomposed = ActionDecomposer::decompose(&action);
        let rollback = ActionDecomposer::rollback_plan(&decomposed);
        assert!(!rollback.can_auto_rollback()); // manual_only for single non-atomic
    }

    #[test]
    fn test_rollback_plan_close_tie() {
        let action = StructuredAction::CloseTieSwitch { switch_id: 30 };
        let decomposed = ActionDecomposer::decompose(&action);
        let rollback = ActionDecomposer::rollback_plan(&decomposed);
        // Single step, non-atomic → manual_only
        assert!(!rollback.can_auto_rollback());
    }

    #[test]
    fn test_rollback_from_step() {
        let action = StructuredAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let decomposed = ActionDecomposer::decompose(&action);
        let rollback = ActionDecomposer::rollback_plan(&decomposed);

        // If step 1 (downstream) fails, rollback step 0 (upstream)
        let steps = rollback.rollback_from(1);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].for_step_index, 0);
    }

    #[test]
    fn test_inverse_action_open_switch() {
        let action = StructuredAction::ExecuteDevice {
            device_id: 10,
            operation: "open".to_string(),
            value: 0.0,
        };
        let inverse = ActionDecomposer::inverse_action(&action);
        assert!(inverse.is_some());
        match inverse.unwrap() {
            StructuredAction::ExecuteDevice { device_id, operation, .. } => {
                assert_eq!(device_id, 10);
                assert_eq!(operation, "close");
            }
            _ => panic!("Expected ExecuteDevice"),
        }
    }

    #[test]
    fn test_inverse_action_shed_load_none() {
        let action = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let inverse = ActionDecomposer::inverse_action(&action);
        assert!(inverse.is_none()); // Cannot auto-un-shed
    }

    #[test]
    fn test_decompose_notify_agent() {
        let action = StructuredAction::NotifyAgent {
            agent_id: "dispatch".to_string(),
            message: "test".to_string(),
        };
        let decomposed = ActionDecomposer::decompose(&action);
        assert!(!decomposed.is_multi_step());
    }
}
