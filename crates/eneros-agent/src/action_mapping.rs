use eneros_core::{ElementId, StructuredAction};
use eneros_gateway::command::{Command, CommandType, CommandPriority};
use eneros_eventbus::{Event, event::{EventType, EventPayload}};
use eneros_reasoning::engine::ReasoningOutput;
use crate::agent::AgentAction;
use serde::{Deserialize, Serialize};

/// Emergency action — structured, executable action for emergency response plans
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmergencyAction {
    /// Execute a device operation
    ExecuteDevice {
        device_id: ElementId,
        operation: String,
        params: std::collections::HashMap<String, f64>,
    },
    /// Notify another agent
    NotifyAgent {
        agent_id: String,
        message: String,
    },
    /// Shed load in a zone
    ShedLoad {
        zone_id: u32,
        amount_mw: f64,
    },
    /// Start or adjust a generator
    StartGenerator {
        gen_id: ElementId,
        target_mw: f64,
    },
    /// Isolate a fault section
    IsolateFault {
        upstream_switch: ElementId,
        downstream_switch: ElementId,
    },
    /// Close a tie switch for restoration
    CloseTieSwitch {
        switch_id: ElementId,
    },
}

impl From<&StructuredAction> for EmergencyAction {
    fn from(action: &StructuredAction) -> Self {
        match action {
            StructuredAction::ExecuteDevice { device_id, operation, value } => {
                let mut params = std::collections::HashMap::new();
                params.insert("value".to_string(), *value);
                EmergencyAction::ExecuteDevice { device_id: *device_id, operation: operation.clone(), params }
            }
            StructuredAction::ShedLoad { zone_id, amount_mw } => {
                EmergencyAction::ShedLoad { zone_id: *zone_id, amount_mw: *amount_mw }
            }
            StructuredAction::StartGenerator { gen_id, target_mw } => {
                EmergencyAction::StartGenerator { gen_id: *gen_id, target_mw: *target_mw }
            }
            StructuredAction::NotifyAgent { agent_id, message } => {
                EmergencyAction::NotifyAgent { agent_id: agent_id.clone(), message: message.clone() }
            }
            StructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
                EmergencyAction::IsolateFault { upstream_switch: *upstream_switch, downstream_switch: *downstream_switch }
            }
            StructuredAction::CloseTieSwitch { switch_id } => {
                EmergencyAction::CloseTieSwitch { switch_id: *switch_id }
            }
        }
    }
}

/// Action mapper — converts EmergencyAction and ReasoningOutput to AgentAction
pub struct ActionMapper;

impl ActionMapper {
    /// Map an EmergencyAction to an AgentAction
    pub fn map_emergency_action(action: &EmergencyAction) -> AgentAction {
        match action {
            EmergencyAction::ExecuteDevice { device_id, operation, params } => {
                let value = params.values().next().copied().unwrap_or(0.0);
                let cmd = Command::new(
                    CommandType::SwitchOperation,
                    *device_id,
                    CommandPriority::High,
                    "action_mapper",
                )
                .with_parameter(operation, value);
                AgentAction::ExecuteCommand(cmd)
            }
            EmergencyAction::NotifyAgent { agent_id, message } => {
                AgentAction::DelegateTask {
                    target_agent_id: agent_id.clone(),
                    task_description: message.clone(),
                }
            }
            EmergencyAction::ShedLoad { zone_id, amount_mw } => {
                let cmd = Command::new(
                    CommandType::LoadShedding,
                    *zone_id as ElementId,
                    CommandPriority::Critical,
                    "action_mapper",
                )
                .with_parameter("amount_mw", *amount_mw);
                AgentAction::ExecuteCommand(cmd)
            }
            EmergencyAction::StartGenerator { gen_id, target_mw } => {
                let cmd = Command::new(
                    CommandType::GeneratorSetpoint,
                    *gen_id,
                    CommandPriority::High,
                    "action_mapper",
                )
                .with_parameter("P", *target_mw);
                AgentAction::ExecuteCommand(cmd)
            }
            EmergencyAction::IsolateFault { upstream_switch, downstream_switch } => {
                // Return as an emergency override with a composite command
                let cmd = Command::new(
                    CommandType::SwitchToggle,
                    *upstream_switch,
                    CommandPriority::Critical,
                    "action_mapper",
                )
                .with_parameter("downstream", *downstream_switch as f64)
                .with_parameter("open", 1.0);
                AgentAction::EmergencyOverride {
                    action: Box::new(AgentAction::ExecuteCommand(cmd)),
                    justification: "Fault isolation".to_string(),
                }
            }
            EmergencyAction::CloseTieSwitch { switch_id } => {
                let cmd = Command::new(
                    CommandType::SwitchToggle,
                    *switch_id,
                    CommandPriority::High,
                    "action_mapper",
                )
                .with_parameter("closed", 1.0);
                AgentAction::ExecuteCommand(cmd)
            }
        }
    }

    /// Map a batch of EmergencyActions to AgentActions
    pub fn map_emergency_actions(actions: &[EmergencyAction]) -> Vec<AgentAction> {
        actions.iter().map(Self::map_emergency_action).collect()
    }

    /// Map ReasoningOutput to AgentActions
    /// Attempts to parse action strings; falls back to PublishEvent for unparseable actions
    /// Map a `ReasoningOutput` to executable `AgentAction`s.
    ///
    /// If the reasoning engine produced structured actions (the preferred
    /// Phase 14 path), each one becomes an `AgentAction::ExecuteStructured`
    /// that the orchestrator routes through the `ConstrainedDecisionPipeline`.
    /// Otherwise we fall back to the legacy free-text keyword matching.
    pub fn map_reasoning_output(output: &ReasoningOutput) -> Vec<AgentAction> {
        // Prefer structured actions when available — they bypass the fragile
        // string keyword matcher and carry typed parameters directly.
        if let Some(ref structured) = output.structured_actions {
            if !structured.is_empty() {
                return structured
                    .iter()
                    .map(|sa| AgentAction::ExecuteStructured(sa.clone()))
                    .collect();
            }
        }

        // Legacy path: parse free-text action strings via keyword matching.
        output.actions.iter().map(|action_str| {
            Self::parse_action_string(action_str)
                .unwrap_or_else(|| {
                    // Fallback: publish as event for human review
                    AgentAction::PublishEvent(Event::new(
                        EventType::SystemAlarm,
                        "action_mapper",
                        EventPayload::Message(format!("Unmapped reasoning action: {}", action_str)),
                    ))
                })
        }).collect()
    }

    /// Try to parse an action string into an AgentAction
    fn parse_action_string(s: &str) -> Option<AgentAction> {
        let lower = s.to_lowercase();

        // Pattern: "adjust generator <id> to <value> mw"
        if lower.contains("adjust generator") || lower.contains("调整发电机") {
            let gen_id = Self::extract_number(s).unwrap_or(0);
            let target_mw = Self::extract_mw_value(s).unwrap_or(0.0);
            let cmd = Command::new(
                CommandType::GeneratorSetpoint,
                gen_id,
                CommandPriority::Normal,
                "action_mapper",
            )
            .with_parameter("P", target_mw);
            return Some(AgentAction::ExecuteCommand(cmd));
        }

        // Pattern: "shed load" / "切负荷"
        if lower.contains("shed load") || lower.contains("切负荷") {
            let amount = Self::extract_mw_value(s).unwrap_or(10.0);
            let cmd = Command::new(
                CommandType::LoadShedding,
                0,
                CommandPriority::Critical,
                "action_mapper",
            )
            .with_parameter("amount_mw", amount);
            return Some(AgentAction::ExecuteCommand(cmd));
        }

        // Pattern: "close breaker" / "合断路器"
        if lower.contains("close breaker") || lower.contains("合断路器") {
            let device_id = Self::extract_number(s).unwrap_or(0);
            let cmd = Command::new(
                CommandType::SwitchToggle,
                device_id,
                CommandPriority::Normal,
                "action_mapper",
            )
            .with_parameter("closed", 1.0);
            return Some(AgentAction::ExecuteCommand(cmd));
        }

        // Pattern: "open breaker" / "断断路器"
        if lower.contains("open breaker") || lower.contains("断断路器") || lower.contains("分断路器") {
            let device_id = Self::extract_number(s).unwrap_or(0);
            let cmd = Command::new(
                CommandType::SwitchToggle,
                device_id,
                CommandPriority::Normal,
                "action_mapper",
            )
            .with_parameter("closed", 0.0);
            return Some(AgentAction::ExecuteCommand(cmd));
        }

        // Pattern: "notify" / "通知"
        if lower.contains("notify") || lower.contains("通知") {
            return Some(AgentAction::PublishEvent(Event::new(
                EventType::SystemAlarm,
                "action_mapper",
                EventPayload::Message(s.to_string()),
            )));
        }

        None
    }

    /// Extract a number from a string
    fn extract_number(s: &str) -> Option<u64> {
        for word in s.split_whitespace() {
            if let Ok(n) = word.parse::<u64>() {
                return Some(n);
            }
            // Try stripping non-numeric prefix
            let stripped: String = word.chars().filter(|c| c.is_ascii_digit()).collect();
            if !stripped.is_empty() {
                if let Ok(n) = stripped.parse::<u64>() {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Extract a MW value from a string
    fn extract_mw_value(s: &str) -> Option<f64> {
        for word in s.split_whitespace() {
            if word.ends_with("mw") || word.ends_with("MW") {
                let num_part = word.trim_end_matches("mw").trim_end_matches("MW");
                if let Ok(v) = num_part.parse::<f64>() {
                    return Some(v);
                }
            }
        }
        // Try any number in the string
        for word in s.split_whitespace() {
            if let Ok(v) = word.parse::<f64>() {
                return Some(v);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_reasoning_output_prefers_structured_actions() {
        // When structured_actions is Some, the mapper must produce
        // AgentAction::ExecuteStructured variants — never touching the
        // fragile text keyword path.
        let output = ReasoningOutput {
            conclusion: "undervoltage".to_string(),
            confidence: 0.9,
            actions: vec!["adjust generator 1 to 300MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: Some(vec![
                StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 },
                StructuredAction::ShedLoad { zone_id: 2, amount_mw: 25.0 },
            ]),
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0],
            AgentAction::ExecuteStructured(StructuredAction::StartGenerator { .. })
        ));
        assert!(matches!(
            results[1],
            AgentAction::ExecuteStructured(StructuredAction::ShedLoad { .. })
        ));
    }

    #[test]
    fn test_map_reasoning_output_falls_back_when_structured_empty() {
        // structured_actions present but empty → must use the legacy text path.
        let output = ReasoningOutput {
            conclusion: "adjust".to_string(),
            confidence: 0.8,
            actions: vec!["adjust generator 1 to 100MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: Some(vec![]),
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_reasoning_output_falls_back_when_structured_none() {
        // structured_actions is None → legacy text path (existing behavior).
        let output = ReasoningOutput {
            conclusion: "adjust".to_string(),
            confidence: 0.8,
            actions: vec!["adjust generator 1 to 100MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: None,
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_emergency_action_execute_device() {
        let mut params = std::collections::HashMap::new();
        params.insert("value".to_string(), 100.0);
        let action = EmergencyAction::ExecuteDevice {
            device_id: 1,
            operation: "close".to_string(),
            params,
        };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_emergency_action_notify_agent() {
        let action = EmergencyAction::NotifyAgent {
            agent_id: "dispatch-1".to_string(),
            message: "fault isolated".to_string(),
        };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::DelegateTask { .. }));
    }

    #[test]
    fn test_map_emergency_action_shed_load() {
        let action = EmergencyAction::ShedLoad {
            zone_id: 1,
            amount_mw: 50.0,
        };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_emergency_action_start_generator() {
        let action = EmergencyAction::StartGenerator {
            gen_id: 5,
            target_mw: 200.0,
        };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_emergency_action_isolate_fault() {
        let action = EmergencyAction::IsolateFault {
            upstream_switch: 10,
            downstream_switch: 20,
        };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::EmergencyOverride { .. }));
    }

    #[test]
    fn test_map_emergency_action_close_tie_switch() {
        let action = EmergencyAction::CloseTieSwitch { switch_id: 30 };
        let result = ActionMapper::map_emergency_action(&action);
        assert!(matches!(result, AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_emergency_actions_batch() {
        let actions = vec![
            EmergencyAction::ShedLoad { zone_id: 1, amount_mw: 50.0 },
            EmergencyAction::StartGenerator { gen_id: 5, target_mw: 200.0 },
        ];
        let results = ActionMapper::map_emergency_actions(&actions);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_map_reasoning_output_adjust_generator() {
        let output = ReasoningOutput {
            conclusion: "adjust gen".to_string(),
            confidence: 0.9,
            actions: vec!["adjust generator 1 to 100MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: None,
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_reasoning_output_shed_load() {
        let output = ReasoningOutput {
            conclusion: "shed load".to_string(),
            confidence: 0.8,
            actions: vec!["shed load 50MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: None,
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentAction::ExecuteCommand(_)));
    }

    #[test]
    fn test_map_reasoning_output_unmapped_fallback() {
        let output = ReasoningOutput {
            conclusion: "unknown".to_string(),
            confidence: 0.5,
            actions: vec!["do something complicated".to_string()],
            reasoning_chain: vec![],
            structured_actions: None,
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], AgentAction::PublishEvent(_)));
    }

    #[test]
    fn test_map_reasoning_output_chinese() {
        let output = ReasoningOutput {
            conclusion: "切负荷".to_string(),
            confidence: 0.9,
            actions: vec!["切负荷 30MW".to_string()],
            reasoning_chain: vec![],
            structured_actions: None,
            preconditions: vec![],
        };
        let results = ActionMapper::map_reasoning_output(&output);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_extract_number() {
        assert_eq!(ActionMapper::extract_number("generator 5"), Some(5));
        assert_eq!(ActionMapper::extract_number("gen_10"), Some(10));
    }

    #[test]
    fn test_extract_mw_value() {
        assert_eq!(ActionMapper::extract_mw_value("to 100MW"), Some(100.0));
        assert_eq!(ActionMapper::extract_mw_value("50MW"), Some(50.0));
    }

    // === StructuredAction → EmergencyAction conversion tests ===

    #[test]
    fn test_structured_action_to_emergency_action_execute_device() {
        let sa = StructuredAction::ExecuteDevice { device_id: 1, operation: "close".to_string(), value: 1.0 };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::ExecuteDevice { device_id, operation, params } => {
                assert_eq!(device_id, 1);
                assert_eq!(operation, "close");
                assert_eq!(params.get("value"), Some(&1.0));
            }
            _ => panic!("Expected ExecuteDevice"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_shed_load() {
        let sa = StructuredAction::ShedLoad { zone_id: 2, amount_mw: 50.0 };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::ShedLoad { zone_id, amount_mw } => {
                assert_eq!(zone_id, 2);
                assert_eq!(amount_mw, 50.0);
            }
            _ => panic!("Expected ShedLoad"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_start_generator() {
        let sa = StructuredAction::StartGenerator { gen_id: 3, target_mw: 100.0 };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::StartGenerator { gen_id, target_mw } => {
                assert_eq!(gen_id, 3);
                assert_eq!(target_mw, 100.0);
            }
            _ => panic!("Expected StartGenerator"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_notify_agent() {
        let sa = StructuredAction::NotifyAgent {
            agent_id: "dispatch".to_string(),
            message: "紧急切负荷".to_string(),
        };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::NotifyAgent { agent_id, message } => {
                assert_eq!(agent_id, "dispatch");
                assert_eq!(message, "紧急切负荷");
            }
            _ => panic!("Expected NotifyAgent"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_isolate_fault() {
        let sa = StructuredAction::IsolateFault { upstream_switch: 10, downstream_switch: 20 };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::IsolateFault { upstream_switch, downstream_switch } => {
                assert_eq!(upstream_switch, 10);
                assert_eq!(downstream_switch, 20);
            }
            _ => panic!("Expected IsolateFault"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_close_tie_switch() {
        let sa = StructuredAction::CloseTieSwitch { switch_id: 30 };
        let ea: EmergencyAction = (&sa).into();
        match ea {
            EmergencyAction::CloseTieSwitch { switch_id } => {
                assert_eq!(switch_id, 30);
            }
            _ => panic!("Expected CloseTieSwitch"),
        }
    }

    #[test]
    fn test_structured_action_to_emergency_action_to_agent_action() {
        // Full pipeline: StructuredAction → EmergencyAction → AgentAction
        let sa = StructuredAction::ShedLoad { zone_id: 1, amount_mw: 50.0 };
        let ea: EmergencyAction = (&sa).into();
        let agent_action = ActionMapper::map_emergency_action(&ea);
        assert!(matches!(agent_action, AgentAction::ExecuteCommand(_)));
    }
}
