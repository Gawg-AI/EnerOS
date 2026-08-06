//! Runbook 步骤定义 (OBS-012).
//!
//! 每个 runbook 包含一系列有序恢复步骤，每步有描述和验证条件。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Runbook 步骤类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunbookStepType {
    /// 诊断步骤（收集信息）。
    Diagnose,
    /// 修复步骤（执行操作）。
    Remediate,
    /// 验证步骤（确认修复生效）。
    Verify,
    /// 升级步骤（转交其他团队）。
    Escalate,
}

impl RunbookStepType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Diagnose => "DIAGNOSE",
            Self::Remediate => "REMEDIATE",
            Self::Verify => "VERIFY",
            Self::Escalate => "ESCALATE",
        }
    }
}

/// 单个 runbook 步骤。
#[derive(Debug, Clone)]
pub struct RunbookStep {
    /// 步骤描述（做什么）。
    pub description: String,
    /// 验证条件（如何确认完成）。
    pub verification: String,
    /// 步骤类型。
    pub step_type: RunbookStepType,
}

impl RunbookStep {
    /// 创建诊断步骤。
    pub fn new(description: &str, verification: &str) -> Self {
        Self {
            description: String::from(description),
            verification: String::from(verification),
            step_type: RunbookStepType::Diagnose,
        }
    }

    /// 创建修复步骤。
    pub fn remediate(description: &str, verification: &str) -> Self {
        Self {
            description: String::from(description),
            verification: String::from(verification),
            step_type: RunbookStepType::Remediate,
        }
    }

    /// 创建验证步骤。
    pub fn verify(description: &str, verification: &str) -> Self {
        Self {
            description: String::from(description),
            verification: String::from(verification),
            step_type: RunbookStepType::Verify,
        }
    }

    /// 创建升级步骤。
    pub fn escalate(description: &str, verification: &str) -> Self {
        Self {
            description: String::from(description),
            verification: String::from(verification),
            step_type: RunbookStepType::Escalate,
        }
    }
}

/// Runbook — 恢复步骤集合。
#[derive(Debug, Clone)]
pub struct Runbook {
    /// Runbook ID。
    pub id: &'static str,
    /// 步骤列表。
    pub steps: Vec<RunbookStep>,
}

impl Runbook {
    /// 创建新 runbook。
    pub fn new(id: &'static str, steps: Vec<RunbookStep>) -> Self {
        Self { id, steps }
    }

    /// 返回步骤数。
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// 生成 runbook JSON。
    pub fn to_json(&self) -> String {
        let steps_json: Vec<String> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    r#"{{"step":{},"type":"{}","description":"{}","verification":"{}"}}"#,
                    i + 1,
                    s.step_type.label(),
                    s.description.replace('"', "\\\""),
                    s.verification.replace('"', "\\\""),
                )
            })
            .collect();
        format!(
            r#"{{"runbook_id":"{}","steps":[{}]}}"#,
            self.id,
            steps_json.join(","),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runbook_with_steps() {
        let rb = Runbook::new(
            "RNB-TEST",
            vec![
                RunbookStep::new("Check status", "Status output shows ok"),
                RunbookStep::remediate("Restart service", "Service is running"),
                RunbookStep::verify("Confirm recovery", "No alerts firing"),
            ],
        );
        assert_eq!(rb.step_count(), 3);
    }

    #[test]
    fn empty_runbook() {
        let rb = Runbook::new("RNB-EMPTY", vec![]);
        assert_eq!(rb.step_count(), 0);
    }

    #[test]
    fn runbook_json_contains_all_steps() {
        let rb = Runbook::new(
            "RNB-001",
            vec![
                RunbookStep::new("Step 1", "Verify 1"),
                RunbookStep::escalate("Step 2", "Verify 2"),
            ],
        );
        let json = rb.to_json();
        assert!(json.contains("RNB-001"));
        assert!(json.contains("Step 1"));
        assert!(json.contains("Step 2"));
        assert!(json.contains("DIAGNOSE"));
        assert!(json.contains("ESCALATE"));
    }

    #[test]
    fn step_types_are_distinct() {
        assert_ne!(RunbookStepType::Diagnose, RunbookStepType::Remediate);
        assert_ne!(RunbookStepType::Verify, RunbookStepType::Escalate);
    }

    #[test]
    fn step_type_labels() {
        assert_eq!(RunbookStepType::Diagnose.label(), "DIAGNOSE");
        assert_eq!(RunbookStepType::Remediate.label(), "REMEDIATE");
        assert_eq!(RunbookStepType::Verify.label(), "VERIFY");
        assert_eq!(RunbookStepType::Escalate.label(), "ESCALATE");
    }
}
