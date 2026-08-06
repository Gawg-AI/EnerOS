//! 隐私威胁评估 (DATA-010).
//!
//! MASTER §21.17 / G9: "privacy threat assessment — 控制与证据".
//!
//! 对数据采集、存储、传输、处理、删除全生命周期执行隐私威胁评估，
//! 将威胁映射到控制措施，确认残留风险可接受。
//!
//! # 证据推导
//!
//! 隐私评估通过结论由证据推导：
//! 1. 每个威胁至少有 1 个控制措施
//! 2. 每个控制措施已验证（verified == true）
//! 3. 无 Unacceptable 残留风险
//! 4. High/Critical 威胁残留风险必须为 Acceptable

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 威胁严重度.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreatSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// 威胁可能性.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLikelihood {
    Rare,
    Unlikely,
    Possible,
    Likely,
}

/// 残留风险等级.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidualRisk {
    /// 可接受（控制充分）.
    Acceptable,
    /// 需缓解（有控制但不够）.
    NeedsMitigation,
    /// 不可接受（无控制或控制失效）.
    Unacceptable,
}

/// 隐私威胁.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyThreat {
    /// 威胁 ID.
    pub threat_id: String,
    /// 威胁描述（如 "遥测数据含 PII 未脱敏"）.
    pub description: String,
    /// 数据流阶段（采集/存储/传输/处理/删除）.
    pub data_flow_stage: String,
    /// 严重度.
    pub severity: ThreatSeverity,
    /// 可能性.
    pub likelihood: ThreatLikelihood,
    /// 关联控制 ID.
    pub control_ids: Vec<String>,
    /// 残留风险.
    pub residual_risk: ResidualRisk,
}

impl PrivacyThreat {
    pub fn new(
        threat_id: &str,
        description: &str,
        data_flow_stage: &str,
        severity: ThreatSeverity,
        likelihood: ThreatLikelihood,
        control_ids: Vec<String>,
        residual_risk: ResidualRisk,
    ) -> Self {
        Self {
            threat_id: String::from(threat_id),
            description: String::from(description),
            data_flow_stage: String::from(data_flow_stage),
            severity,
            likelihood,
            control_ids,
            residual_risk,
        }
    }

    /// 是否为高危（High 或 Critical）.
    pub fn is_high_severity(&self) -> bool {
        matches!(
            self.severity,
            ThreatSeverity::High | ThreatSeverity::Critical
        )
    }
}

/// 隐私控制措施.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyControl {
    /// 控制 ID.
    pub control_id: String,
    /// 控制描述（如 "脱敏规则覆盖所有 PII 字段"）.
    pub description: String,
    /// 控制类型（技术/流程/合同）.
    pub control_type: String,
    /// 是否已验证（有测试证据）.
    pub verified: bool,
    /// 该控制缓解的威胁 ID.
    pub mitigates_threat_ids: Vec<String>,
}

impl PrivacyControl {
    pub fn new(
        control_id: &str,
        description: &str,
        control_type: &str,
        verified: bool,
        mitigates_threat_ids: Vec<String>,
    ) -> Self {
        Self {
            control_id: String::from(control_id),
            description: String::from(description),
            control_type: String::from(control_type),
            verified,
            mitigates_threat_ids,
        }
    }
}

/// 追溯缺口.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyTraceGap {
    /// 缺口类型.
    pub gap_type: PrivacyGapType,
    /// 关联 ID.
    pub id: String,
    /// 说明.
    pub detail: String,
}

/// 缺口类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyGapType {
    /// 威胁无控制.
    ThreatWithoutControl,
    /// 控制未验证.
    UnverifiedControl,
    /// 残留风险不可接受.
    UnacceptableRisk,
    /// 高危威胁残留风险非可接受.
    HighRiskNotAcceptable,
    /// 控制引用不存在的威胁.
    DanglingControlRef,
}

/// 隐私威胁评估器.
///
/// 汇总威胁/控制，计算追溯矩阵与评估结论。结论**由证据推导**。
pub struct PrivacyThreatAssessment {
    threats: Vec<PrivacyThreat>,
    controls: Vec<PrivacyControl>,
}

impl PrivacyThreatAssessment {
    pub fn new(threats: Vec<PrivacyThreat>, controls: Vec<PrivacyControl>) -> Self {
        Self { threats, controls }
    }

    /// 计算追溯矩阵缺口（由证据推导）.
    pub fn compute_gaps(&self) -> Vec<PrivacyTraceGap> {
        let mut gaps = Vec::new();
        let valid_control_ids: Vec<&String> = self.controls.iter().map(|c| &c.control_id).collect();

        for threat in &self.threats {
            // 1. 威胁无控制
            let has_control = threat
                .control_ids
                .iter()
                .any(|cid| valid_control_ids.contains(&cid));
            if !has_control {
                gaps.push(PrivacyTraceGap {
                    gap_type: PrivacyGapType::ThreatWithoutControl,
                    id: threat.threat_id.clone(),
                    detail: format!("威胁 {} 无关联控制", threat.threat_id),
                });
            }

            // 2. 残留风险不可接受
            if threat.residual_risk == ResidualRisk::Unacceptable {
                gaps.push(PrivacyTraceGap {
                    gap_type: PrivacyGapType::UnacceptableRisk,
                    id: threat.threat_id.clone(),
                    detail: format!("威胁 {} 残留风险不可接受", threat.threat_id),
                });
            }

            // 3. 高危威胁残留风险非可接受
            if threat.is_high_severity() && threat.residual_risk != ResidualRisk::Acceptable {
                gaps.push(PrivacyTraceGap {
                    gap_type: PrivacyGapType::HighRiskNotAcceptable,
                    id: threat.threat_id.clone(),
                    detail: format!(
                        "高危威胁 {} 残留风险为 {:?}，必须为 Acceptable",
                        threat.threat_id, threat.residual_risk
                    ),
                });
            }
        }

        // 4. 控制未验证
        for control in &self.controls {
            if !control.verified {
                gaps.push(PrivacyTraceGap {
                    gap_type: PrivacyGapType::UnverifiedControl,
                    id: control.control_id.clone(),
                    detail: format!("控制 {} 未验证", control.control_id),
                });
            }
        }

        // 5. 控制引用不存在的威胁
        let valid_threat_ids: Vec<&String> = self.threats.iter().map(|t| &t.threat_id).collect();
        for control in &self.controls {
            for tid in &control.mitigates_threat_ids {
                if !valid_threat_ids.contains(&tid) {
                    gaps.push(PrivacyTraceGap {
                        gap_type: PrivacyGapType::DanglingControlRef,
                        id: control.control_id.clone(),
                        detail: format!("控制 {} 引用不存在的威胁 {}", control.control_id, tid),
                    });
                }
            }
        }

        gaps
    }

    /// 评估是否通过（由证据推导：零缺口 → 通过）.
    ///
    /// 条件：
    /// 1. 非空（至少有威胁）
    /// 2. 零缺口
    pub fn is_approved(&self) -> bool {
        if self.threats.is_empty() {
            return false;
        }
        self.compute_gaps().is_empty()
    }

    /// 威胁总数.
    pub fn threat_count(&self) -> usize {
        self.threats.len()
    }

    /// 控制总数.
    pub fn control_count(&self) -> usize {
        self.controls.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threat(
        id: &str,
        severity: ThreatSeverity,
        controls: &[&str],
        risk: ResidualRisk,
    ) -> PrivacyThreat {
        PrivacyThreat::new(
            id,
            "test threat",
            "collection",
            severity,
            ThreatLikelihood::Possible,
            controls.iter().map(|s| String::from(*s)).collect(),
            risk,
        )
    }

    fn control(id: &str, verified: bool, mitigates: &[&str]) -> PrivacyControl {
        PrivacyControl::new(
            id,
            "test control",
            "technical",
            verified,
            mitigates.iter().map(|s| String::from(*s)).collect(),
        )
    }

    #[test]
    fn t1_approved_when_all_threats_have_controls_and_verified() {
        let threats = vec![
            threat(
                "t1",
                ThreatSeverity::High,
                &["c1"],
                ResidualRisk::Acceptable,
            ),
            threat("t2", ThreatSeverity::Low, &["c2"], ResidualRisk::Acceptable),
        ];
        let controls = vec![control("c1", true, &["t1"]), control("c2", true, &["t2"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(assess.is_approved());
    }

    #[test]
    fn t2_fails_when_threat_without_control() {
        let threats = vec![
            threat("t1", ThreatSeverity::Low, &["c1"], ResidualRisk::Acceptable),
            threat("t2", ThreatSeverity::Low, &[], ResidualRisk::Unacceptable),
        ];
        let controls = vec![control("c1", true, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(!assess.is_approved());
        let gaps = assess.compute_gaps();
        assert!(gaps
            .iter()
            .any(|g| g.gap_type == PrivacyGapType::ThreatWithoutControl));
    }

    #[test]
    fn t3_fails_when_control_unverified() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::Low,
            &["c1"],
            ResidualRisk::Acceptable,
        )];
        let controls = vec![control("c1", false, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(!assess.is_approved());
        let gaps = assess.compute_gaps();
        assert!(gaps
            .iter()
            .any(|g| g.gap_type == PrivacyGapType::UnverifiedControl));
    }

    #[test]
    fn t4_fails_when_unacceptable_risk() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::Low,
            &["c1"],
            ResidualRisk::Unacceptable,
        )];
        let controls = vec![control("c1", true, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(!assess.is_approved());
        let gaps = assess.compute_gaps();
        assert!(gaps
            .iter()
            .any(|g| g.gap_type == PrivacyGapType::UnacceptableRisk));
    }

    #[test]
    fn t5_high_severity_needs_acceptable_risk() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::High,
            &["c1"],
            ResidualRisk::NeedsMitigation,
        )];
        let controls = vec![control("c1", true, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(!assess.is_approved());
        let gaps = assess.compute_gaps();
        assert!(gaps
            .iter()
            .any(|g| g.gap_type == PrivacyGapType::HighRiskNotAcceptable));
    }

    #[test]
    fn t6_critical_severity_needs_acceptable_risk() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::Critical,
            &["c1"],
            ResidualRisk::Acceptable,
        )];
        let controls = vec![control("c1", true, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(assess.is_approved());
    }

    #[test]
    fn t7_medium_severity_allows_needs_mitigation() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::Medium,
            &["c1"],
            ResidualRisk::NeedsMitigation,
        )];
        let controls = vec![control("c1", true, &["t1"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        // Medium 允许 NeedsMitigation（仅 High/Critical 要求 Acceptable）
        assert!(assess.is_approved());
    }

    #[test]
    fn t8_empty_assessment_fails() {
        let assess = PrivacyThreatAssessment::new(Vec::new(), Vec::new());
        assert!(!assess.is_approved());
    }

    #[test]
    fn t9_dangling_control_ref_detected() {
        let threats = vec![threat(
            "t1",
            ThreatSeverity::Low,
            &["c1"],
            ResidualRisk::Acceptable,
        )];
        let controls = vec![control("c1", true, &["t1", "t999"])]; // t999 不存在
        let assess = PrivacyThreatAssessment::new(threats, controls);
        let gaps = assess.compute_gaps();
        assert!(gaps
            .iter()
            .any(|g| g.gap_type == PrivacyGapType::DanglingControlRef));
    }

    #[test]
    fn t10_multiple_threats_one_control() {
        let threats = vec![
            threat("t1", ThreatSeverity::Low, &["c1"], ResidualRisk::Acceptable),
            threat("t2", ThreatSeverity::Low, &["c1"], ResidualRisk::Acceptable),
        ];
        let controls = vec![control("c1", true, &["t1", "t2"])];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(assess.is_approved());
    }

    #[test]
    fn t11_data_flow_stages_covered() {
        let threats = vec![
            PrivacyThreat::new(
                "t1",
                "PII in collection",
                "collection",
                ThreatSeverity::High,
                ThreatLikelihood::Possible,
                vec![String::from("c1")],
                ResidualRisk::Acceptable,
            ),
            PrivacyThreat::new(
                "t2",
                "data at rest unencrypted",
                "storage",
                ThreatSeverity::High,
                ThreatLikelihood::Unlikely,
                vec![String::from("c2")],
                ResidualRisk::Acceptable,
            ),
            PrivacyThreat::new(
                "t3",
                "transmission sniffing",
                "transmission",
                ThreatSeverity::Medium,
                ThreatLikelihood::Rare,
                vec![String::from("c3")],
                ResidualRisk::Acceptable,
            ),
            PrivacyThreat::new(
                "t4",
                "deletion not propagated",
                "deletion",
                ThreatSeverity::High,
                ThreatLikelihood::Possible,
                vec![String::from("c4")],
                ResidualRisk::Acceptable,
            ),
        ];
        let controls = vec![
            control("c1", true, &["t1"]),
            control("c2", true, &["t2"]),
            control("c3", true, &["t3"]),
            control("c4", true, &["t4"]),
        ];
        let assess = PrivacyThreatAssessment::new(threats, controls);
        assert!(assess.is_approved());
        assert_eq!(assess.threat_count(), 4);
        assert_eq!(assess.control_count(), 4);
    }
}
