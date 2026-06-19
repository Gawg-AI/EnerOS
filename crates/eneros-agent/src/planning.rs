//! Behavior planning engine — multi-step goal-directed action planning.
//!
//! This module implements the F5 fix: agents can now generate and execute
//! multi-step plans to achieve goals, rather than only reacting to single
//! events with single actions.
//!
//! ## Architecture
//!
//! ```text
//! Goal → Planner::plan() → Plan (DAG of steps)
//!                           ↓
//!                    PlanExecutor::execute()
//!                           ↓
//!                    Step 1 → Step 2 → Step 3 (topological order)
//!                           ↓
//!                    Each step → AgentAction → Dispatcher
//! ```
//!
//! ## Plan Structure
//!
//! A `Plan` is a directed acyclic graph (DAG) of `PlanStep`s. Each step has:
//! - A unique ID
//! - An action to execute
//! - Dependencies (step IDs that must complete before this step)
//! - Preconditions (must be true before execution)
//! - Expected outcome (for post-execution validation)
//!
//! ## Rule-Based Planning
//!
//! The `RuleBasedPlanner` uses pattern matching on goal types to generate
//! plans from templates. For example:
//!
//! | Goal | Plan Steps |
//! |------|------------|
//! | "voltage_violation" | check_adjacent_buses → adjust_reactive_power → verify_voltage |
//! | "overload" | check_alternative_paths → reroute_power → verify_loading |
//! | "frequency_deviation" | check_generation_balance → adjust_setpoints → verify_frequency |
//! | "restore_supply" | isolate_fault_section → find_restoration_path → close_switches → verify_restoration |

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use eneros_core::Result;

use crate::agent::AgentAction;
use crate::dispatcher::ActionDispatcher;

/// A goal that an agent wants to achieve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    /// Goal type identifier (e.g., "voltage_violation", "restore_supply")
    pub goal_type: String,
    /// Human-readable description
    pub description: String,
    /// Priority (0=normal, 1=high, 2=emergency)
    pub priority: u8,
    /// Optional target parameters (e.g., {"bus_id": "3", "target_voltage": "1.0"})
    pub params: HashMap<String, String>,
}

impl Goal {
    /// Create a new goal
    pub fn new(goal_type: &str, description: &str) -> Self {
        Self {
            goal_type: goal_type.to_string(),
            description: description.to_string(),
            priority: 0,
            params: HashMap::new(),
        }
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Add a parameter
    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }
}

/// A single step in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Unique step ID within the plan
    pub step_id: String,
    /// Human-readable description
    pub description: String,
    /// The action to execute
    pub action: AgentAction,
    /// Step IDs that must complete before this step
    pub depends_on: Vec<String>,
    /// Preconditions (human-readable, for logging/validation)
    pub preconditions: Vec<String>,
    /// Expected outcome (for post-execution validation)
    pub expected_outcome: String,
}

impl PlanStep {
    /// Create a new plan step
    pub fn new(step_id: &str, description: impl Into<String>, action: AgentAction) -> Self {
        Self {
            step_id: step_id.to_string(),
            description: description.into(),
            action,
            depends_on: Vec::new(),
            preconditions: Vec::new(),
            expected_outcome: String::new(),
        }
    }

    /// Add a dependency
    pub fn depends_on_step(mut self, step_id: &str) -> Self {
        self.depends_on.push(step_id.to_string());
        self
    }

    /// Add a precondition
    pub fn with_precondition(mut self, condition: &str) -> Self {
        self.preconditions.push(condition.to_string());
        self
    }

    /// Set expected outcome
    pub fn with_expected_outcome(mut self, outcome: &str) -> Self {
        self.expected_outcome = outcome.to_string();
        self
    }
}

/// A plan is a DAG of steps to achieve a goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Plan ID
    pub id: String,
    /// The goal this plan achieves
    pub goal: Goal,
    /// Steps in the plan (order = topological order)
    pub steps: Vec<PlanStep>,
    /// Whether the plan is currently executing
    pub status: PlanStatus,
}

/// Plan execution status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStatus {
    /// Plan created but not started
    Pending,
    /// Plan is executing
    InProgress,
    /// Plan completed successfully
    Completed,
    /// Plan failed
    Failed(String),
    /// Plan was cancelled
    Cancelled,
}

impl Plan {
    /// Create a new plan
    pub fn new(goal: Goal) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            goal,
            steps: Vec::new(),
            status: PlanStatus::Pending,
        }
    }

    /// Add a step to the plan
    pub fn add_step(&mut self, step: PlanStep) {
        self.steps.push(step);
    }

    /// Get a step by ID
    pub fn get_step(&self, step_id: &str) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.step_id == step_id)
    }

    /// Validate that the plan is a proper DAG (no cycles, all dependencies exist)
    pub fn validate(&self) -> std::result::Result<(), String> {
        let step_ids: HashSet<&str> = self.steps.iter().map(|s| s.step_id.as_str()).collect();

        // Check all dependencies reference existing steps
        for step in &self.steps {
            for dep in &step.depends_on {
                if !step_ids.contains(dep.as_str()) {
                    return Err(format!(
                        "Step '{}' depends on non-existent step '{}'",
                        step.step_id, dep
                    ));
                }
            }
        }

        // Check for cycles using topological sort (Kahn's algorithm)
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for step in &self.steps {
            in_degree.entry(step.step_id.as_str()).or_insert(0);
            for _dep in &step.depends_on {
                *in_degree.entry(step.step_id.as_str()).or_insert(0) += 1;
            }
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&k, _)| k)
            .collect();

        let mut visited = 0;
        while let Some(current) = queue.pop() {
            visited += 1;
            // Find steps that depend on `current`
            for step in &self.steps {
                if step.depends_on.iter().any(|d| d == current) {
                    if let Some(deg) = in_degree.get_mut(step.step_id.as_str()) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(step.step_id.as_str());
                        }
                    }
                }
            }
        }

        if visited != self.steps.len() {
            return Err("Plan contains a cycle".to_string());
        }

        Ok(())
    }

    /// Get steps in topological order (dependencies first)
    pub fn topological_order(&self) -> Vec<&PlanStep> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for step in &self.steps {
            *in_degree.entry(step.step_id.as_str()).or_insert(0) += step.depends_on.len();
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&k, _)| k)
            .collect();

        let mut result = Vec::new();
        while let Some(current) = queue.pop() {
            if let Some(step) = self.get_step(current) {
                result.push(step);
            }
            for step in &self.steps {
                if step.depends_on.iter().any(|d| d == current) {
                    if let Some(deg) = in_degree.get_mut(step.step_id.as_str()) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(step.step_id.as_str());
                        }
                    }
                }
            }
        }

        result
    }
}

/// Planner trait — generates plans for goals.
#[async_trait::async_trait]
pub trait Planner: Send + Sync {
    /// Generate a plan for the given goal.
    async fn plan(&self, goal: &Goal) -> Result<Plan>;
}

/// Rule-based planner — generates plans from goal-type templates.
pub struct RuleBasedPlanner {
    /// Registered plan templates, keyed by goal type
    templates: HashMap<String, PlanTemplate>,
}

/// A plan template — a function that generates a plan for a goal.
type PlanTemplate = Box<dyn Fn(&Goal) -> Plan + Send + Sync>;

impl RuleBasedPlanner {
    /// Create a new rule-based planner with built-in templates
    pub fn new() -> Self {
        let mut planner = Self {
            templates: HashMap::new(),
        };
        planner.register_builtin_templates();
        planner
    }

    /// Register a plan template for a goal type
    pub fn register_template<F>(&mut self, goal_type: &str, template: F)
    where
        F: Fn(&Goal) -> Plan + Send + Sync + 'static,
    {
        self.templates
            .insert(goal_type.to_string(), Box::new(template));
    }

    /// Register built-in templates for common power system goals
    fn register_builtin_templates(&mut self) {
        // Voltage violation: check → adjust reactive power → verify
        self.register_template("voltage_violation", |goal| {
            let bus_id = goal
                .params
                .get("bus_id")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let mut plan = Plan::new(goal.clone());
            plan.add_step(
                PlanStep::new(
                    "check_voltage",
                    format!("Check voltage at bus {}", bus_id),
                    AgentAction::LogMessage(format!("Checking voltage at bus {}", bus_id)),
                )
                .with_precondition("SCADA data available")
                .with_expected_outcome("Voltage reading obtained"),
            );
            plan.add_step(
                PlanStep::new(
                    "adjust_reactive",
                    "Adjust reactive power output",
                    AgentAction::LogMessage("Adjusting reactive power".to_string()),
                )
                .depends_on_step("check_voltage")
                .with_precondition("Voltage below 0.95 pu or above 1.05 pu")
                .with_expected_outcome("Voltage within 0.95-1.05 pu"),
            );
            plan.add_step(
                PlanStep::new(
                    "verify_voltage",
                    "Verify voltage is within limits",
                    AgentAction::LogMessage("Verifying voltage".to_string()),
                )
                .depends_on_step("adjust_reactive")
                .with_expected_outcome("Voltage confirmed within limits"),
            );
            plan
        });

        // Overload: check → reroute → verify
        self.register_template("overload", |goal| {
            let branch_id = goal
                .params
                .get("branch_id")
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let mut plan = Plan::new(goal.clone());
            plan.add_step(PlanStep::new(
                "check_loading",
                format!("Check loading on branch {}", branch_id),
                AgentAction::LogMessage(format!("Checking branch {} loading", branch_id)),
            ));
            plan.add_step(
                PlanStep::new(
                    "reroute_power",
                    "Reroute power via alternative paths",
                    AgentAction::LogMessage("Rerouting power".to_string()),
                )
                .depends_on_step("check_loading")
                .with_expected_outcome("Branch loading below 100%"),
            );
            plan.add_step(
                PlanStep::new(
                    "verify_loading",
                    "Verify branch loading is within limits",
                    AgentAction::LogMessage("Verifying loading".to_string()),
                )
                .depends_on_step("reroute_power")
                .with_expected_outcome("Loading confirmed below limits"),
            );
            plan
        });

        // Frequency deviation: check balance → adjust setpoints → verify
        self.register_template("frequency_deviation", |goal| {
            let mut plan = Plan::new(goal.clone());
            plan.add_step(PlanStep::new(
                "check_balance",
                "Check generation-load balance",
                AgentAction::LogMessage("Checking gen-load balance".to_string()),
            ));
            plan.add_step(
                PlanStep::new(
                    "adjust_setpoints",
                    "Adjust generator setpoints",
                    AgentAction::LogMessage("Adjusting generator setpoints".to_string()),
                )
                .depends_on_step("check_balance")
                .with_expected_outcome("Frequency within 49.5-50.5 Hz"),
            );
            plan.add_step(
                PlanStep::new(
                    "verify_frequency",
                    "Verify frequency is within limits",
                    AgentAction::LogMessage("Verifying frequency".to_string()),
                )
                .depends_on_step("adjust_setpoints")
                .with_expected_outcome("Frequency confirmed within limits"),
            );
            plan
        });

        // Restore supply: isolate → find path → close switches → verify
        self.register_template("restore_supply", |goal| {
            let mut plan = Plan::new(goal.clone());
            plan.add_step(PlanStep::new(
                "isolate_fault",
                "Isolate fault section",
                AgentAction::LogMessage("Isolating fault section".to_string()),
            ));
            plan.add_step(
                PlanStep::new(
                    "find_restoration_path",
                    "Find restoration path",
                    AgentAction::LogMessage("Finding restoration path".to_string()),
                )
                .depends_on_step("isolate_fault")
                .with_expected_outcome("Restoration path identified"),
            );
            plan.add_step(
                PlanStep::new(
                    "close_switches",
                    "Close switches to restore supply",
                    AgentAction::LogMessage("Closing switches".to_string()),
                )
                .depends_on_step("find_restoration_path")
                .with_expected_outcome("Supply restored to affected area"),
            );
            plan.add_step(
                PlanStep::new(
                    "verify_restoration",
                    "Verify supply is restored",
                    AgentAction::LogMessage("Verifying restoration".to_string()),
                )
                .depends_on_step("close_switches")
                .with_expected_outcome("Restoration confirmed"),
            );
            plan
        });
    }
}

impl Default for RuleBasedPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Planner for RuleBasedPlanner {
    async fn plan(&self, goal: &Goal) -> Result<Plan> {
        if let Some(template) = self.templates.get(&goal.goal_type) {
            let plan = template(goal);
            plan.validate().map_err(|e| {
                eneros_core::EnerOSError::Internal(format!("plan validation failed: {}", e))
            })?;
            Ok(plan)
        } else {
            // Default: single-step plan that logs the goal
            let mut plan = Plan::new(goal.clone());
            plan.add_step(PlanStep::new(
                "default_action",
                format!("Handle goal: {}", goal.description),
                AgentAction::LogMessage(format!("No template for goal type '{}', logging", goal.goal_type)),
            ));
            Ok(plan)
        }
    }
}

/// Result of executing a plan step
#[derive(Debug, Clone)]
pub struct StepResult {
    /// Step ID
    pub step_id: String,
    /// Whether the step succeeded
    pub success: bool,
    /// Optional error message
    pub error: Option<String>,
}

/// Result of executing a plan
#[derive(Debug, Clone)]
pub struct PlanExecutionResult {
    /// Plan ID
    pub plan_id: String,
    /// Whether the plan completed successfully
    pub success: bool,
    /// Results for each step
    pub step_results: Vec<StepResult>,
    /// Error message if the plan failed
    pub error: Option<String>,
}

/// Executes plans step by step in topological order.
pub struct PlanExecutor {
    dispatcher: Arc<ActionDispatcher>,
}

impl PlanExecutor {
    /// Create a new plan executor
    pub fn new(dispatcher: Arc<ActionDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Execute a plan step by step in topological order.
    ///
    /// If any step fails, the plan is aborted and remaining steps are skipped.
    pub async fn execute(&self, plan: &mut Plan) -> Result<PlanExecutionResult> {
        plan.status = PlanStatus::InProgress;

        let ordered_steps: Vec<PlanStep> = plan
            .topological_order()
            .into_iter()
            .cloned()
            .collect();

        let mut step_results = Vec::new();
        let mut completed_steps: HashSet<String> = HashSet::new();

        for step in ordered_steps {
            // Check dependencies are all completed
            let deps_ok: bool = step
                .depends_on
                .iter()
                .all(|dep| completed_steps.contains(dep));

            if !deps_ok {
                let err = format!(
                    "Step '{}' dependencies not satisfied: {:?}",
                    step.step_id, step.depends_on
                );
                step_results.push(StepResult {
                    step_id: step.step_id.clone(),
                    success: false,
                    error: Some(err.clone()),
                });
                plan.status = PlanStatus::Failed(err.clone());
                return Ok(PlanExecutionResult {
                    plan_id: plan.id.clone(),
                    success: false,
                    step_results,
                    error: Some(err),
                });
            }

            // Execute the step's action
            let dispatch_result = self.dispatcher.dispatch(step.action.clone()).await;

            match dispatch_result {
                Ok(result) => {
                    let success = !matches!(
                        result,
                        crate::dispatcher::DispatchResult::CommandRejected(_)
                            | crate::dispatcher::DispatchResult::ConstraintRejected(_)
                    );
                    step_results.push(StepResult {
                        step_id: step.step_id.clone(),
                        success,
                        error: if success {
                            None
                        } else {
                            Some(format!("Dispatch result: {:?}", result))
                        },
                    });
                    if success {
                        completed_steps.insert(step.step_id.clone());
                    } else {
                        let err = format!("Step '{}' action rejected", step.step_id);
                        plan.status = PlanStatus::Failed(err.clone());
                        return Ok(PlanExecutionResult {
                            plan_id: plan.id.clone(),
                            success: false,
                            step_results,
                            error: Some(err),
                        });
                    }
                }
                Err(e) => {
                    let err = format!("Step '{}' failed: {}", step.step_id, e);
                    step_results.push(StepResult {
                        step_id: step.step_id.clone(),
                        success: false,
                        error: Some(err.clone()),
                    });
                    plan.status = PlanStatus::Failed(err.clone());
                    return Ok(PlanExecutionResult {
                        plan_id: plan.id.clone(),
                        success: false,
                        step_results,
                        error: Some(err),
                    });
                }
            }
        }

        plan.status = PlanStatus::Completed;
        Ok(PlanExecutionResult {
            plan_id: plan.id.clone(),
            success: true,
            step_results,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AgentContext;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_memory::InMemoryMemory;
    use eneros_network::PowerNetwork;
    use eneros_reasoning::RuleBasedEngine;
    use eneros_tool::ToolEngine;
    use parking_lot::RwLock;

    fn test_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    fn test_dispatcher(ctx: &AgentContext) -> ActionDispatcher {
        ActionDispatcher::new(
            std::sync::Arc::clone(&ctx.remote.event_bus),
            std::sync::Arc::clone(&ctx.remote.gateway_client),
        )
    }

    #[tokio::test]
    async fn test_rule_based_planner_voltage_violation() {
        let planner = RuleBasedPlanner::new();
        let goal = Goal::new("voltage_violation", "Bus 3 voltage low")
            .with_param("bus_id", "3");
        let plan = planner.plan(&goal).await.unwrap();

        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].step_id, "check_voltage");
        assert_eq!(plan.steps[1].step_id, "adjust_reactive");
        assert_eq!(plan.steps[2].step_id, "verify_voltage");
        assert!(plan.steps[1].depends_on.contains(&"check_voltage".to_string()));
    }

    #[tokio::test]
    async fn test_rule_based_planner_restore_supply() {
        let planner = RuleBasedPlanner::new();
        let goal = Goal::new("restore_supply", "Restore supply after fault");
        let plan = planner.plan(&goal).await.unwrap();

        assert_eq!(plan.steps.len(), 4);
        assert_eq!(plan.steps[0].step_id, "isolate_fault");
        assert_eq!(plan.steps[3].step_id, "verify_restoration");
    }

    #[tokio::test]
    async fn test_rule_based_planner_unknown_goal_type() {
        let planner = RuleBasedPlanner::new();
        let goal = Goal::new("unknown_type", "Some unknown goal");
        let plan = planner.plan(&goal).await.unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].step_id, "default_action");
    }

    #[tokio::test]
    async fn test_plan_validate_detects_cycle() {
        let mut plan = Plan::new(Goal::new("test", "test"));
        plan.add_step(
            PlanStep::new("a", "A", AgentAction::NoOp)
                .depends_on_step("b"),
        );
        plan.add_step(
            PlanStep::new("b", "B", AgentAction::NoOp)
                .depends_on_step("a"),
        );
        assert!(plan.validate().is_err());
    }

    #[tokio::test]
    async fn test_plan_validate_detects_missing_dependency() {
        let mut plan = Plan::new(Goal::new("test", "test"));
        plan.add_step(
            PlanStep::new("a", "A", AgentAction::NoOp)
                .depends_on_step("nonexistent"),
        );
        assert!(plan.validate().is_err());
    }

    #[tokio::test]
    async fn test_plan_topological_order() {
        let mut plan = Plan::new(Goal::new("test", "test"));
        plan.add_step(
            PlanStep::new("c", "C", AgentAction::NoOp)
                .depends_on_step("a")
                .depends_on_step("b"),
        );
        plan.add_step(PlanStep::new("a", "A", AgentAction::NoOp));
        plan.add_step(
            PlanStep::new("b", "B", AgentAction::NoOp)
                .depends_on_step("a"),
        );

        let order: Vec<&str> = plan.topological_order().iter().map(|s| s.step_id.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn test_plan_executor_success() {
        let ctx = test_context();
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let executor = PlanExecutor::new(dispatcher);

        let planner = RuleBasedPlanner::new();
        let goal = Goal::new("voltage_violation", "Bus 3 voltage low");
        let mut plan = planner.plan(&goal).await.unwrap();

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(result.success);
        assert_eq!(result.step_results.len(), 3);
        assert!(result.step_results.iter().all(|r| r.success));
        assert_eq!(plan.status, PlanStatus::Completed);
    }

    #[tokio::test]
    async fn test_plan_executor_empty_plan() {
        let ctx = test_context();
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let executor = PlanExecutor::new(dispatcher);

        let mut plan = Plan::new(Goal::new("test", "empty plan"));
        let result = executor.execute(&mut plan).await.unwrap();

        assert!(result.success);
        assert!(result.step_results.is_empty());
    }

    #[tokio::test]
    async fn test_plan_executor_dependency_chain() {
        let ctx = test_context();
        let dispatcher = Arc::new(test_dispatcher(&ctx));
        let executor = PlanExecutor::new(dispatcher);

        let mut plan = Plan::new(Goal::new("test", "dependency chain"));
        plan.add_step(PlanStep::new("step1", "First", AgentAction::NoOp));
        plan.add_step(
            PlanStep::new("step2", "Second", AgentAction::NoOp)
                .depends_on_step("step1"),
        );
        plan.add_step(
            PlanStep::new("step3", "Third", AgentAction::NoOp)
                .depends_on_step("step2"),
        );

        let result = executor.execute(&mut plan).await.unwrap();
        assert!(result.success);
        assert_eq!(result.step_results.len(), 3);
        // Steps should execute in order
        assert_eq!(result.step_results[0].step_id, "step1");
        assert_eq!(result.step_results[1].step_id, "step2");
        assert_eq!(result.step_results[2].step_id, "step3");
    }

    #[test]
    fn test_goal_with_params() {
        let goal = Goal::new("voltage_violation", "test")
            .with_priority(2)
            .with_param("bus_id", "5")
            .with_param("target", "1.0");
        assert_eq!(goal.priority, 2);
        assert_eq!(goal.params.get("bus_id").unwrap(), "5");
        assert_eq!(goal.params.get("target").unwrap(), "1.0");
    }

    #[test]
    fn test_plan_step_builder() {
        let step = PlanStep::new("s1", "test", AgentAction::NoOp)
            .depends_on_step("s0")
            .with_precondition("condition A")
            .with_expected_outcome("result A");
        assert_eq!(step.step_id, "s1");
        assert!(step.depends_on.contains(&"s0".to_string()));
        assert!(step.preconditions.contains(&"condition A".to_string()));
        assert_eq!(step.expected_outcome, "result A");
    }
}
