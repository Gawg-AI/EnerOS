//! 标准提案管理 — 起草、添加章节、提交到标准组织。

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::types::{
    StandardBody, StandardProposal, StandardSection, SubmissionReceipt, SubmissionStatus,
};
use crate::StdError;

/// 提案管理器 trait
pub trait ProposalManager {
    /// 起草新提案
    fn draft(&mut self, title: &str, scope: &str) -> Result<StandardProposal, StdError>;
    /// 向提案添加章节
    fn add_section(&mut self, proposal_id: u64, section: StandardSection) -> Result<(), StdError>;
    /// 提交提案到标准组织
    fn submit(
        &mut self,
        proposal_id: u64,
        body: StandardBody,
    ) -> Result<SubmissionReceipt, StdError>;
}

/// 提案管理器实现
pub struct ProposalManagerImpl {
    /// 所有提案
    pub proposals: Vec<StandardProposal>,
    /// 下一个提案 ID
    pub next_id: u64,
    /// 所有回执
    pub receipts: Vec<SubmissionReceipt>,
}

impl ProposalManagerImpl {
    /// 创建空的提案管理器
    pub fn new() -> Self {
        Self {
            proposals: Vec::new(),
            next_id: 1,
            receipts: Vec::new(),
        }
    }
}

impl Default for ProposalManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl ProposalManager for ProposalManagerImpl {
    fn draft(&mut self, title: &str, scope: &str) -> Result<StandardProposal, StdError> {
        let proposal = StandardProposal {
            proposal_id: self.next_id,
            title: String::from(title),
            scope: String::from(scope),
            version: String::from("0.1.0"),
            sections: Vec::new(),
            submitted_to: None,
        };
        self.next_id += 1;
        self.proposals.push(proposal.clone());
        Ok(proposal)
    }

    fn add_section(&mut self, proposal_id: u64, section: StandardSection) -> Result<(), StdError> {
        let proposal = self
            .proposals
            .iter_mut()
            .find(|p| p.proposal_id == proposal_id)
            .ok_or(StdError::ProposalNotFound)?;
        if proposal.submitted_to.is_some() {
            return Err(StdError::AlreadySubmitted);
        }
        if section.id.is_empty() {
            return Err(StdError::InvalidSection);
        }
        proposal.sections.push(section);
        Ok(())
    }

    fn submit(
        &mut self,
        proposal_id: u64,
        body: StandardBody,
    ) -> Result<SubmissionReceipt, StdError> {
        let proposal = self
            .proposals
            .iter_mut()
            .find(|p| p.proposal_id == proposal_id)
            .ok_or(StdError::ProposalNotFound)?;
        if proposal.submitted_to.is_some() {
            return Err(StdError::AlreadySubmitted);
        }
        if proposal.sections.is_empty() {
            return Err(StdError::NoSections);
        }
        proposal.submitted_to = Some(body);

        let receipt = SubmissionReceipt {
            receipt_id: self.receipts.len() as u64 + 1,
            body,
            submitted_at: 0,
            status: SubmissionStatus::Received,
        };
        self.receipts.push(receipt.clone());
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draft_proposal() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO Standard", "Power Native OS").unwrap();
        assert_eq!(p.proposal_id, 1);
        assert_eq!(p.title, "PNO Standard");
        assert!(p.sections.is_empty());
        assert!(p.submitted_to.is_none());
    }

    #[test]
    fn test_add_section() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO", "scope").unwrap();
        let sec = StandardSection {
            id: String::from("s1"),
            title: String::from("Intro"),
            normative: true,
            content: String::from("content"),
        };
        mgr.add_section(p.proposal_id, sec).unwrap();
        assert_eq!(mgr.proposals[0].sections.len(), 1);
    }

    #[test]
    fn test_submit_success() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO", "scope").unwrap();
        let sec = StandardSection {
            id: String::from("s1"),
            title: String::from("Intro"),
            normative: true,
            content: String::from("content"),
        };
        mgr.add_section(p.proposal_id, sec).unwrap();
        let receipt = mgr.submit(p.proposal_id, StandardBody::Iec).unwrap();
        assert_eq!(receipt.body, StandardBody::Iec);
        assert_eq!(receipt.status, SubmissionStatus::Received);
        assert_eq!(mgr.proposals[0].submitted_to, Some(StandardBody::Iec));
    }

    #[test]
    fn test_submit_without_sections_fails() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO", "scope").unwrap();
        let result = mgr.submit(p.proposal_id, StandardBody::Ieee);
        assert_eq!(result.unwrap_err(), StdError::NoSections);
    }

    #[test]
    fn test_submit_already_submitted_fails() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO", "scope").unwrap();
        let sec = StandardSection {
            id: String::from("s1"),
            title: String::from("Intro"),
            normative: false,
            content: String::from("c"),
        };
        mgr.add_section(p.proposal_id, sec).unwrap();
        mgr.submit(p.proposal_id, StandardBody::Itu).unwrap();
        let result = mgr.submit(p.proposal_id, StandardBody::Iec);
        assert_eq!(result.unwrap_err(), StdError::AlreadySubmitted);
    }

    #[test]
    fn test_add_section_invalid() {
        let mut mgr = ProposalManagerImpl::new();
        let p = mgr.draft("PNO", "scope").unwrap();
        let sec = StandardSection {
            id: String::new(),
            title: String::from("Bad"),
            normative: false,
            content: String::new(),
        };
        let result = mgr.add_section(p.proposal_id, sec);
        assert_eq!(result.unwrap_err(), StdError::InvalidSection);
    }
}
