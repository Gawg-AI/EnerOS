use std::sync::Arc;
use eneros_core::{ActionVerdict, StructuredAction, Result};
use eneros_constraint::projector::ProjectionResult;
use crate::engine::{ReasoningEngine, ReasoningInput, ReasoningOutput};

/// LLM feedback loop — re-prompts LLM when actions are rejected
pub struct FeedbackLoop {
    /// The reasoning engine to re-prompt. Held as `Arc` so the same engine
    /// instance backing an `AgentContext` (which stores
    /// `Arc<dyn ReasoningEngine>`) can be shared with the loop without
    /// cloning a non-`Clone` engine.
    engine: Arc<dyn ReasoningEngine>,
    /// Maximum number of feedback iterations
    max_iterations: u32,
}

/// Result of a feedback loop iteration
#[derive(Debug, Clone)]
pub struct FeedbackResult {
    /// Final reasoning output (may be from a retry)
    pub output: ReasoningOutput,
    /// Number of retries performed
    pub retries: u32,
    /// Whether the final output was accepted
    pub accepted: bool,
    /// Rejection reasons from each iteration
    pub rejection_history: Vec<String>,
}

impl FeedbackLoop {
    /// Create a new feedback loop sharing an `Arc<dyn ReasoningEngine>`.
    ///
    /// This is the preferred Phase 14 constructor: it matches the storage
    /// form in `AgentContext.reasoning`, so the live reasoning engine can be
    /// reused for re-prompting without cloning.
    pub fn new_shared(engine: Arc<dyn ReasoningEngine>, max_iterations: u32) -> Self {
        Self { engine, max_iterations }
    }

    /// Create with default max iterations (2), sharing an engine via `Arc`.
    pub fn with_default_iterations_shared(engine: Arc<dyn ReasoningEngine>) -> Self {
        Self::new_shared(engine, 2)
    }

    /// Create a new feedback loop taking ownership of a boxed engine.
    ///
    /// The boxed engine is converted into an `Arc` internally. Retained for
    /// backward compatibility with existing callers/tests.
    pub fn new(engine: Box<dyn ReasoningEngine>, max_iterations: u32) -> Self {
        Self { engine: Arc::from(engine), max_iterations }
    }

    /// Create with default max iterations (2), taking ownership of a boxed engine.
    pub fn with_default_iterations(engine: Box<dyn ReasoningEngine>) -> Self {
        Self::new(engine, 2)
    }

    /// Execute reasoning with feedback on rejection
    pub async fn reason_with_feedback(
        &self,
        input: &ReasoningInput,
        rejection_reason: &str,
    ) -> Result<FeedbackResult> {
        let current_input = input.clone();
        let mut rejection_history = vec![rejection_reason.to_string()];
        let mut retries = 0;

        loop {
            // Build feedback-enhanced input
            let feedback_input = self.build_feedback_input(&current_input, &rejection_history);

            // Re-reason
            let output = self.engine.reason(feedback_input).await?;

            // Check if the output has structured actions
            if let Some(ref actions) = output.structured_actions {
                if !actions.is_empty() {
                    return Ok(FeedbackResult {
                        output,
                        retries,
                        accepted: true,
                        rejection_history,
                    });
                }
            }

            // If no structured actions but has text actions, still return
            if !output.actions.is_empty() {
                return Ok(FeedbackResult {
                    output,
                    retries,
                    accepted: true,
                    rejection_history,
                });
            }

            retries += 1;
            if retries >= self.max_iterations {
                return Ok(FeedbackResult {
                    output,
                    retries,
                    accepted: false,
                    rejection_history,
                });
            }

            // Add the LLM's failed attempt to rejection history
            rejection_history.push(format!(
                "Retry {} produced no valid actions: {}",
                retries,
                output.conclusion.chars().take(100).collect::<String>()
            ));
        }
    }

    /// Build a feedback-enhanced ReasoningInput
    fn build_feedback_input(
        &self,
        original: &ReasoningInput,
        rejection_history: &[String],
    ) -> ReasoningInput {
        let feedback_observation = format!(
            "[约束反馈] 你之前的建议被拒绝：\n{}\n请基于以上约束信息重新推理，给出满足所有物理约束的动作建议。",
            rejection_history.iter()
                .enumerate()
                .map(|(i, r)| format!("  第{}次拒绝: {}", i + 1, r))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let mut new_input = ReasoningInput::new(&original.goal);

        // Carry over original observations + add feedback
        new_input.observations = original.observations.clone();
        new_input.observations.push(feedback_observation);

        // Carry over constraints with emphasis
        new_input.constraints = original.constraints.clone();
        new_input.constraints.push("所有动作必须满足电压约束(0.95-1.05 pu)、热稳定约束(<100%加载)、N-1安全约束".to_string());

        // Carry over other fields
        new_input.memory_entries = original.memory_entries.clone();
        new_input.available_tools = original.available_tools.clone();
        new_input.power_observation = original.power_observation.clone();

        new_input
    }

    /// Format a rejection from ProjectionResult and ActionVerdict for feedback
    pub fn format_rejection(
        action: &StructuredAction,
        projection: &ProjectionResult,
        verdict: &ActionVerdict,
    ) -> String {
        let mut parts = Vec::new();

        parts.push(format!("建议的动作: {:?}", action));

        match projection {
            ProjectionResult::Projected { modifications, .. } => {
                let mods: Vec<String> = modifications.iter()
                    .map(|m| format!("  - {} 从 {:.2} 调整为 {:.2}（原因: {}）",
                        m.parameter, m.original_value, m.projected_value, m.reason))
                    .collect();
                parts.push(format!("投影结果: 动作需要修改\n{}", mods.join("\n")));
            }
            ProjectionResult::Infeasible { violated_constraints, suggested_alternatives, .. } => {
                parts.push(format!("不可行原因:\n{}", violated_constraints.iter()
                    .map(|v| format!("  - {}", v))
                    .collect::<Vec<_>>()
                    .join("\n")));
                if !suggested_alternatives.is_empty() {
                    parts.push(format!("建议替代方案: {:?}", suggested_alternatives));
                }
            }
            ProjectionResult::Feasible(_) => {
                // Projection was fine but verdict rejected
            }
        }

        match verdict {
            ActionVerdict::Rejected(reason) => {
                parts.push(format!("验证拒绝: {}", reason));
            }
            ActionVerdict::PendingApproval { approver_level, reason } => {
                parts.push(format!("需要 {:?} 审批: {}", approver_level, reason));
            }
            _ => {}
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::Result;

    /// Mock reasoning engine for testing
    struct MockEngine {
        response: String,
        include_actions: bool,
        call_count: std::sync::Mutex<usize>,
    }

    impl MockEngine {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                include_actions: true,
                call_count: std::sync::Mutex::new(0),
            }
        }

        fn no_actions(response: &str) -> Self {
            Self {
                response: response.to_string(),
                include_actions: false,
                call_count: std::sync::Mutex::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ReasoningEngine for MockEngine {
        fn name(&self) -> &str {
            "mock"
        }

        async fn reason(&self, _input: ReasoningInput) -> Result<ReasoningOutput> {
            let mut count = self.call_count.lock().unwrap();
            *count += 1;

            let mut output = ReasoningOutput::new(&self.response, 0.8)
                .with_step("Mock reasoning step");
            if self.include_actions {
                output = output.with_action("adjust generator 1 to 100MW");
            }
            Ok(output)
        }
    }

    #[tokio::test]
    async fn test_feedback_loop_basic() {
        let engine = MockEngine::new("Adjusted reasoning");
        let feedback = FeedbackLoop::with_default_iterations(Box::new(engine));

        let input = ReasoningInput::new("Handle voltage violation")
            .with_observation("Bus 3 voltage low: 0.88 pu");

        let result = feedback.reason_with_feedback(&input, "Action would worsen voltage").await.unwrap();
        assert!(result.retries <= 2);
    }

    #[tokio::test]
    async fn test_feedback_loop_max_iterations() {
        let engine = MockEngine::no_actions("No valid actions");
        let feedback = FeedbackLoop::new(Box::new(engine), 1);

        let input = ReasoningInput::new("Test");
        let result = feedback.reason_with_feedback(&input, "Rejected").await.unwrap();
        assert_eq!(result.retries, 1);
    }

    #[test]
    fn test_format_rejection_infeasible() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 };
        let projection = ProjectionResult::Infeasible {
            original: StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 },
            violated_constraints: vec!["Voltage violation: Bus 3 voltage 0.88 pu < 0.95 pu".to_string()],
            suggested_alternatives: vec![StructuredAction::ExecuteDevice {
                device_id: 1,
                operation: "adjust_reactive".to_string(),
                value: 10.0,
            }],
        };
        let verdict = ActionVerdict::Rejected("Voltage constraint violation".to_string());

        let formatted = FeedbackLoop::format_rejection(&action, &projection, &verdict);
        assert!(formatted.contains("300"));
        assert!(formatted.contains("0.88"));
        assert!(formatted.contains("adjust_reactive"));
    }

    #[test]
    fn test_format_rejection_projected() {
        let action = StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 };
        let projection = ProjectionResult::Projected {
            original: StructuredAction::StartGenerator { gen_id: 1, target_mw: 300.0 },
            projected: StructuredAction::StartGenerator { gen_id: 1, target_mw: 200.0 },
            modifications: vec![eneros_constraint::projector::ActionModification {
                parameter: "target_mw".to_string(),
                original_value: 300.0,
                projected_value: 200.0,
                reason: "Generator rated capacity 200MW".to_string(),
            }],
        };
        let verdict = ActionVerdict::Approved;

        let formatted = FeedbackLoop::format_rejection(&action, &projection, &verdict);
        assert!(formatted.contains("300"));
        assert!(formatted.contains("200"));
    }

    #[test]
    fn test_build_feedback_input() {
        let engine = MockEngine::new("test");
        let feedback = FeedbackLoop::new(Box::new(engine), 2);

        let input = ReasoningInput::new("Handle voltage")
            .with_observation("Bus 3 voltage low")
            .with_constraint("V > 0.95 pu");

        let feedback_input = feedback.build_feedback_input(&input, &["Voltage violation".to_string()]);

        assert!(feedback_input.observations.iter().any(|o| o.contains("约束反馈")));
        assert!(feedback_input.constraints.iter().any(|c| c.contains("0.95-1.05")));
    }
}
