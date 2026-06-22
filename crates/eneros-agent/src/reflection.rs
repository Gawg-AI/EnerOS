//! Reflection and learning closed loop — post-execution evaluation and lesson extraction.
//!
//! This module implements the F6 fix: agents can now reflect on their
//! execution outcomes, extract lessons learned, and store them in memory
//! for future decision improvement.
//!
//! ## Architecture
//!
//! ```text
//! Plan + ExecutionResult + ExpectedOutcome
//!                ↓
//!      ReflectionEngine::reflect()
//!                ↓
//!      ReflectionResult { success, failure_reasons, lessons_learned }
//!                ↓
//!      Lessons stored in AgentMemory (Procedural memory type)
//!                ↓
//!      Next time similar scenario → recall lessons → improve decision
//! ```
//!
//! ## Lesson Structure
//!
//! A `Lesson` captures:
//! - The scenario pattern (what situation triggered the plan)
//! - The failure reason (what went wrong, if anything)
//! - The improvement suggestion (what to do differently next time)
//! - The importance weight (higher = more likely to be recalled)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use eneros_core::Result;
use eneros_memory::{AgentMemory, MemoryEntry, MemoryType, RecallQuery};

use crate::planning::{Plan, PlanExecutionResult};

/// A lesson learned from reflecting on an execution outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Unique lesson ID
    pub id: String,
    /// The scenario pattern (e.g., "voltage_violation at bus 3")
    pub scenario: String,
    /// What went wrong (empty if success)
    pub failure_reason: String,
    /// What to do differently next time
    pub improvement: String,
    /// Importance weight (0.0~1.0)
    pub importance: f64,
    /// Tags for retrieval
    pub tags: Vec<String>,
    /// When the lesson was learned
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Lesson {
    /// Create a new lesson
    pub fn new(scenario: &str, failure_reason: &str, improvement: &str, importance: f64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            scenario: scenario.to_string(),
            failure_reason: failure_reason.to_string(),
            improvement: improvement.to_string(),
            importance: importance.clamp(0.0, 1.0),
            tags: Vec::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    /// Add tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Convert to a MemoryEntry for storage in AgentMemory
    pub fn to_memory_entry(&self) -> MemoryEntry {
        let content = format!(
            "Scenario: {}\nFailure: {}\nImprovement: {}",
            self.scenario, self.failure_reason, self.improvement
        );
        MemoryEntry::new(MemoryType::Procedural, content, self.importance)
            .with_tags(self.tags.clone())
    }

    /// Parse from a MemoryEntry (best-effort, for recall)
    pub fn from_memory_entry(entry: &MemoryEntry) -> Option<Self> {
        let content = &entry.content;
        let scenario = extract_field(content, "Scenario:")?;
        let failure_reason = extract_field(content, "Failure:").unwrap_or_default();
        let improvement = extract_field(content, "Improvement:")?;
        Some(Self {
            id: entry.id.clone(),
            scenario,
            failure_reason,
            improvement,
            importance: entry.importance,
            tags: entry.tags.clone(),
            timestamp: entry.timestamp,
        })
    }
}

/// Extract a field value from "Field: value" format in a multi-line string
fn extract_field(content: &str, field_name: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(field_name) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Result of reflecting on an execution outcome
#[derive(Debug, Clone)]
pub struct ReflectionResult {
    /// Whether the execution was successful
    pub success: bool,
    /// Reasons for failure (empty if success)
    pub failure_reasons: Vec<String>,
    /// Lessons learned from this execution
    pub lessons_learned: Vec<Lesson>,
    /// Summary of the reflection
    pub summary: String,
}

/// Controls when and how learning happens.
#[derive(Debug, Clone)]
pub struct LearningPolicy {
    /// Learn from every N-th execution (1 = every execution)
    pub learn_every_n: usize,
    /// Minimum importance for a lesson to be stored
    pub min_importance: f64,
    /// Maximum number of lessons to keep per agent
    pub max_lessons_per_agent: usize,
    /// Execution counter (internal)
    execution_count: usize,
}

impl Default for LearningPolicy {
    fn default() -> Self {
        Self {
            learn_every_n: 1,
            min_importance: 0.3,
            max_lessons_per_agent: 1000,
            execution_count: 0,
        }
    }
}

impl LearningPolicy {
    /// Create a new learning policy
    pub fn new(learn_every_n: usize, min_importance: f64) -> Self {
        Self {
            learn_every_n,
            min_importance,
            max_lessons_per_agent: 1000,
            execution_count: 0,
        }
    }

    /// Check if learning should happen for this execution
    pub fn should_learn(&mut self) -> bool {
        self.execution_count += 1;
        if self.learn_every_n == 0 {
            return false;
        }
        self.execution_count.is_multiple_of(self.learn_every_n)
    }
}

/// Reflection engine — evaluates execution outcomes and extracts lessons.
pub struct ReflectionEngine {
    /// Learning policy
    policy: LearningPolicy,
}

impl ReflectionEngine {
    /// Create a new reflection engine with default learning policy
    pub fn new() -> Self {
        Self {
            policy: LearningPolicy::default(),
        }
    }

    /// Create a new reflection engine with a custom learning policy
    pub fn with_policy(policy: LearningPolicy) -> Self {
        Self { policy }
    }

    /// Reflect on a plan execution result.
    ///
    /// This is the core method: it compares the expected outcomes with the
    /// actual results, identifies failures, and extracts lessons.
    pub async fn reflect(
        &mut self,
        plan: &Plan,
        execution_result: &PlanExecutionResult,
    ) -> Result<ReflectionResult> {
        let success = execution_result.success;
        let mut failure_reasons = Vec::new();
        let mut lessons_learned = Vec::new();

        if !success {
            // Extract failure reasons from step results
            for step_result in &execution_result.step_results {
                if !step_result.success {
                    if let Some(ref error) = step_result.error {
                        failure_reasons.push(format!(
                            "Step '{}' failed: {}",
                            step_result.step_id, error
                        ));
                    }
                }
            }

            // Check if we should learn from this execution
            if self.policy.should_learn() {
                // Find the failed step's expected outcome for context
                for step in &plan.steps {
                    if let Some(sr) = execution_result
                        .step_results
                        .iter()
                        .find(|r| r.step_id == step.step_id && !r.success)
                    {
                        let scenario = format!(
                            "Goal: {}, Step: {} ({})",
                            plan.goal.goal_type, step.step_id, step.description
                        );
                        let failure_reason = sr.error.clone().unwrap_or_default();
                        let improvement = generate_improvement_suggestion(
                            &plan.goal.goal_type,
                            &step.step_id,
                            &failure_reason,
                        );
                        let importance = calculate_importance(&plan.goal, &failure_reason);

                        if importance >= self.policy.min_importance {
                            let lesson = Lesson::new(
                                &scenario,
                                &failure_reason,
                                &improvement,
                                importance,
                            )
                            .with_tags(vec![
                                plan.goal.goal_type.clone(),
                                step.step_id.clone(),
                            ]);
                            lessons_learned.push(lesson);
                        }
                    }
                }
            }
        } else {
            // Successful execution — optionally learn from success patterns
            if self.policy.should_learn() {
                // Only learn from high-priority successes
                if plan.goal.priority >= 1 {
                    let scenario = format!("Goal: {} (successful)", plan.goal.goal_type);
                    let improvement = format!(
                        "Plan with steps {:?} succeeded for goal type '{}'",
                        plan.steps.iter().map(|s| s.step_id.as_str()).collect::<Vec<_>>(),
                        plan.goal.goal_type
                    );
                    let lesson = Lesson::new(
                        &scenario,
                        "",
                        &improvement,
                        0.5, // moderate importance for successes
                    )
                    .with_tags(vec![plan.goal.goal_type.clone(), "success".to_string()]);
                    lessons_learned.push(lesson);
                }
            }
        }

        let summary = if success {
            format!(
                "Plan '{}' completed successfully with {} steps",
                plan.id,
                execution_result.step_results.len()
            )
        } else {
            format!(
                "Plan '{}' failed with {} failure(s): {}",
                plan.id,
                failure_reasons.len(),
                failure_reasons.join("; ")
            )
        };

        Ok(ReflectionResult {
            success,
            failure_reasons,
            lessons_learned,
            summary,
        })
    }

    /// Store lessons in agent memory for future recall.
    pub async fn store_lessons(
    &self,
    memory: &Arc<dyn AgentMemory>,
    agent_id: &str,
    lessons: &[Lesson],
    ) -> Result<()> {
        for lesson in lessons {
            let entry = lesson.to_memory_entry();
            memory.store(agent_id, entry).await?;
        }
        Ok(())
    }

    /// Recall relevant lessons from agent memory for a given scenario.
    pub async fn recall_lessons(
    &self,
    memory: &Arc<dyn AgentMemory>,
    agent_id: &str,
    scenario_keyword: &str,
    ) -> Result<Vec<Lesson>> {
        let query = RecallQuery::new()
            .with_type(MemoryType::Procedural)
            .with_keyword(scenario_keyword)
            .with_limit(10);

        let entries = memory.recall(agent_id, &query).await?;
        let lessons: Vec<Lesson> = entries
            .iter()
            .filter_map(Lesson::from_memory_entry)
            .collect();
        Ok(lessons)
    }
}

impl Default for ReflectionEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate an improvement suggestion based on the goal type and failure.
fn generate_improvement_suggestion(
    goal_type: &str,
    step_id: &str,
    failure_reason: &str,
) -> String {
    match (goal_type, step_id) {
        ("voltage_violation", "adjust_reactive") => {
            "Consider adjusting multiple reactive power sources simultaneously, or check if the generator has sufficient reactive capacity".to_string()
        }
        ("voltage_violation", "verify_voltage") => {
            "Wait longer for voltage to stabilize after adjustment, or check if SCADA data is fresh".to_string()
        }
        ("overload", "reroute_power") => {
            "Check if alternative paths have sufficient capacity before rerouting".to_string()
        }
        ("frequency_deviation", "adjust_setpoints") => {
            "Coordinate setpoint adjustments across multiple generators to avoid oscillation".to_string()
        }
        ("restore_supply", "close_switches") => {
            "Verify switch status before closing, and check for any remaining fault indicators".to_string()
        }
        _ => {
            format!("Review step '{}' failure: {}. Consider adding pre-validation or alternative actions.", step_id, failure_reason)
        }
    }
}

/// Calculate the importance of a lesson based on goal priority and failure severity.
fn calculate_importance(goal: &crate::planning::Goal, failure_reason: &str) -> f64 {
    let mut importance = 0.5; // base importance

    // Higher priority goals → more important lessons
    importance += goal.priority as f64 * 0.15;

    // Constraint rejections are more important than simple errors
    if failure_reason.contains("ConstraintRejected") || failure_reason.contains("constraint") {
        importance += 0.2;
    }

    // Safety-related failures are most important
    if failure_reason.contains("safety") || failure_reason.contains("emergency") {
        importance += 0.3;
    }

    importance.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentAction;
    use crate::planning::{Goal, Plan, PlanStep};

    fn make_test_plan() -> Plan {
        let mut plan = Plan::new(Goal::new("voltage_violation", "Bus 3 voltage low").with_priority(1));
        plan.add_step(PlanStep::new("check_voltage", "Check voltage", AgentAction::NoOp));
        plan.add_step(
            PlanStep::new("adjust_reactive", "Adjust reactive power", AgentAction::NoOp)
                .depends_on_step("check_voltage"),
        );
        plan.add_step(
            PlanStep::new("verify_voltage", "Verify voltage", AgentAction::NoOp)
                .depends_on_step("adjust_reactive"),
        );
        plan
    }

    #[tokio::test]
    async fn test_reflect_on_success() {
        let mut engine = ReflectionEngine::new();
        let plan = make_test_plan();
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: true,
            step_results: vec![
                crate::planning::StepResult {
                    step_id: "check_voltage".to_string(),
                    success: true,
                    error: None,
                },
                crate::planning::StepResult {
                    step_id: "adjust_reactive".to_string(),
                    success: true,
                    error: None,
                },
                crate::planning::StepResult {
                    step_id: "verify_voltage".to_string(),
                    success: true,
                    error: None,
                },
            ],
            error: None,
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(result.success);
        assert!(result.failure_reasons.is_empty());
        // High-priority success should generate a lesson
        assert_eq!(result.lessons_learned.len(), 1);
    }

    #[tokio::test]
    async fn test_reflect_on_failure() {
        let mut engine = ReflectionEngine::new();
        let plan = make_test_plan();
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: false,
            step_results: vec![
                crate::planning::StepResult {
                    step_id: "check_voltage".to_string(),
                    success: true,
                    error: None,
                },
                crate::planning::StepResult {
                    step_id: "adjust_reactive".to_string(),
                    success: false,
                    error: Some("ConstraintRejected: reactive power limit reached".to_string()),
                },
                crate::planning::StepResult {
                    step_id: "verify_voltage".to_string(),
                    success: false,
                    error: Some("dependencies not satisfied".to_string()),
                },
            ],
            error: Some("Step 'adjust_reactive' action rejected".to_string()),
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.failure_reasons.len(), 2);
        assert!(!result.lessons_learned.is_empty());

        // Check lesson content
        let lesson = &result.lessons_learned[0];
        assert!(lesson.scenario.contains("voltage_violation"));
        assert!(lesson.improvement.contains("reactive"));
        assert!(lesson.importance > 0.5); // constraint rejection boosts importance
    }

    #[tokio::test]
    async fn test_store_and_recall_lessons() {
        let engine = ReflectionEngine::new();
        let memory: Arc<dyn AgentMemory> = Arc::new(eneros_memory::InMemoryMemory::default());

        let lessons = vec![
            Lesson::new(
                "voltage_violation at bus 3",
                "reactive power limit reached",
                "adjust multiple sources",
                0.8,
            )
            .with_tags(vec!["voltage_violation".to_string()]),
            Lesson::new(
                "overload on branch 5",
                "alternative path also overloaded",
                "check capacity before rerouting",
                0.7,
            )
            .with_tags(vec!["overload".to_string()]),
        ];

        engine.store_lessons(&memory, "agent-1", &lessons).await.unwrap();

        let recalled = engine.recall_lessons(&memory, "agent-1", "voltage").await.unwrap();
        assert_eq!(recalled.len(), 1);
        assert!(recalled[0].scenario.contains("voltage_violation"));
    }

    #[tokio::test]
    async fn test_learning_policy_skip() {
        let policy = LearningPolicy::new(3, 0.5); // learn every 3rd execution
        let mut engine = ReflectionEngine::with_policy(policy);

        let plan = make_test_plan();
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: false,
            step_results: vec![crate::planning::StepResult {
                step_id: "check_voltage".to_string(),
                success: false,
                error: Some("error".to_string()),
            }],
            error: Some("failed".to_string()),
        };

        // First execution: should not learn (counter=1, not divisible by 3)
        let r1 = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(r1.lessons_learned.is_empty());

        // Second execution: should not learn (counter=2)
        let r2 = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(r2.lessons_learned.is_empty());

        // Third execution: should learn (counter=3, divisible by 3)
        let r3 = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(!r3.lessons_learned.is_empty());
    }

    #[test]
    fn test_lesson_to_memory_entry_roundtrip() {
        let lesson = Lesson::new(
            "test scenario",
            "test failure",
            "test improvement",
            0.75,
        )
        .with_tags(vec!["tag1".to_string()]);

        let entry = lesson.to_memory_entry();
        assert_eq!(entry.memory_type, MemoryType::Procedural);
        assert!((entry.importance - 0.75).abs() < 1e-6);

        let parsed = Lesson::from_memory_entry(&entry).unwrap();
        assert_eq!(parsed.scenario, "test scenario");
        assert_eq!(parsed.failure_reason, "test failure");
        assert_eq!(parsed.improvement, "test improvement");
    }

    #[test]
    fn test_generate_improvement_suggestion() {
        let suggestion = generate_improvement_suggestion(
            "voltage_violation",
            "adjust_reactive",
            "limit reached",
        );
        assert!(suggestion.contains("reactive"));

        let suggestion = generate_improvement_suggestion(
            "restore_supply",
            "close_switches",
            "switch stuck",
        );
        assert!(suggestion.contains("switch"));
    }

    #[test]
    fn test_calculate_importance() {
        let goal = Goal::new("voltage_violation", "test").with_priority(2);
        let importance = calculate_importance(&goal, "ConstraintRejected: limit reached");
        assert!(importance > 0.8); // priority 2 + constraint rejection
        assert!(importance <= 1.0);
    }

    // ========================================================================
    // T030-07: 覆盖率补充测试
    // ========================================================================

    #[test]
    fn test_lesson_new_clamps_importance() {
        // importance 应被 clamp 到 [0.0, 1.0]
        let lesson_high = Lesson::new("s", "f", "i", 1.5);
        assert!((lesson_high.importance - 1.0).abs() < 1e-6);

        let lesson_low = Lesson::new("s", "f", "i", -0.5);
        assert!((lesson_low.importance - 0.0).abs() < 1e-6);

        let lesson_normal = Lesson::new("s", "f", "i", 0.7);
        assert!((lesson_normal.importance - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_lesson_with_tags() {
        let lesson = Lesson::new("s", "f", "i", 0.5)
            .with_tags(vec!["tag1".to_string(), "tag2".to_string()]);
        assert_eq!(lesson.tags.len(), 2);
        assert!(lesson.tags.contains(&"tag1".to_string()));
        assert!(lesson.tags.contains(&"tag2".to_string()));
    }

    #[test]
    fn test_lesson_to_memory_entry_has_correct_type() {
        let lesson = Lesson::new("scenario", "failure", "improvement", 0.6);
        let entry = lesson.to_memory_entry();
        assert_eq!(entry.memory_type, MemoryType::Procedural);
        assert!((entry.importance - 0.6).abs() < 1e-6);
        // 内容应包含三个字段
        assert!(entry.content.contains("scenario"));
        assert!(entry.content.contains("failure"));
        assert!(entry.content.contains("improvement"));
    }

    #[test]
    fn test_lesson_from_memory_entry_missing_fields_returns_none() {
        // 缺少 Scenario 或 Improvement 字段应返回 None
        let entry = MemoryEntry::new(MemoryType::Procedural, "no fields here".to_string(), 0.5);
        assert!(Lesson::from_memory_entry(&entry).is_none());
    }

    #[test]
    fn test_lesson_from_memory_entry_partial_fields() {
        // 只有 Scenario 和 Improvement（无 Failure）应能解析，failure_reason 为空
        let content = "Scenario: test scenario\nImprovement: do better".to_string();
        let entry = MemoryEntry::new(MemoryType::Procedural, content, 0.5);
        let lesson = Lesson::from_memory_entry(&entry).unwrap();
        assert_eq!(lesson.scenario, "test scenario");
        assert!(lesson.failure_reason.is_empty());
        assert_eq!(lesson.improvement, "do better");
    }

    #[test]
    fn test_learning_policy_default() {
        let policy = LearningPolicy::default();
        assert_eq!(policy.learn_every_n, 1);
        assert_eq!(policy.min_importance, 0.3);
        assert_eq!(policy.max_lessons_per_agent, 1000);
    }

    #[test]
    fn test_learning_policy_should_learn_every_n() {
        let mut policy = LearningPolicy::new(2, 0.5);
        // 第 1 次：不学习（1 不是 2 的倍数）
        assert!(!policy.should_learn());
        // 第 2 次：学习
        assert!(policy.should_learn());
        // 第 3 次：不学习
        assert!(!policy.should_learn());
        // 第 4 次：学习
        assert!(policy.should_learn());
    }

    #[test]
    fn test_learning_policy_zero_n_never_learns() {
        let mut policy = LearningPolicy::new(0, 0.5);
        assert!(!policy.should_learn());
        assert!(!policy.should_learn());
        assert!(!policy.should_learn());
    }

    #[test]
    fn test_learning_policy_one_n_always_learns() {
        let mut policy = LearningPolicy::new(1, 0.5);
        assert!(policy.should_learn());
        assert!(policy.should_learn());
        assert!(policy.should_learn());
    }

    #[test]
    fn test_reflection_engine_new_has_default_policy() {
        let engine = ReflectionEngine::new();
        // 默认 policy: learn_every_n=1, min_importance=0.3
        // 验证默认 policy 的 should_learn() 行为（每次都学习）
        // 需要可变引用来调用 should_learn()
        let mut engine = engine;
        assert!(engine.policy.should_learn()); // learn_every_n=1，每次都应学习
    }

    #[test]
    fn test_reflection_engine_with_policy() {
        let policy = LearningPolicy::new(5, 0.8);
        // 验证自定义 policy 被正确设置
        assert_eq!(policy.learn_every_n, 5);
        assert_eq!(policy.min_importance, 0.8);
        let engine = ReflectionEngine::with_policy(policy);
        // 验证 engine 可正常构造
        let _ = engine;
    }

    #[tokio::test]
    async fn test_reflect_on_success_low_priority_no_lesson() {
        // 低优先级（priority=0）成功执行不应生成 lesson
        let mut engine = ReflectionEngine::new();
        let plan = Plan::new(Goal::new("test", "low priority success"));
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: true,
            step_results: vec![crate::planning::StepResult {
                step_id: "s1".to_string(),
                success: true,
                error: None,
            }],
            error: None,
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(result.success);
        // priority=0 < 1，不应生成 lesson
        assert!(result.lessons_learned.is_empty());
    }

    #[tokio::test]
    async fn test_reflect_summary_on_success() {
        let mut engine = ReflectionEngine::new();
        let plan = make_test_plan();
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: true,
            step_results: vec![crate::planning::StepResult {
                step_id: "check_voltage".to_string(),
                success: true,
                error: None,
            }],
            error: None,
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(result.summary.contains("successfully"));
        assert!(result.summary.contains(&plan.id));
    }

    #[tokio::test]
    async fn test_reflect_summary_on_failure() {
        let mut engine = ReflectionEngine::new();
        let plan = make_test_plan();
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: false,
            step_results: vec![crate::planning::StepResult {
                step_id: "check_voltage".to_string(),
                success: false,
                error: Some("connection lost".to_string()),
            }],
            error: Some("failed".to_string()),
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        assert!(result.summary.contains("failed"));
        assert!(result.summary.contains(&plan.id));
    }

    #[tokio::test]
    async fn test_reflect_importance_below_min_not_stored() {
        // 重要性低于 min_importance 的 lesson 不应生成
        let policy = LearningPolicy::new(1, 0.99); // 极高的 min_importance
        let mut engine = ReflectionEngine::with_policy(policy);

        let plan = make_test_plan(); // priority=1
        let exec_result = PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: false,
            step_results: vec![crate::planning::StepResult {
                step_id: "check_voltage".to_string(),
                success: false,
                error: Some("simple error".to_string()),
            }],
            error: Some("failed".to_string()),
        };

        let result = engine.reflect(&plan, &exec_result).await.unwrap();
        // importance = 0.5 + 1*0.15 = 0.65 < 0.99，不应生成 lesson
        assert!(result.lessons_learned.is_empty());
    }

    #[test]
    fn test_calculate_importance_base_value() {
        // 基础重要性 = 0.5 + priority * 0.15
        let goal = Goal::new("test", "d").with_priority(0);
        let importance = calculate_importance(&goal, "plain error");
        assert!((importance - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_calculate_importance_safety_related() {
        // safety/emergency 关键词应增加重要性
        let goal = Goal::new("test", "d").with_priority(0);
        let importance = calculate_importance(&goal, "safety violation detected");
        // 0.5 + 0.3 (safety) = 0.8
        assert!(importance >= 0.8);
    }

    #[test]
    fn test_calculate_importance_capped_at_1() {
        // 重要性不应超过 1.0
        let goal = Goal::new("test", "d").with_priority(2);
        let importance = calculate_importance(
            &goal,
            "ConstraintRejected: safety emergency detected",
        );
        assert!(importance <= 1.0);
    }

    #[test]
    fn test_generate_improvement_suggestion_default_case() {
        // 未知 goal_type/step_id 组合应返回默认建议
        let suggestion = generate_improvement_suggestion(
            "unknown_goal",
            "unknown_step",
            "some failure",
        );
        assert!(suggestion.contains("unknown_step"));
        assert!(suggestion.contains("some failure"));
    }

    #[test]
    fn test_generate_improvement_suggestion_all_known_cases() {
        // 验证所有已知 case 都有定制建议
        let cases = vec![
            ("voltage_violation", "adjust_reactive"),
            ("voltage_violation", "verify_voltage"),
            ("overload", "reroute_power"),
            ("frequency_deviation", "adjust_setpoints"),
            ("restore_supply", "close_switches"),
        ];
        for (goal, step) in cases {
            let suggestion = generate_improvement_suggestion(goal, step, "failure");
            assert!(!suggestion.contains("Review step"), "case ({}, {}) should have custom suggestion", goal, step);
        }
    }

    #[tokio::test]
    async fn test_store_lessons_empty_list() {
        // 空 lesson 列表应正常返回
        let engine = ReflectionEngine::new();
        let memory: Arc<dyn AgentMemory> = Arc::new(eneros_memory::InMemoryMemory::default());
        let result = engine.store_lessons(&memory, "agent-1", &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_recall_lessons_empty_memory() {
        // 空 memory 应返回空列表
        let engine = ReflectionEngine::new();
        let memory: Arc<dyn AgentMemory> = Arc::new(eneros_memory::InMemoryMemory::default());
        let result = engine.recall_lessons(&memory, "agent-1", "anything").await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_field_present() {
        let content = "Line1\nScenario: my scenario\nImprovement: do better";
        let field = extract_field(content, "Scenario:");
        assert_eq!(field, Some("my scenario".to_string()));
    }

    #[test]
    fn test_extract_field_absent() {
        let content = "Line1\nNo matching field here";
        let field = extract_field(content, "Scenario:");
        assert_eq!(field, None);
    }

    #[test]
    fn test_extract_field_empty_value() {
        let content = "Scenario: \nImprovement: something";
        let field = extract_field(content, "Scenario:");
        assert_eq!(field, Some("".to_string()));
    }
}
