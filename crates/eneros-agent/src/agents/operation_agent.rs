use std::time::Duration;
use std::collections::HashMap;
use eneros_core::{AuthorityLevel, Jurisdiction, ZoneId, ElementId, SeverityLevel};
use eneros_eventbus::{Event, event::{EventType, EventPayload}};
use eneros_reasoning::ReasoningInput;
use crate::agent::{Agent, AgentType, AgentAction};
use crate::action_mapping::ActionMapper;
use crate::context::AgentContext;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Device health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceHealth {
    Healthy,
    Degraded,
    Warning,
    Critical,
}

impl DeviceHealth {
    pub fn from_score(score: f64) -> Self {
        if score >= 0.8 { DeviceHealth::Healthy }
        else if score >= 0.6 { DeviceHealth::Degraded }
        else if score >= 0.4 { DeviceHealth::Warning }
        else { DeviceHealth::Critical }
    }
}

/// Fault diagnosis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultDiagnosis {
    pub fault_type: String,
    pub affected_devices: Vec<ElementId>,
    pub cause: String,
    pub severity: SeverityLevel,
    pub recommendation: String,
    pub confidence: f64,
}

/// Device health record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHealthRecord {
    pub device_id: ElementId,
    pub device_name: String,
    pub health: DeviceHealth,
    pub health_score: f64,
    pub last_check_timestamp: u64,
    pub issues: Vec<String>,
}

/// Fault symptom pattern for causal reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultPattern {
    pub pattern_id: String,
    pub symptoms: Vec<String>,
    pub fault_type: String,
    pub cause: String,
    pub recommendation: String,
    pub severity: SeverityLevel,
}

/// Operation Agent — handles fault diagnosis, device health, and maintenance
pub struct OperationAgent {
    id: String,
    name: String,
    jurisdiction: Jurisdiction,
    device_health: HashMap<ElementId, DeviceHealthRecord>,
    fault_patterns: Vec<FaultPattern>,
}

impl OperationAgent {
    pub fn new(id: &str, name: &str, zone_ids: Vec<ZoneId>) -> Self {
        let mut agent = Self {
            id: id.to_string(),
            name: name.to_string(),
            jurisdiction: Jurisdiction::for_zones(zone_ids),
            device_health: HashMap::new(),
            fault_patterns: Vec::new(),
        };
        agent.add_builtin_fault_patterns();
        agent
    }

    fn add_builtin_fault_patterns(&mut self) {
        // Transformer fault patterns
        self.fault_patterns.push(FaultPattern {
            pattern_id: "TX_OVERTEMP".to_string(),
            symptoms: vec!["high_temperature".to_string(), "overload".to_string()],
            fault_type: "transformer_overheating".to_string(),
            cause: "变压器过载导致温升过高".to_string(),
            recommendation: "降低负荷或投入备用变压器".to_string(),
            severity: SeverityLevel::Major,
        });

        self.fault_patterns.push(FaultPattern {
            pattern_id: "LINE_OVERLOAD".to_string(),
            symptoms: vec!["high_loading".to_string(), "thermal_alarm".to_string()],
            fault_type: "line_thermal_overload".to_string(),
            cause: "线路潮流超过热稳定极限".to_string(),
            recommendation: "转移负荷或调整拓扑".to_string(),
            severity: SeverityLevel::Major,
        });

        self.fault_patterns.push(FaultPattern {
            pattern_id: "CB_FAILURE".to_string(),
            symptoms: vec!["operation_failure".to_string(), "no_position_change".to_string()],
            fault_type: "breaker_malfunction".to_string(),
            cause: "断路器操作机构故障".to_string(),
            recommendation: "闭锁断路器，安排检修".to_string(),
            severity: SeverityLevel::Critical,
        });

        self.fault_patterns.push(FaultPattern {
            pattern_id: "CAP_BANK_FAULT".to_string(),
            symptoms: vec!["unbalance".to_string(), "overcurrent".to_string()],
            fault_type: "capacitor_bank_failure".to_string(),
            cause: "电容器组内部故障".to_string(),
            recommendation: "退出故障电容器组，更换损坏单元".to_string(),
            severity: SeverityLevel::Minor,
        });
    }

    /// Diagnose fault based on symptoms
    pub fn diagnose(&self, symptoms: &[String]) -> Vec<FaultDiagnosis> {
        let mut diagnoses = Vec::new();

        for pattern in &self.fault_patterns {
            let match_count = pattern.symptoms.iter()
                .filter(|s| symptoms.iter().any(|obs| obs.contains(s.as_str()) || s.contains(obs.as_str())))
                .count();

            if match_count > 0 {
                let confidence = match_count as f64 / pattern.symptoms.len() as f64;
                diagnoses.push(FaultDiagnosis {
                    fault_type: pattern.fault_type.clone(),
                    affected_devices: Vec::new(),
                    cause: pattern.cause.clone(),
                    severity: pattern.severity,
                    recommendation: pattern.recommendation.clone(),
                    confidence,
                });
            }
        }

        // Sort by confidence descending
        diagnoses.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        diagnoses
    }

    /// Update device health record
    pub fn update_device_health(&mut self, device_id: ElementId, name: &str, score: f64, issues: Vec<String>) {
        let health = DeviceHealth::from_score(score);
        self.device_health.insert(device_id, DeviceHealthRecord {
            device_id,
            device_name: name.to_string(),
            health,
            health_score: score,
            last_check_timestamp: 0,
            issues,
        });
    }

    /// Diagnose fault using reasoning engine for complex or unknown symptoms.
    /// Falls back gracefully if the reasoning engine fails.
    async fn diagnose_with_reasoning(
        &self,
        symptoms: &[String],
        ctx: &AgentContext,
    ) -> Vec<AgentAction> {
        let mut input = ReasoningInput::new("故障诊断");
        for symptom in symptoms {
            input = input.with_observation(symptom);
        }
        input = input.with_constraint("必须识别故障类型和受影响设备");
        input = input.with_constraint("必须给出安全可行的处置建议");

        match ctx.remote.reasoning.as_ref() {
            Some(r) => match r.reason(input).await {
                Ok(output) => {
                    info!(
                        conclusion = %output.conclusion,
                        confidence = output.confidence,
                        "Reasoning engine diagnosis result"
                    );
                    let mut actions = vec![AgentAction::LogMessage(format!(
                        "LLM推理诊断: {} (置信度:{:.0}%), 推理链: {:?}",
                        output.conclusion, output.confidence * 100.0, output.reasoning_chain
                    ))];
                    let mapped = ActionMapper::map_reasoning_output(&output);
                    actions.extend(mapped);
                    actions
                }
                Err(e) => {
                    warn!(error = %e, "Reasoning engine diagnosis failed, using hardcoded patterns only");
                    Vec::new()
                }
            },
            None => {
                warn!("No reasoning engine configured, using hardcoded patterns only");
                Vec::new()
            }
        }
    }

    /// Get devices with health issues
    pub fn unhealthy_devices(&self) -> Vec<&DeviceHealthRecord> {
        self.device_health.values()
            .filter(|d| d.health != DeviceHealth::Healthy)
            .collect()
    }
}

#[async_trait::async_trait]
impl Agent for OperationAgent {
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn agent_type(&self) -> AgentType { AgentType::Operator }
    fn authority_level(&self) -> AuthorityLevel { AuthorityLevel::Operator }
    fn jurisdiction(&self) -> Jurisdiction { self.jurisdiction.clone() }
    fn tick_interval(&self) -> Duration { Duration::from_secs(60) }

    async fn handle_event(&mut self, event: &Event, ctx: &AgentContext) -> eneros_core::Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        match event.event_type {
            EventType::ConstraintViolation | EventType::SystemAlarm => {
                // Extract symptoms from event payload
                let message = match &event.payload {
                    EventPayload::ConstraintViolation {
                        constraint_id, element_id, actual_value, limit_value, severity, ..
                    } => format!(
                        "constraint_violation {} element_{} actual_{:.4} limit_{:.4} severity_{}",
                        constraint_id, element_id, actual_value, limit_value, severity
                    ),
                    EventPayload::Message(msg) => msg.clone(),
                    _ => String::new(),
                };

                let symptoms: Vec<String> = message.split_whitespace()
                    .map(|s| s.to_lowercase())
                    .collect();

                // Step 1: Fast path — hardcoded pattern matching
                let diagnoses = self.diagnose(&symptoms);
                let max_confidence = diagnoses.iter().map(|d| d.confidence).fold(0.0_f64, f64::max);

                for diag in &diagnoses {
                    actions.push(AgentAction::LogMessage(format!(
                        "故障诊断: {} (置信度:{:.0}%), 原因: {}, 建议: {}",
                        diag.fault_type, diag.confidence * 100.0, diag.cause, diag.recommendation
                    )));
                }

                // Step 2: If low confidence or no match, use reasoning engine for deeper analysis
                if max_confidence < 0.5 || diagnoses.is_empty() {
                    let reasoning_actions = self.diagnose_with_reasoning(&symptoms, ctx).await;
                    actions.extend(reasoning_actions);
                }
            }
            _ => {}
        }

        Ok(actions)
    }

    async fn tick(&mut self, _ctx: &AgentContext) -> eneros_core::Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        // Check unhealthy devices
        let unhealthy = self.unhealthy_devices();
        for device in unhealthy {
            actions.push(AgentAction::LogMessage(format!(
                "设备健康预警: {} ({:?}, 得分:{:.2}), 问题: {:?}",
                device.device_name, device.health, device.health_score, device.issues
            )));
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_health_from_score() {
        assert_eq!(DeviceHealth::from_score(0.9), DeviceHealth::Healthy);
        assert_eq!(DeviceHealth::from_score(0.7), DeviceHealth::Degraded);
        assert_eq!(DeviceHealth::from_score(0.5), DeviceHealth::Warning);
        assert_eq!(DeviceHealth::from_score(0.3), DeviceHealth::Critical);
    }

    #[test]
    fn test_diagnose_transformer_overheating() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1]);
        let symptoms = vec!["high_temperature".to_string(), "overload".to_string()];
        let diagnoses = agent.diagnose(&symptoms);
        assert!(!diagnoses.is_empty());
        assert_eq!(diagnoses[0].fault_type, "transformer_overheating");
    }

    #[test]
    fn test_diagnose_line_overload() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1]);
        let symptoms = vec!["high_loading".to_string(), "thermal_alarm".to_string()];
        let diagnoses = agent.diagnose(&symptoms);
        assert!(!diagnoses.is_empty());
        assert!(diagnoses[0].fault_type.contains("line"));
    }

    #[test]
    fn test_diagnose_no_match() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1]);
        let symptoms = vec!["unknown_symptom".to_string()];
        let diagnoses = agent.diagnose(&symptoms);
        assert!(diagnoses.is_empty());
    }

    #[test]
    fn test_update_device_health() {
        let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
        agent.update_device_health(1, "变压器T1", 0.5, vec!["温度偏高".to_string()]);
        assert_eq!(agent.device_health.len(), 1);
        assert_eq!(agent.device_health[&1].health, DeviceHealth::Warning);
    }

    #[test]
    fn test_unhealthy_devices() {
        let mut agent = OperationAgent::new("op1", "Op-1", vec![1]);
        agent.update_device_health(1, "T1", 0.9, vec![]); // Healthy
        agent.update_device_health(2, "T2", 0.5, vec!["问题".to_string()]); // Warning
        let unhealthy = agent.unhealthy_devices();
        assert_eq!(unhealthy.len(), 1);
    }

    #[test]
    fn test_operation_agent_new() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1, 2]);
        assert_eq!(agent.id(), "op1");
        assert_eq!(agent.agent_type(), AgentType::Operator);
        assert_eq!(agent.authority_level(), AuthorityLevel::Operator);
    }

    #[test]
    fn test_operation_agent_tick_interval() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1]);
        assert_eq!(agent.tick_interval(), Duration::from_secs(60));
    }

    #[test]
    fn test_builtin_fault_patterns() {
        let agent = OperationAgent::new("op1", "Op-1", vec![1]);
        assert_eq!(agent.fault_patterns.len(), 4);
    }
}
