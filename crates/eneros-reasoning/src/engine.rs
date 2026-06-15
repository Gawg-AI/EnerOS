use async_trait::async_trait;
use eneros_core::{PowerObservation, Result, SeverityLevel};
use eneros_memory::MemoryEntry;
use eneros_tool::ToolInfo;
use serde::{Deserialize, Serialize};

/// Reasoning engine trait
#[async_trait]
pub trait ReasoningEngine: Send + Sync {
    /// Engine name
    fn name(&self) -> &str;

    /// Perform reasoning
    async fn reason(&self, input: ReasoningInput) -> Result<ReasoningOutput>;
}

/// Input for reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningInput {
    /// Reasoning goal
    pub goal: String,
    /// Observed facts
    pub observations: Vec<String>,
    /// Constraints to respect
    pub constraints: Vec<String>,
    /// Related memory entries
    pub memory_entries: Vec<MemoryEntry>,
    /// Available tools
    pub available_tools: Vec<ToolInfo>,
    /// Structured power system observation
    pub power_observation: Option<PowerObservation>,
}

impl ReasoningInput {
    /// Create a new reasoning input
    pub fn new(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            observations: Vec::new(),
            constraints: Vec::new(),
            memory_entries: Vec::new(),
            available_tools: Vec::new(),
            power_observation: None,
        }
    }

    /// Add an observation
    pub fn with_observation(mut self, obs: &str) -> Self {
        self.observations.push(obs.to_string());
        self
    }

    /// Add a constraint
    pub fn with_constraint(mut self, constraint: &str) -> Self {
        self.constraints.push(constraint.to_string());
        self
    }

    /// Add memory entries
    pub fn with_memory(mut self, entries: Vec<MemoryEntry>) -> Self {
        self.memory_entries = entries;
        self
    }

    /// Add available tools
    pub fn with_tools(mut self, tools: Vec<ToolInfo>) -> Self {
        self.available_tools = tools;
        self
    }

    /// Add a power observation
    pub fn with_power_observation(mut self, obs: PowerObservation) -> Self {
        self.power_observation = Some(obs);
        self
    }
}

/// Output from reasoning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningOutput {
    /// Conclusion
    pub conclusion: String,
    /// Confidence level (0.0~1.0)
    pub confidence: f64,
    /// Suggested actions
    pub actions: Vec<String>,
    /// Reasoning chain (explainability)
    pub reasoning_chain: Vec<String>,
    /// Structured actions (when available from LLM reasoning)
    pub structured_actions: Option<Vec<eneros_core::StructuredAction>>,
    /// Preconditions that must be satisfied before executing actions
    pub preconditions: Vec<String>,
}

impl ReasoningOutput {
    /// Create a new reasoning output
    pub fn new(conclusion: &str, confidence: f64) -> Self {
        Self {
            conclusion: conclusion.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            actions: Vec::new(),
            reasoning_chain: Vec::new(),
            structured_actions: None,
            preconditions: Vec::new(),
        }
    }

    /// Add an action
    pub fn with_action(mut self, action: &str) -> Self {
        self.actions.push(action.to_string());
        self
    }

    /// Add a reasoning step
    pub fn with_step(mut self, step: &str) -> Self {
        self.reasoning_chain.push(step.to_string());
        self
    }

    /// Create from a StructuredActionOutput
    pub fn from_structured(output: crate::structured_output::StructuredActionOutput) -> Self {
        Self {
            conclusion: output.reasoning_chain.clone(),
            confidence: output.confidence,
            actions: output.actions.iter().map(|a| format!("{:?}", a)).collect(),
            reasoning_chain: vec![output.reasoning_chain],
            structured_actions: Some(output.actions),
            preconditions: output.preconditions,
        }
    }
}

/// Which field of PowerObservation to check
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NumericField {
    Frequency,
    MinBusVoltage,
    MaxBusVoltage,
    MaxBranchLoading,
    TotalLoad,
    TotalGeneration,
}

/// Comparison operator for numeric rules
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComparisonOperator {
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
}

impl ComparisonOperator {
    /// Apply the comparison operator to a value and threshold
    pub fn compare(&self, value: f64, threshold: f64) -> bool {
        match self {
            ComparisonOperator::LessThan => value < threshold,
            ComparisonOperator::LessThanOrEqual => value <= threshold,
            ComparisonOperator::GreaterThan => value > threshold,
            ComparisonOperator::GreaterThanOrEqual => value >= threshold,
            ComparisonOperator::Equal => (value - threshold).abs() < f64::EPSILON,
        }
    }

    /// Get the symbol for this operator
    pub fn symbol(&self) -> &str {
        match self {
            ComparisonOperator::LessThan => "<",
            ComparisonOperator::LessThanOrEqual => "<=",
            ComparisonOperator::GreaterThan => ">",
            ComparisonOperator::GreaterThanOrEqual => ">=",
            ComparisonOperator::Equal => "==",
        }
    }
}

/// Numeric comparison rule for structured power observations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericRule {
    /// Rule identifier
    pub rule_id: String,
    /// Which field to check
    pub field: NumericField,
    /// Comparison operator
    pub operator: ComparisonOperator,
    /// Threshold value
    pub threshold: f64,
    /// Action template when rule fires ({value} and {threshold} are replaced)
    pub action_template: String,
    /// Severity when rule fires
    pub severity: SeverityLevel,
}

impl NumericRule {
    /// Extract the relevant value from a PowerObservation for this rule's field
    fn extract_value(&self, obs: &PowerObservation) -> f64 {
        match self.field {
            NumericField::Frequency => obs.frequency_hz,
            NumericField::MinBusVoltage => obs
                .bus_voltages
                .values()
                .map(|v| v.vm_pu)
                .fold(f64::INFINITY, f64::min),
            NumericField::MaxBusVoltage => obs
                .bus_voltages
                .values()
                .map(|v| v.vm_pu)
                .fold(f64::NEG_INFINITY, f64::max),
            NumericField::MaxBranchLoading => obs
                .branch_flows
                .values()
                .map(|v| v.loading_percent)
                .fold(f64::NEG_INFINITY, f64::max),
            NumericField::TotalLoad => obs.total_load_mw,
            NumericField::TotalGeneration => obs.total_gen_mw,
        }
    }

    /// Check if this rule is triggered by the given observation
    pub fn evaluate(&self, obs: &PowerObservation) -> Option<NumericRuleResult> {
        let value = self.extract_value(obs);
        if self.operator.compare(value, self.threshold) {
            let action = self.format_action(value);
            Some(NumericRuleResult {
                rule_id: self.rule_id.clone(),
                field: self.field.clone(),
                value,
                threshold: self.threshold,
                operator: self.operator.clone(),
                action,
                severity: self.severity,
            })
        } else {
            None
        }
    }

    /// Format the action template, replacing {value:.N} and {threshold:.N} placeholders
    fn format_action(&self, value: f64) -> String {
        let mut result = self.action_template.clone();
        // Replace {value:.N} patterns (e.g., {value:.2}, {value:.3}, {value:.1})
        while let Some(start) = result.find("{value:") {
            let end = result[start..].find('}').map(|i| start + i + 1);
            if let Some(end) = end {
                let placeholder = &result[start..end];
                // Extract precision from {value:.N}
                if let Some(precision_str) = placeholder.strip_prefix("{value:.")
                    .and_then(|s| s.strip_suffix('}'))
                {
                    if let Ok(precision) = precision_str.parse::<usize>() {
                        let formatted = format!("{:.*}", precision, value);
                        result = format!("{}{}{}", &result[..start], formatted, &result[end..]);
                        continue;
                    }
                }
            }
            break;
        }
        // Replace plain {value}
        result = result.replace("{value}", &format!("{:.2}", value));
        // Replace {threshold:.N} patterns
        while let Some(start) = result.find("{threshold:") {
            let end = result[start..].find('}').map(|i| start + i + 1);
            if let Some(end) = end {
                let placeholder = &result[start..end];
                if let Some(precision_str) = placeholder.strip_prefix("{threshold:.")
                    .and_then(|s| s.strip_suffix('}'))
                {
                    if let Ok(precision) = precision_str.parse::<usize>() {
                        let formatted = format!("{:.*}", precision, self.threshold);
                        result = format!("{}{}{}", &result[..start], formatted, &result[end..]);
                        continue;
                    }
                }
            }
            break;
        }
        // Replace plain {threshold}
        result = result.replace("{threshold}", &format!("{:.2}", self.threshold));
        result
    }
}

/// Result of a triggered numeric rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumericRuleResult {
    /// Rule identifier
    pub rule_id: String,
    /// Field that was checked
    pub field: NumericField,
    /// Actual value
    pub value: f64,
    /// Threshold value
    pub threshold: f64,
    /// Comparison operator
    pub operator: ComparisonOperator,
    /// Formatted action
    pub action: String,
    /// Severity level
    pub severity: SeverityLevel,
}

/// A single reasoning rule
#[derive(Debug, Clone)]
struct ReasoningRule {
    name: String,
    condition: fn(&ReasoningInput) -> bool,
    conclusion: String,
    actions: Vec<String>,
    priority: f64,
}

/// Rule-based reasoning engine for power system domain
pub struct RuleBasedEngine {
    rules: Vec<ReasoningRule>,
    pub numeric_rules: Vec<NumericRule>,
}

impl RuleBasedEngine {
    /// Create a new rule-based engine with built-in power system rules
    pub fn new() -> Self {
        let rules = vec![
            ReasoningRule {
                name: "voltage_violation_low".to_string(),
                condition: |input| {
                    input.observations.iter().any(|o| {
                        o.to_lowercase().contains("voltage") && o.to_lowercase().contains("low")
                    })
                },
                conclusion: "Low voltage violation detected".to_string(),
                actions: vec![
                    "Adjust transformer tap position".to_string(),
                    "Switch capacitor bank ON".to_string(),
                    "Increase generator reactive output".to_string(),
                ],
                priority: 0.9,
            },
            ReasoningRule {
                name: "voltage_violation_high".to_string(),
                condition: |input| {
                    input.observations.iter().any(|o| {
                        o.to_lowercase().contains("voltage") && o.to_lowercase().contains("high")
                    })
                },
                conclusion: "High voltage violation detected".to_string(),
                actions: vec![
                    "Switch reactor ON".to_string(),
                    "Switch capacitor bank OFF".to_string(),
                    "Decrease generator reactive output".to_string(),
                ],
                priority: 0.9,
            },
            ReasoningRule {
                name: "thermal_overload".to_string(),
                condition: |input| {
                    input.observations.iter().any(|o| {
                        o.to_lowercase().contains("overload") || o.to_lowercase().contains("thermal")
                    })
                },
                conclusion: "Thermal overload detected".to_string(),
                actions: vec![
                    "Redistribute power flow".to_string(),
                    "Activate alternative transmission path".to_string(),
                    "Reduce generation at source bus".to_string(),
                ],
                priority: 0.95,
            },
            ReasoningRule {
                name: "frequency_deviation".to_string(),
                condition: |input| {
                    input.observations.iter().any(|o| {
                        o.to_lowercase().contains("frequency") &&
                        (o.to_lowercase().contains("high") || o.to_lowercase().contains("low"))
                    })
                },
                conclusion: "Frequency deviation detected".to_string(),
                actions: vec![
                    "Adjust generator active power output".to_string(),
                    "Activate load shedding if necessary".to_string(),
                ],
                priority: 0.95,
            },
            ReasoningRule {
                name: "n1_violation".to_string(),
                condition: |input| {
                    input.observations.iter().any(|o| {
                        o.to_lowercase().contains("n-1") && o.to_lowercase().contains("violation")
                    })
                },
                conclusion: "N-1 security violation detected".to_string(),
                actions: vec![
                    "Run full N-1 analysis".to_string(),
                    "Identify critical contingencies".to_string(),
                    "Prepare corrective actions".to_string(),
                ],
                priority: 0.85,
            },
        ];

        Self { rules, numeric_rules: Vec::new() }
    }

    /// Create a rule-based engine with default numeric rules for power system monitoring
    pub fn with_default_numeric_rules() -> Self {
        let mut engine = Self::new();
        engine.numeric_rules.push(NumericRule {
            rule_id: "FREQ_LOW".to_string(),
            field: NumericField::Frequency,
            operator: ComparisonOperator::LessThan,
            threshold: 49.8,
            action_template: "频率偏低 ({value:.2}Hz < {threshold}Hz)，建议增加发电出力".to_string(),
            severity: SeverityLevel::Major,
        });
        engine.numeric_rules.push(NumericRule {
            rule_id: "FREQ_CRITICAL".to_string(),
            field: NumericField::Frequency,
            operator: ComparisonOperator::LessThan,
            threshold: 49.5,
            action_template: "频率严重偏低 ({value:.2}Hz < {threshold}Hz)，建议紧急切负荷".to_string(),
            severity: SeverityLevel::Critical,
        });
        engine.numeric_rules.push(NumericRule {
            rule_id: "VOLTAGE_LOW".to_string(),
            field: NumericField::MinBusVoltage,
            operator: ComparisonOperator::LessThan,
            threshold: 0.95,
            action_template: "电压偏低 (最低{value:.2}pu < {threshold}pu)，建议投入无功补偿".to_string(),
            severity: SeverityLevel::Major,
        });
        engine.numeric_rules.push(NumericRule {
            rule_id: "VOLTAGE_HIGH".to_string(),
            field: NumericField::MaxBusVoltage,
            operator: ComparisonOperator::GreaterThan,
            threshold: 1.05,
            action_template: "电压偏高 (最高{value:.2}pu > {threshold}pu)，建议切除电容器".to_string(),
            severity: SeverityLevel::Major,
        });
        engine.numeric_rules.push(NumericRule {
            rule_id: "BRANCH_OVERLOAD".to_string(),
            field: NumericField::MaxBranchLoading,
            operator: ComparisonOperator::GreaterThan,
            threshold: 100.0,
            action_template: "支路过载 (最高{value:.2}% > {threshold}%)，建议转移负荷".to_string(),
            severity: SeverityLevel::Major,
        });
        engine
    }
}

impl Default for RuleBasedEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ReasoningEngine for RuleBasedEngine {
    fn name(&self) -> &str {
        "rule_based"
    }

    async fn reason(&self, input: ReasoningInput) -> Result<ReasoningOutput> {
        let mut matched_rules: Vec<&ReasoningRule> = self
            .rules
            .iter()
            .filter(|rule| (rule.condition)(&input))
            .collect();

        // Sort by priority (highest first)
        matched_rules.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal));

        // Evaluate numeric rules against power observation
        let mut numeric_results: Vec<NumericRuleResult> = Vec::new();
        if let Some(ref obs) = input.power_observation {
            for rule in &self.numeric_rules {
                if let Some(result) = rule.evaluate(obs) {
                    numeric_results.push(result);
                }
            }
            // Sort by severity (Critical first)
            numeric_results.sort_by_key(|b| std::cmp::Reverse(b.severity));
        }

        let has_string_match = !matched_rules.is_empty();
        let has_numeric_match = !numeric_results.is_empty();

        if !has_string_match && !has_numeric_match {
            return Ok(ReasoningOutput::new("No matching rule found", 0.3)
                .with_step("Evaluated all rules, no condition matched")
                .with_action("Monitor and wait for more information"));
        }

        // Determine primary conclusion from string rules or numeric rules
        let (conclusion, confidence) = if has_string_match {
            let best_rule = matched_rules[0];
            (best_rule.conclusion.clone(), best_rule.priority)
        } else {
            // Use highest-severity numeric rule as conclusion
            let first = &numeric_results[0];
            (format!("Numeric rule triggered: {}", first.rule_id), 0.9)
        };

        let mut output = ReasoningOutput::new(&conclusion, confidence);

        // Build reasoning chain from string rules
        if has_string_match {
            let best_rule = matched_rules[0];
            output = output.with_step(&format!("Matched rule: {}", best_rule.name));
            output = output.with_step(&format!("Rule priority: {:.2}", best_rule.priority));

            for obs in &input.observations {
                output = output.with_step(&format!("Observation: {}", obs));
            }

            // Add actions from string rule
            for action in &best_rule.actions {
                output = output.with_action(action);
            }

            // If multiple rules matched, add secondary recommendations
            if matched_rules.len() > 1 {
                output = output.with_step(&format!("Also matched {} other rules", matched_rules.len() - 1));
                for rule in matched_rules.iter().skip(1) {
                    output = output.with_step(&format!("Secondary: {} (priority {:.2})", rule.name, rule.priority));
                }
            }
        }

        // Add numeric rule results to reasoning chain and actions
        for result in &numeric_results {
            output = output.with_step(&format!(
                "Numeric rule [{}]: {} {} {:.2} (actual: {:.2})",
                result.rule_id,
                match result.field {
                    NumericField::Frequency => "Frequency",
                    NumericField::MinBusVoltage => "MinBusVoltage",
                    NumericField::MaxBusVoltage => "MaxBusVoltage",
                    NumericField::MaxBranchLoading => "MaxBranchLoading",
                    NumericField::TotalLoad => "TotalLoad",
                    NumericField::TotalGeneration => "TotalGeneration",
                },
                result.operator.symbol(),
                result.threshold,
                result.value,
            ));
            output = output.with_action(&result.action);
        }

        // Check memory for relevant past experiences
        for entry in &input.memory_entries {
            if entry.importance > 0.7 {
                output = output.with_step(&format!("Recalled: {}", entry.content));
            }
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::agentos_types::{BusVoltageObservation, BranchFlowObservation};
    use eneros_memory::MemoryType;

    #[tokio::test]
    async fn test_rule_based_voltage_low() {
        let engine = RuleBasedEngine::new();
        let input = ReasoningInput::new("Handle voltage violation")
            .with_observation("Bus 3 voltage low: 0.88 pu")
            .with_constraint("Voltage must be within 0.95-1.05 pu");

        let output = engine.reason(input).await.unwrap();
        assert!(output.conclusion.to_lowercase().contains("low voltage"));
        assert!(output.confidence > 0.8);
        assert!(!output.actions.is_empty());
    }

    #[tokio::test]
    async fn test_rule_based_overload() {
        let engine = RuleBasedEngine::new();
        let input = ReasoningInput::new("Handle thermal issue")
            .with_observation("Branch 5-6 thermal overload at 120%");

        let output = engine.reason(input).await.unwrap();
        assert!(output.conclusion.to_lowercase().contains("thermal"));
        assert!(!output.actions.is_empty());
    }

    #[tokio::test]
    async fn test_rule_based_no_match() {
        let engine = RuleBasedEngine::new();
        let input = ReasoningInput::new("Unknown situation")
            .with_observation("System is operating normally");

        let output = engine.reason(input).await.unwrap();
        assert!(output.confidence < 0.5);
    }

    #[tokio::test]
    async fn test_reasoning_with_memory() {
        let engine = RuleBasedEngine::new();
        let memory_entry = MemoryEntry::new(
            MemoryType::Procedural,
            "Previous voltage issue resolved by adjusting tap".to_string(),
            0.8,
        );

        let input = ReasoningInput::new("Handle voltage violation")
            .with_observation("Bus 5 voltage low: 0.92 pu")
            .with_memory(vec![memory_entry]);

        let output = engine.reason(input).await.unwrap();
        assert!(output.reasoning_chain.iter().any(|s| s.contains("Recalled")));
    }

    // === Numeric rule tests ===

    #[tokio::test]
    async fn test_reasoning_input_with_power_observation() {
        let mut obs = PowerObservation::empty();
        obs.frequency_hz = 49.7;
        let input = ReasoningInput::new("Check system status")
            .with_power_observation(obs);

        assert!(input.power_observation.is_some());
        assert_eq!(input.power_observation.unwrap().frequency_hz, 49.7);
    }

    #[tokio::test]
    async fn test_numeric_rule_frequency_low() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        let mut obs = PowerObservation::empty();
        obs.frequency_hz = 49.4; // Below both 49.8 and 49.5

        let input = ReasoningInput::new("Check frequency")
            .with_power_observation(obs);

        let output = engine.reason(input).await.unwrap();
        // Both FREQ_LOW and FREQ_CRITICAL should trigger
        let freq_actions: Vec<_> = output.actions.iter()
            .filter(|a| a.contains("频率"))
            .collect();
        assert!(freq_actions.len() >= 2, "Expected at least 2 frequency actions, got: {:?}", freq_actions);
        assert!(output.reasoning_chain.iter().any(|s| s.contains("FREQ_LOW")));
        assert!(output.reasoning_chain.iter().any(|s| s.contains("FREQ_CRITICAL")));
    }

    #[tokio::test]
    async fn test_numeric_rule_voltage_low() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 0.90, va_degree: 0.0 });
        obs.bus_voltages.insert(2, BusVoltageObservation { vm_pu: 0.96, va_degree: -1.0 });

        let input = ReasoningInput::new("Check voltage")
            .with_power_observation(obs);

        let output = engine.reason(input).await.unwrap();
        assert!(output.actions.iter().any(|a| a.contains("电压偏低")));
        assert!(output.reasoning_chain.iter().any(|s| s.contains("VOLTAGE_LOW")));
    }

    #[tokio::test]
    async fn test_numeric_rule_branch_overload() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        let mut obs = PowerObservation::empty();
        obs.branch_flows.insert(10, BranchFlowObservation { p_mw: 100.0, q_mvar: 20.0, loading_percent: 120.0 });

        let input = ReasoningInput::new("Check branch loading")
            .with_power_observation(obs);

        let output = engine.reason(input).await.unwrap();
        assert!(output.actions.iter().any(|a| a.contains("支路过载")));
        assert!(output.reasoning_chain.iter().any(|s| s.contains("BRANCH_OVERLOAD")));
    }

    #[tokio::test]
    async fn test_with_default_numeric_rules() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        assert_eq!(engine.numeric_rules.len(), 5);
        assert!(engine.numeric_rules.iter().any(|r| r.rule_id == "FREQ_LOW"));
        assert!(engine.numeric_rules.iter().any(|r| r.rule_id == "FREQ_CRITICAL"));
        assert!(engine.numeric_rules.iter().any(|r| r.rule_id == "VOLTAGE_LOW"));
        assert!(engine.numeric_rules.iter().any(|r| r.rule_id == "VOLTAGE_HIGH"));
        assert!(engine.numeric_rules.iter().any(|r| r.rule_id == "BRANCH_OVERLOAD"));
    }

    #[tokio::test]
    async fn test_string_rules_still_work() {
        let engine = RuleBasedEngine::new();
        let input = ReasoningInput::new("Handle voltage violation")
            .with_observation("Bus 3 voltage low: 0.88 pu");

        let output = engine.reason(input).await.unwrap();
        assert!(output.conclusion.to_lowercase().contains("low voltage"));
        assert!(output.confidence > 0.8);
        assert!(!output.actions.is_empty());
    }

    #[tokio::test]
    async fn test_numeric_rule_no_trigger_when_normal() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        let obs = PowerObservation::empty(); // frequency 50.0, no buses, no branches

        let input = ReasoningInput::new("Check system")
            .with_power_observation(obs);

        let output = engine.reason(input).await.unwrap();
        // No string rules match and no numeric rules should trigger for normal state
        assert!(output.confidence < 0.5);
    }

    #[tokio::test]
    async fn test_numeric_rule_voltage_high() {
        let engine = RuleBasedEngine::with_default_numeric_rules();
        let mut obs = PowerObservation::empty();
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 1.08, va_degree: 0.0 });

        let input = ReasoningInput::new("Check voltage")
            .with_power_observation(obs);

        let output = engine.reason(input).await.unwrap();
        assert!(output.actions.iter().any(|a| a.contains("电压偏高")));
        assert!(output.reasoning_chain.iter().any(|s| s.contains("VOLTAGE_HIGH")));
    }

    #[test]
    fn test_comparison_operator() {
        assert!(ComparisonOperator::LessThan.compare(3.0, 5.0));
        assert!(!ComparisonOperator::LessThan.compare(5.0, 3.0));
        assert!(ComparisonOperator::LessThanOrEqual.compare(5.0, 5.0));
        assert!(ComparisonOperator::GreaterThan.compare(5.0, 3.0));
        assert!(ComparisonOperator::GreaterThanOrEqual.compare(5.0, 5.0));
        assert!(ComparisonOperator::Equal.compare(5.0, 5.0));
    }

    #[test]
    fn test_numeric_rule_evaluate() {
        let rule = NumericRule {
            rule_id: "TEST_FREQ".to_string(),
            field: NumericField::Frequency,
            operator: ComparisonOperator::LessThan,
            threshold: 49.8,
            action_template: "频率低 {value} < {threshold}".to_string(),
            severity: SeverityLevel::Major,
        };

        let mut obs = PowerObservation::empty();
        obs.frequency_hz = 49.5;

        let result = rule.evaluate(&obs).unwrap();
        assert_eq!(result.rule_id, "TEST_FREQ");
        assert_eq!(result.value, 49.5);
        assert_eq!(result.threshold, 49.8);
        assert!(result.action.contains("49.50"));
        assert!(result.action.contains("49.80"));

        // Normal frequency should not trigger
        obs.frequency_hz = 50.0;
        assert!(rule.evaluate(&obs).is_none());
    }
}
