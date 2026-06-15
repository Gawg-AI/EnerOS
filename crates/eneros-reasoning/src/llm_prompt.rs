use eneros_core::{Result, StructuredAction};
use serde::Deserialize;

use crate::engine::{ReasoningInput, ReasoningOutput};
use crate::structured_output::StructuredActionOutput;

/// Build a structured prompt for the LLM from the reasoning input.
pub fn build_power_system_prompt(input: &ReasoningInput) -> String {
    let mut prompt = String::new();

    // Goal
    prompt.push_str(&format!("## Goal\n{}\n\n", input.goal));

    // Observations
    if !input.observations.is_empty() {
        prompt.push_str("## Observations\n");
        for obs in &input.observations {
            prompt.push_str(&format!("- {}\n", obs));
        }
        prompt.push('\n');
    }

    // Constraints
    if !input.constraints.is_empty() {
        prompt.push_str("## Constraints (rules to respect)\n");
        for c in &input.constraints {
            prompt.push_str(&format!("- {}\n", c));
        }
        prompt.push('\n');
    }

    // Power observation
    if let Some(ref obs) = input.power_observation {
        prompt.push_str("## Power System Data\n");
        prompt.push_str(&obs.summary());
        prompt.push_str("\n\n");
    }

    // Available tools
    if !input.available_tools.is_empty() {
        prompt.push_str("## Available Tools\n");
        for tool in &input.available_tools {
            prompt.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
        }
        prompt.push('\n');
    }

    // Output format instruction
    prompt.push_str(
        "## Output Format\n\
         Respond with a JSON object containing:\n\
         - \"conclusion\": a concise summary of your analysis (string)\n\
         - \"confidence\": your confidence level from 0.0 to 1.0 (number)\n\
         - \"actions\": recommended actions as human-readable strings (array of strings)\n\
         - \"structured_actions\": RECOMMENDED — machine-executable actions (array of objects).\n\
           Each object MUST have an \"action_type\" tag plus its parameters. Valid types:\n\
             {\"action_type\": \"StartGenerator\", \"gen_id\": <u64>, \"target_mw\": <f64>}\n\
             {\"action_type\": \"ShedLoad\", \"zone_id\": <u32>, \"amount_mw\": <f64>}\n\
             {\"action_type\": \"ExecuteDevice\", \"device_id\": <u64>, \"operation\": \"<open|close|adjust_reactive>\", \"value\": <f64>}\n\
             {\"action_type\": \"IsolateFault\", \"upstream_switch\": <u64>, \"downstream_switch\": <u64>}\n\
             {\"action_type\": \"CloseTieSwitch\", \"switch_id\": <u64>}\n\
             {\"action_type\": \"NotifyAgent\", \"agent_id\": \"<string>\", \"message\": \"<string>\"}\n\
         - \"preconditions\": conditions that must hold before executing (array of strings, optional)\n\
         - \"reasoning_chain\": step-by-step reasoning (array of strings)\n",
    );

    prompt
}

/// Tagged intermediate representation that deserializes from the LLM's JSON
/// `structured_actions` array. The `#[serde(tag = ...)]` form maps a single
/// `"action_type"` discriminator to a `StructuredAction` variant one-to-one.
///
/// We keep this as a thin wrapper (rather than deserializing `StructuredAction`
/// directly) so that unknown/malformed entries from the LLM degrade gracefully
/// instead of failing the entire parse.
#[derive(Debug, Deserialize)]
#[serde(tag = "action_type")]
enum RawStructuredAction {
    StartGenerator { gen_id: u64, target_mw: f64 },
    ShedLoad { zone_id: u32, amount_mw: f64 },
    ExecuteDevice { device_id: u64, operation: String, value: f64 },
    IsolateFault { upstream_switch: u64, downstream_switch: u64 },
    CloseTieSwitch { switch_id: u64 },
    NotifyAgent { agent_id: String, message: String },
}

impl From<RawStructuredAction> for StructuredAction {
    fn from(raw: RawStructuredAction) -> Self {
        match raw {
            RawStructuredAction::StartGenerator { gen_id, target_mw } => {
                StructuredAction::StartGenerator { gen_id, target_mw }
            }
            RawStructuredAction::ShedLoad { zone_id, amount_mw } => {
                StructuredAction::ShedLoad { zone_id, amount_mw }
            }
            RawStructuredAction::ExecuteDevice { device_id, operation, value } => {
                StructuredAction::ExecuteDevice { device_id, operation, value }
            }
            RawStructuredAction::IsolateFault { upstream_switch, downstream_switch } => {
                StructuredAction::IsolateFault { upstream_switch, downstream_switch }
            }
            RawStructuredAction::CloseTieSwitch { switch_id } => {
                StructuredAction::CloseTieSwitch { switch_id }
            }
            RawStructuredAction::NotifyAgent { agent_id, message } => {
                StructuredAction::NotifyAgent { agent_id, message }
            }
        }
    }
}

/// Parse the `structured_actions` JSON array (already extracted) into typed
/// `StructuredAction`s. Malformed entries are skipped rather than failing the
/// whole list — this is the LLM-facing "best effort" boundary.
fn parse_structured_actions(json_array_str: &str) -> Vec<StructuredAction> {
    // Deserialize as a slice of `serde_json::Value` so we can tolerate per-item
    // failures without aborting the entire array.
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(json_array_str) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawStructuredAction>(v).ok().map(Into::into))
        .collect()
}

/// Intermediate struct for JSON parsing of LLM response.
#[derive(Debug, Deserialize)]
struct LlmResponseJson {
    conclusion: String,
    confidence: f64,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    reasoning_chain: Vec<String>,
    /// Raw JSON value of the optional `structured_actions` array; kept as
    /// `Value` so its absence or malformation never fails the top-level parse.
    #[serde(default)]
    structured_actions: Option<serde_json::Value>,
    #[serde(default)]
    preconditions: Vec<String>,
}

/// Parse the LLM response text into a ReasoningOutput.
///
/// First attempts JSON parsing; if that fails, falls back to text extraction.
/// When the JSON carries a valid `structured_actions` array, the output's
/// `structured_actions` field is populated (reviving the previously-dead
/// `ReasoningOutput::from_structured` path) and feeds the constrained
/// decision pipeline. Malformed entries are silently skipped.
pub fn parse_llm_response(response: &str) -> Result<ReasoningOutput> {
    // Try JSON parsing first — look for a JSON block in the response
    let json_text = extract_json_block(response);

    if let Ok(parsed) = serde_json::from_str::<LlmResponseJson>(&json_text) {
        // Best-effort parse of the structured_actions array, if present.
        let structured: Vec<StructuredAction> = parsed
            .structured_actions
            .as_ref()
            .and_then(|v| v.as_array().map(|_| v.to_string()))
            .map(|s| parse_structured_actions(&s))
            .unwrap_or_default();

        if !structured.is_empty() {
            // Revive the structured path: build via StructuredActionOutput so
            // that structured_actions is Some(...) and preconditions carry over.
            let mut builder = StructuredActionOutput::new(&parsed.conclusion, parsed.confidence);
            for action in &structured {
                builder = builder.with_action(action.clone());
            }
            for pre in &parsed.preconditions {
                builder = builder.with_precondition(pre);
            }
            let mut output = ReasoningOutput::from_structured(builder);
            // Keep human-readable actions + reasoning chain from the LLM too.
            if !parsed.actions.is_empty() {
                output.actions = parsed.actions;
            }
            if !parsed.reasoning_chain.is_empty() {
                output.reasoning_chain = parsed.reasoning_chain;
            }
            return Ok(output);
        }

        // No structured actions — fall back to the legacy string-only output.
        return Ok(ReasoningOutput {
            conclusion: parsed.conclusion,
            confidence: parsed.confidence.clamp(0.0, 1.0),
            actions: parsed.actions,
            reasoning_chain: parsed.reasoning_chain,
            structured_actions: None,
            preconditions: parsed.preconditions,
        });
    }

    // Fallback: extract from plain text
    let conclusion = extract_conclusion(response);
    let confidence = extract_confidence(response);
    let actions = extract_actions(response);
    let reasoning_chain = extract_reasoning_chain(response);

    Ok(ReasoningOutput {
        conclusion,
        confidence,
        actions,
        reasoning_chain,
        structured_actions: None,
        preconditions: Vec::new(),
    })
}

/// Try to extract a JSON block from the response (handles ```json ... ``` wrapping).
fn extract_json_block(response: &str) -> String {
    let trimmed = response.trim();

    // Try the whole response as JSON
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    // Try extracting from ```json ... ``` block
    if let Some(start_marker) = trimmed.find("```json") {
        let json_start = trimmed[start_marker..].find('\n').map(|i| start_marker + i + 1).unwrap_or(start_marker + 7);
        if let Some(end_marker) = trimmed[json_start..].find("```") {
            return trimmed[json_start..json_start + end_marker].trim().to_string();
        }
    }

    // Try extracting from ``` ... ``` block
    if let Some(start_marker) = trimmed.find("```") {
        let after_start = start_marker + 3;
        let json_start = trimmed[after_start..].find('\n').map(|i| after_start + i + 1).unwrap_or(after_start);
        if let Some(end_marker) = trimmed[json_start..].find("```") {
            let candidate = trimmed[json_start..json_start + end_marker].trim();
            if candidate.starts_with('{') {
                return candidate.to_string();
            }
        }
    }

    trimmed.to_string()
}

/// Extract conclusion from plain text: first paragraph or sentence.
fn extract_conclusion(text: &str) -> String {
    let text = text.trim();
    // Try first paragraph
    if let Some(para_end) = text.find("\n\n") {
        let para = text[..para_end].trim();
        if !para.is_empty() {
            return para.to_string();
        }
    }
    // Try first sentence
    if let Some(sent_end) = text.find(". ") {
        return text[..sent_end + 1].to_string();
    }
    // Fallback: first line
    if let Some(line_end) = text.find('\n') {
        return text[..line_end].trim().to_string();
    }
    if text.len() > 200 {
        text[..200].to_string()
    } else {
        text.to_string()
    }
}

/// Extract confidence from text, looking for patterns like "confidence: 0.8".
fn extract_confidence(text: &str) -> f64 {
    let lower = text.to_lowercase();

    // Look for "confidence: 0.8" or "confidence:0.8"
    for prefix in &["confidence:", "confidence :", "confidence ="] {
        if let Some(idx) = lower.find(prefix) {
            let after = &lower[idx + prefix.len()..].trim_start();
            if let Some(conf) = parse_leading_f64(after) {
                return conf.clamp(0.0, 1.0);
            }
        }
    }

    0.5
}

/// Parse a leading f64 from a string slice.
fn parse_leading_f64(s: &str) -> Option<f64> {
    let end = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end].parse().ok()
}

/// Extract actions from text: numbered lists or "recommend:" sections.
fn extract_actions(text: &str) -> Vec<String> {
    let mut actions = Vec::new();
    let lower = text.to_lowercase();

    // Look for "recommend:" or "recommendations:" sections
    if let Some(idx) = lower.find("recommend") {
        let section = &text[idx..];
        for line in section.lines() {
            let trimmed = line.trim();
            if let Some(action) = strip_list_item(trimmed) {
                actions.push(action);
            }
            if actions.len() >= 10 {
                break;
            }
        }
    }

    // If no "recommend" section found, look for numbered lists
    if actions.is_empty() {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(action) = strip_list_item(trimmed) {
                actions.push(action);
            }
            if actions.len() >= 10 {
                break;
            }
        }
    }

    actions
}

/// Strip a list item prefix (e.g. "1. ", "- ", "* ") and return the content.
fn strip_list_item(s: &str) -> Option<String> {
    // Numbered: "1. action" or "1) action"
    if let Some(rest) = s.strip_prefix(|c: char| c.is_ascii_digit()) {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            let content = rest.trim();
            if !content.is_empty() {
                return Some(content.to_string());
            }
        }
    }
    // Bullet: "- action" or "* action"
    if let Some(rest) = s.strip_prefix('-').or_else(|| s.strip_prefix('*')) {
        let content = rest.trim();
        if !content.is_empty() {
            return Some(content.to_string());
        }
    }
    None
}

/// Extract reasoning chain by splitting text into logical steps.
fn extract_reasoning_chain(text: &str) -> Vec<String> {
    let mut steps = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip lines that look like headers or JSON markers
        if trimmed.starts_with('#') || trimmed.starts_with("```") {
            continue;
        }
        steps.push(trimmed.to_string());
        if steps.len() >= 20 {
            break;
        }
    }

    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::agentos_types::{BusVoltageObservation, PowerObservation};
    use eneros_tool::ToolInfo;

    #[test]
    fn test_build_power_system_prompt() {
        let input = ReasoningInput::new("Analyze voltage violation")
            .with_observation("Bus 3 voltage low: 0.88 pu")
            .with_constraint("Voltage must be within 0.95-1.05 pu");

        let prompt = build_power_system_prompt(&input);

        assert!(prompt.contains("Analyze voltage violation"));
        assert!(prompt.contains("Bus 3 voltage low: 0.88 pu"));
        assert!(prompt.contains("Voltage must be within 0.95-1.05 pu"));
        assert!(prompt.contains("conclusion"));
        assert!(prompt.contains("confidence"));
        assert!(prompt.contains("actions"));
        assert!(prompt.contains("structured_actions"));
        assert!(prompt.contains("reasoning_chain"));
    }

    #[test]
    fn test_build_power_system_prompt_with_observation() {
        let mut obs = PowerObservation::empty();
        obs.frequency_hz = 49.7;
        obs.bus_voltages.insert(1, BusVoltageObservation { vm_pu: 0.90, va_degree: 0.0 });

        let input = ReasoningInput::new("Check system status")
            .with_power_observation(obs);

        let prompt = build_power_system_prompt(&input);

        assert!(prompt.contains("Power System Data"));
        assert!(prompt.contains("49.70Hz"));
    }

    #[test]
    fn test_build_power_system_prompt_with_tools() {
        let tool = ToolInfo {
            name: "power_flow".to_string(),
            description: "Run power flow analysis".to_string(),
            parameters_schema: serde_json::Value::Null,
        };

        let input = ReasoningInput::new("Analyze grid")
            .with_tools(vec![tool]);

        let prompt = build_power_system_prompt(&input);
        assert!(prompt.contains("power_flow"));
        assert!(prompt.contains("Run power flow analysis"));
    }

    #[test]
    fn test_parse_llm_response_json() {
        let json = r#"{
            "conclusion": "Low voltage detected at Bus 3",
            "confidence": 0.85,
            "actions": ["Switch capacitor bank ON", "Adjust transformer tap"],
            "reasoning_chain": ["Observed low voltage", "Identified corrective actions"]
        }"#;

        let output = parse_llm_response(json).unwrap();
        assert_eq!(output.conclusion, "Low voltage detected at Bus 3");
        assert!((output.confidence - 0.85).abs() < 0.01);
        assert_eq!(output.actions.len(), 2);
        assert_eq!(output.reasoning_chain.len(), 2);
    }

    #[test]
    fn test_parse_llm_response_json_in_code_block() {
        let response = "Here is my analysis:\n```json\n{\"conclusion\": \"OK\", \"confidence\": 0.9, \"actions\": [\"Monitor\"], \"reasoning_chain\": [\"System normal\"]}\n```";

        let output = parse_llm_response(response).unwrap();
        assert_eq!(output.conclusion, "OK");
        assert!((output.confidence - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_parse_llm_response_with_structured_actions() {
        let json = r#"{
            "conclusion": "Undervoltage at Bus 3 — start generator 1",
            "confidence": 0.9,
            "actions": ["Start generator 1 to 100 MW"],
            "reasoning_chain": ["Bus 3 voltage low"],
            "structured_actions": [
                {"action_type": "StartGenerator", "gen_id": 1, "target_mw": 100.0}
            ],
            "preconditions": ["Generator 1 must be available"]
        }"#;

        let output = parse_llm_response(json).unwrap();
        // structured_actions should now be populated
        let structured = output.structured_actions.expect("structured_actions should be Some");
        assert_eq!(structured.len(), 1);
        assert_eq!(
            structured[0],
            StructuredAction::StartGenerator { gen_id: 1, target_mw: 100.0 }
        );
        // Preconditions carry through
        assert_eq!(output.preconditions.len(), 1);
        assert!(output.preconditions[0].contains("available"));
        // Human-readable actions preserved
        assert_eq!(output.actions.len(), 1);
    }

    #[test]
    fn test_parse_llm_response_structured_actions_multiple() {
        let json = r#"{
            "conclusion": "Fault isolation plan",
            "confidence": 0.85,
            "structured_actions": [
                {"action_type": "IsolateFault", "upstream_switch": 10, "downstream_switch": 20},
                {"action_type": "ShedLoad", "zone_id": 2, "amount_mw": 50.0},
                {"action_type": "NotifyAgent", "agent_id": "dispatch", "message": "fault isolated"}
            ]
        }"#;

        let output = parse_llm_response(json).unwrap();
        let structured = output.structured_actions.expect("structured_actions should be Some");
        assert_eq!(structured.len(), 3);
        assert!(matches!(structured[0], StructuredAction::IsolateFault { .. }));
        assert!(matches!(structured[1], StructuredAction::ShedLoad { .. }));
        assert!(matches!(structured[2], StructuredAction::NotifyAgent { .. }));
    }

    #[test]
    fn test_parse_llm_response_structured_actions_malformed_skipped() {
        // A malformed entry (missing required field) must be skipped without
        // dropping the valid ones — best-effort boundary.
        let json = r#"{
            "conclusion": "mixed",
            "confidence": 0.7,
            "structured_actions": [
                {"action_type": "StartGenerator", "gen_id": 1, "target_mw": 100.0},
                {"action_type": "StartGenerator", "gen_id": 2},
                {"action_type": "UnknownType", "foo": 1}
            ]
        }"#;

        let output = parse_llm_response(json).unwrap();
        let structured = output.structured_actions.expect("structured_actions should be Some");
        // Only the first, well-formed entry survives.
        assert_eq!(structured.len(), 1);
    }

    #[test]
    fn test_parse_llm_response_structured_actions_absent() {
        // No structured_actions field — must behave exactly as before (None).
        let json = r#"{
            "conclusion": "normal",
            "confidence": 0.6,
            "actions": ["monitor"],
            "reasoning_chain": ["ok"]
        }"#;

        let output = parse_llm_response(json).unwrap();
        assert!(output.structured_actions.is_none());
        assert_eq!(output.actions, vec!["monitor".to_string()]);
    }

    #[test]
    fn test_parse_llm_response_text() {
        let text = "The system frequency is low at 49.7 Hz. This indicates a generation deficit.\n\n\
            Confidence: 0.8\n\n\
            Recommendations:\n\
            1. Increase generator output\n\
            2. Activate reserve generation\n\
            3. Prepare load shedding plan";

        let output = parse_llm_response(text).unwrap();
        assert!(!output.conclusion.is_empty());
        assert!((output.confidence - 0.8).abs() < 0.01);
        assert!(!output.actions.is_empty());
    }

    #[test]
    fn test_parse_llm_response_text_default_confidence() {
        let text = "System is operating normally. No issues detected.";
        let output = parse_llm_response(text).unwrap();
        assert!((output.confidence - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_extract_json_block_plain() {
        let json = r#"{"conclusion": "test", "confidence": 0.5, "actions": [], "reasoning_chain": []}"#;
        assert_eq!(extract_json_block(json), json);
    }

    #[test]
    fn test_extract_json_block_code_fence() {
        let wrapped = "```json\n{\"conclusion\": \"test\"}\n```";
        let result = extract_json_block(wrapped);
        assert!(result.starts_with('{'));
    }
}
