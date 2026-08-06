//! 签名来源与分支策略 (REL-008~009).
//!
//! REL-008: signed provenance — materials/builder/artifact
//! REL-009: branch/CODEOWNERS/tag policy — 双人/签名/不可变

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// 来源材料类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceMaterial {
    /// 源代码 commit。
    SourceCommit,
    /// 构建脚本。
    BuildScript,
    /// 工具链。
    Toolchain,
    /// 依赖图。
    DependencyGraph,
    /// 构建环境。
    BuildEnv,
}

impl ProvenanceMaterial {
    pub fn label(self) -> &'static str {
        match self {
            ProvenanceMaterial::SourceCommit => "SOURCE_COMMIT",
            ProvenanceMaterial::BuildScript => "BUILD_SCRIPT",
            ProvenanceMaterial::Toolchain => "TOOLCHAIN",
            ProvenanceMaterial::DependencyGraph => "DEPENDENCY_GRAPH",
            ProvenanceMaterial::BuildEnv => "BUILD_ENV",
        }
    }
}

/// 来源记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceRecord {
    /// 产物名称。
    pub artifact_name: String,
    /// 产物版本。
    pub artifact_version: String,
    /// 产物 SHA-256。
    pub artifact_sha256: String,
    /// 构建者 ID。
    pub builder_id: String,
    /// 构建时间（Unix 秒）。
    pub built_at: u64,
    /// 来源材料（类型 → SHA-256）。
    pub materials: Vec<(ProvenanceMaterial, String)>,
    /// 来源签名（SM2）。
    pub signature: String,
}

impl ProvenanceRecord {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.artifact_name.is_empty() {
            return Err("artifact_name is empty");
        }
        if self.artifact_sha256.len() != 64 {
            return Err("artifact_sha256 must be 64 hex chars");
        }
        if self.builder_id.is_empty() {
            return Err("builder_id is empty");
        }
        if self.materials.is_empty() {
            return Err("no materials recorded");
        }
        if self.signature.is_empty() {
            return Err("signature is empty");
        }
        for (_, hash) in &self.materials {
            if hash.len() != 64 {
                return Err("material hash must be 64 hex chars");
            }
        }
        Ok(())
    }

    /// 是否包含所有必要材料类型。
    pub fn has_all_materials(&self) -> bool {
        let required = [
            ProvenanceMaterial::SourceCommit,
            ProvenanceMaterial::BuildScript,
            ProvenanceMaterial::Toolchain,
            ProvenanceMaterial::DependencyGraph,
        ];
        required.iter().all(|r| {
            self.materials.iter().any(|(m, _)| m == r)
        })
    }
}

/// 分支策略规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPolicy {
    /// 分支名模式（如 "release/*"）。
    pub branch_pattern: String,
    /// 是否需要双人审批。
    pub require_dual_review: bool,
    /// 是否需要签名提交。
    pub require_signed_commits: bool,
    /// 是否禁止 force push。
    pub no_force_push: bool,
    /// 是否禁止删除分支。
    pub no_deletion: bool,
    /// CODEOWNERS 规则。
    pub codeowners: Vec<CodeownersRule>,
}

/// CODEOWNERS 规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeownersRule {
    /// 路径模式（如 "/crates/security/*"）。
    pub path: String,
    /// 所有者列表。
    pub owners: Vec<String>,
    /// 是否需要所有所有者审批。
    pub require_all_owners: bool,
}

impl CodeownersRule {
    pub fn new(path: &str, owners: Vec<String>, require_all: bool) -> Self {
        Self {
            path: String::from(path),
            owners,
            require_all_owners: require_all,
        }
    }

    /// 检查路径是否匹配。
    pub fn matches(&self, changed_path: &str) -> bool {
        if self.path.ends_with('*') {
            let prefix = &self.path[..self.path.len() - 1];
            changed_path.starts_with(prefix)
        } else {
            changed_path == self.path
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.path.is_empty() {
            return Err("path is empty");
        }
        if self.owners.is_empty() {
            return Err("no owners");
        }
        Ok(())
    }
}

impl BranchPolicy {
    /// 生产策略。
    pub fn production() -> Self {
        Self {
            branch_pattern: String::from("release/*"),
            require_dual_review: true,
            require_signed_commits: true,
            no_force_push: true,
            no_deletion: true,
            codeowners: vec![
                CodeownersRule::new(
                    "/crates/security/*",
                    vec![String::from("security-lead"), String::from("crypto-lead")],
                    true,
                ),
                CodeownersRule::new(
                    "/crates/kernel/*",
                    vec![String::from("kernel-lead")],
                    false,
                ),
            ],
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.branch_pattern.is_empty() {
            return Err("branch_pattern is empty");
        }
        for co in &self.codeowners {
            co.validate()?;
        }
        Ok(())
    }

    /// 查找路径的 CODEOWNERS。
    pub fn find_owners(&self, path: &str) -> Option<&CodeownersRule> {
        self.codeowners.iter().find(|co| co.matches(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provenance() -> ProvenanceRecord {
        ProvenanceRecord {
            artifact_name: String::from("eneros-kernel"),
            artifact_version: String::from("0.200.0"),
            artifact_sha256: String::from("a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890"),
            builder_id: String::from("ci-builder-01"),
            built_at: 1000,
            materials: vec![
                (ProvenanceMaterial::SourceCommit, String::from("b1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890")),
                (ProvenanceMaterial::BuildScript, String::from("c1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890")),
                (ProvenanceMaterial::Toolchain, String::from("d1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890")),
                (ProvenanceMaterial::DependencyGraph, String::from("e1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890a1b2c3d4e5f67890")),
            ],
            signature: String::from("sm2-sig-abc"),
        }
    }

    #[test]
    fn provenance_validates() {
        assert!(sample_provenance().validate().is_ok());
    }

    #[test]
    fn provenance_has_all_materials() {
        let p = sample_provenance();
        assert!(p.has_all_materials());
    }

    #[test]
    fn missing_material_fails() {
        let mut p = sample_provenance();
        p.materials.retain(|(m, _)| *m != ProvenanceMaterial::Toolchain);
        assert!(!p.has_all_materials());
    }

    #[test]
    fn unsigned_provenance_rejected() {
        let mut p = sample_provenance();
        p.signature = String::new();
        assert!(p.validate().is_err());
    }

    #[test]
    fn branch_policy_production() {
        let policy = BranchPolicy::production();
        assert!(policy.validate().is_ok());
        assert!(policy.require_dual_review);
        assert!(policy.require_signed_commits);
        assert!(policy.no_force_push);
    }

    #[test]
    fn codeowners_matches() {
        let policy = BranchPolicy::production();
        let owners = policy.find_owners("/crates/security/crypto/src/lib.rs");
        assert!(owners.is_some());
        assert_eq!(owners.unwrap().owners.len(), 2);

        let owners = policy.find_owners("/crates/kernel/kernel/src/main.rs");
        assert!(owners.is_some());
        assert_eq!(owners.unwrap().owners.len(), 1);
    }

    #[test]
    fn codeowners_no_match() {
        let policy = BranchPolicy::production();
        assert!(policy.find_owners("/docs/readme.md").is_none());
    }
}
