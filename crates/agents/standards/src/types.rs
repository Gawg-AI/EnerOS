//! 标准类型定义 — 标准组织、提案结构、一致性条款等。

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// 标准组织 / 提交目标
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardBody {
    /// IEEE
    Ieee,
    /// IEC
    Iec,
    /// ITU
    Itu,
    /// 国家标准委员会
    NationalCommittee,
}

/// API 稳定性等级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiStability {
    /// 稳定
    Stable,
    /// 测试
    Beta,
    /// 实验性
    Experimental,
}

/// 标准章节
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardSection {
    /// 章节 ID
    pub id: String,
    /// 标题
    pub title: String,
    /// 是否为规范性条款
    pub normative: bool,
    /// 内容
    pub content: String,
}

/// 标准提案
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardProposal {
    /// 提案 ID
    pub proposal_id: u64,
    /// 标题
    pub title: String,
    /// 范围
    pub scope: String,
    /// 版本
    pub version: String,
    /// 章节列表
    pub sections: Vec<StandardSection>,
    /// 提交目标组织
    pub submitted_to: Option<StandardBody>,
}

/// 提交状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionStatus {
    /// 已接收
    Received,
    /// 审核中
    UnderReview,
    /// 已接受
    Accepted,
    /// 已拒绝
    Rejected,
}

/// 提交回执
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionReceipt {
    /// 回执 ID
    pub receipt_id: u64,
    /// 目标组织
    pub body: StandardBody,
    /// 提交时间戳
    pub submitted_at: u64,
    /// 状态
    pub status: SubmissionStatus,
}

/// API 表面
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSurface {
    /// 名称
    pub name: String,
    /// 稳定性
    pub stability: ApiStability,
}

/// 一致性条款
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceClause {
    /// 条款 ID
    pub id: String,
    /// 是否为必须条款
    pub must: bool,
    /// 测试标识
    pub test: String,
}

/// 一致性报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceReport {
    /// 总条款数
    pub total: u32,
    /// 符合数
    pub conformed: u32,
    /// 不符合条款 ID 列表
    pub non_conformed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_body_equality() {
        assert_eq!(StandardBody::Ieee, StandardBody::Ieee);
        assert_ne!(StandardBody::Iec, StandardBody::Itu);
    }

    #[test]
    fn test_api_stability_variants() {
        let s = ApiStability::Stable;
        let b = ApiStability::Beta;
        let e = ApiStability::Experimental;
        assert_ne!(s, b);
        assert_ne!(b, e);
    }

    #[test]
    fn test_standard_section_creation() {
        let sec = StandardSection {
            id: String::from("sec-1"),
            title: String::from("Overview"),
            normative: true,
            content: String::from("This is the overview."),
        };
        assert_eq!(sec.id, "sec-1");
        assert!(sec.normative);
    }

    #[test]
    fn test_standard_proposal_creation() {
        let proposal = StandardProposal {
            proposal_id: 1,
            title: String::from("PNO Standard"),
            scope: String::from("Power Native OS"),
            version: String::from("0.1.0"),
            sections: Vec::new(),
            submitted_to: None,
        };
        assert!(proposal.sections.is_empty());
        assert!(proposal.submitted_to.is_none());
    }

    #[test]
    fn test_submission_receipt() {
        let receipt = SubmissionReceipt {
            receipt_id: 100,
            body: StandardBody::Iec,
            submitted_at: 1_700_000_000,
            status: SubmissionStatus::Received,
        };
        assert_eq!(receipt.status, SubmissionStatus::Received);
        assert_eq!(receipt.body, StandardBody::Iec);
    }

    #[test]
    fn test_conformance_report() {
        let report = ConformanceReport {
            total: 5,
            conformed: 4,
            non_conformed: alloc::vec![String::from("clause-3")],
        };
        assert_eq!(report.total, 5);
        assert_eq!(report.conformed, 4);
        assert_eq!(report.non_conformed.len(), 1);
    }
}
