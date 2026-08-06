//! OS/Config/Agent/Model 发布与兼容矩阵 (OPS-006).
//!
//! MASTER §21.14 OPS-006: "OS/config/Agent/model rollout — 兼容矩阵强制"
//!
//! 原则：任何发布（OS/配置/Agent/模型）必须通过兼容矩阵校验。
//! 兼容矩阵是**冻结规则**：不允许跳过、不允许"先发后补"。
//! 违反兼容矩阵的发布会破坏运行态一致性，必须 fail-closed 拒绝。

use alloc::string::String;
use alloc::vec::Vec;

/// 发布类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RolloutType {
    /// OS/固件。
    Os,
    /// 配置。
    Config,
    /// Agent 二进制。
    Agent,
    /// AI 模型。
    Model,
}

impl RolloutType {
    pub fn label(self) -> &'static str {
        match self {
            RolloutType::Os => "OS",
            RolloutType::Config => "CONFIG",
            RolloutType::Agent => "AGENT",
            RolloutType::Model => "MODEL",
        }
    }
}

/// 发布目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutTarget {
    /// 发布类型。
    pub rollout_type: RolloutType,
    /// 组件名（如 "eneros-kernel"、"agent-coordinator"、"llm-7b-int4"）。
    pub component: String,
    /// 当前版本。
    pub current_version: String,
    /// 目标版本。
    pub target_version: String,
}

impl RolloutTarget {
    /// 校验目标合法性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.component.is_empty() {
            return Err("component is empty");
        }
        if self.current_version.is_empty() {
            return Err("current_version is empty");
        }
        if self.target_version.is_empty() {
            return Err("target_version is empty");
        }
        if self.current_version == self.target_version {
            return Err("target_version equals current_version (no-op)");
        }
        Ok(())
    }
}

/// 兼容性规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatRule {
    /// 发布类型。
    pub rollout_type: RolloutType,
    /// 组件名（"" = 匹配所有组件）。
    pub component: String,
    /// 起始版本（含）。
    pub from_version: String,
    /// 目标版本（含）。
    pub to_version: String,
    /// 所需前置依赖（组件名 → 最低版本）。
    pub requires: Vec<(String, String)>,
    /// 是否允许降级。
    pub allow_downgrade: bool,
}

/// 兼容性错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatError {
    /// 无匹配规则（未声明的升级路径）。
    NoMatchingRule,
    /// 版本降级且未允许。
    DowngradeNotAllowed,
    /// 前置依赖不满足。
    MissingDependency {
        component: String,
        required: String,
        actual: String,
    },
    /// 目标本身非法。
    InvalidTarget(&'static str),
}

impl CompatError {
    pub fn label(&self) -> &'static str {
        match self {
            CompatError::NoMatchingRule => "NO_MATCHING_RULE",
            CompatError::DowngradeNotAllowed => "DOWNGRADE_NOT_ALLOWED",
            CompatError::MissingDependency { .. } => "MISSING_DEPENDENCY",
            CompatError::InvalidTarget(_) => "INVALID_TARGET",
        }
    }
}

/// 兼容矩阵。
///
/// 冻结规则集合。任何发布必须匹配至少一条规则并通过其依赖检查。
pub struct CompatibilityMatrix {
    rules: Vec<CompatRule>,
}

impl Default for CompatibilityMatrix {
    fn default() -> Self {
        Self::new()
    }
}

impl CompatibilityMatrix {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// 添加兼容规则。
    pub fn add_rule(&mut self, rule: CompatRule) {
        self.rules.push(rule);
    }

    /// 校验发布目标是否兼容。
    ///
    /// `installed` 为当前已安装组件清单（组件名 → 版本）。
    pub fn check(
        &self,
        target: &RolloutTarget,
        installed: &[(String, String)],
    ) -> Result<&CompatRule, CompatError> {
        target.validate().map_err(CompatError::InvalidTarget)?;

        // 查找匹配规则
        let matching: Vec<&CompatRule> = self
            .rules
            .iter()
            .filter(|r| {
                r.rollout_type == target.rollout_type
                    && (r.component.is_empty() || r.component == target.component)
                    && r.from_version == target.current_version
                    && r.to_version == target.target_version
            })
            .collect();

        if matching.is_empty() {
            return Err(CompatError::NoMatchingRule);
        }

        // 取第一条匹配规则检查依赖
        let rule = matching[0];

        // 降级检查：current > target 表示降级（target_version < current_version）。
        // 仅在版本可数值比较时执行；非数值版本（如 "v3"/"v4"）由矩阵规则本身保证方向。
        let cur_v = parse_version(&target.current_version);
        let tgt_v = parse_version(&target.target_version);
        if !rule.allow_downgrade && cur_v != tgt_v && cur_v > tgt_v {
            return Err(CompatError::DowngradeNotAllowed);
        }

        // 依赖检查：actual >= min 才满足
        for (dep_component, dep_min_version) in &rule.requires {
            let actual = installed
                .iter()
                .find(|(name, _)| name == dep_component)
                .map(|(_, ver)| ver.as_str())
                .unwrap_or("");

            if actual.is_empty() {
                return Err(CompatError::MissingDependency {
                    component: dep_component.clone(),
                    required: dep_min_version.clone(),
                    actual: String::from("not installed"),
                });
            }

            if !version_gte(actual, dep_min_version) {
                return Err(CompatError::MissingDependency {
                    component: dep_component.clone(),
                    required: dep_min_version.clone(),
                    actual: String::from(actual),
                });
            }
        }

        Ok(rule)
    }

    /// 返回规则数。
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// 发布管理器。
pub struct RolloutManager {
    matrix: CompatibilityMatrix,
    /// 已发起的发布记录。
    rollouts: Vec<(RolloutTarget, RolloutStatus)>,
}

/// 发布状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RolloutStatus {
    /// 兼容性校验通过，已批准。
    Approved,
    /// 兼容性校验失败，已拒绝。
    Rejected,
    /// 正在发布中。
    InProgress,
    /// 发布完成。
    Completed,
    /// 发布失败，已回滚。
    RolledBack,
}

impl RolloutManager {
    pub fn new(matrix: CompatibilityMatrix) -> Self {
        Self {
            matrix,
            rollouts: Vec::new(),
        }
    }

    /// 批准发布（执行兼容性校验）。
    pub fn approve(
        &mut self,
        target: RolloutTarget,
        installed: &[(String, String)],
    ) -> Result<RolloutStatus, CompatError> {
        self.matrix.check(&target, installed)?;
        let status = RolloutStatus::Approved;
        self.rollouts.push((target, status));
        Ok(status)
    }

    /// 拒绝发布（记录原因）。
    pub fn reject(&mut self, target: RolloutTarget, _err: &CompatError) -> RolloutStatus {
        let status = RolloutStatus::Rejected;
        self.rollouts.push((target, status));
        status
    }

    /// 标记发布进行中。
    pub fn start(&mut self, component: &str, target_version: &str) -> Result<(), &'static str> {
        let entry = self
            .rollouts
            .iter_mut()
            .find(|(t, s)| {
                t.component == component
                    && t.target_version == target_version
                    && *s == RolloutStatus::Approved
            })
            .ok_or("no approved rollout found")?;
        entry.1 = RolloutStatus::InProgress;
        Ok(())
    }

    /// 标记发布完成。
    pub fn complete(&mut self, component: &str, target_version: &str) -> Result<(), &'static str> {
        let entry = self
            .rollouts
            .iter_mut()
            .find(|(t, s)| {
                t.component == component
                    && t.target_version == target_version
                    && *s == RolloutStatus::InProgress
            })
            .ok_or("no in-progress rollout found")?;
        entry.1 = RolloutStatus::Completed;
        Ok(())
    }

    /// 标记发布回滚。
    pub fn rollback(&mut self, component: &str, target_version: &str) -> Result<(), &'static str> {
        let entry = self
            .rollouts
            .iter_mut()
            .find(|(t, s)| {
                t.component == component
                    && t.target_version == target_version
                    && *s == RolloutStatus::InProgress
            })
            .ok_or("no in-progress rollout found")?;
        entry.1 = RolloutStatus::RolledBack;
        Ok(())
    }

    /// 返回所有发布记录。
    pub fn rollouts(&self) -> &[(RolloutTarget, RolloutStatus)] {
        &self.rollouts
    }

    /// 返回兼容矩阵引用。
    pub fn matrix(&self) -> &CompatibilityMatrix {
        &self.matrix
    }
}

/// 语义版本比较：a >= b 返回 true。
///
/// 仅支持 `major.minor.patch` 格式。非数字段按 0 处理。
fn version_gte(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    pa >= pb
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let parts: Vec<&str> = v.split('.').collect();
    let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn production_matrix() -> CompatibilityMatrix {
        let mut m = CompatibilityMatrix::new();
        // OS 0.199.0 → 0.200.0，需要 agent-runtime >= 0.50.0
        m.add_rule(CompatRule {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            from_version: String::from("0.199.0"),
            to_version: String::from("0.200.0"),
            requires: vec![(String::from("agent-runtime"), String::from("0.50.0"))],
            allow_downgrade: false,
        });
        // Agent 1.2.0 → 1.3.0，无依赖
        m.add_rule(CompatRule {
            rollout_type: RolloutType::Agent,
            component: String::from("coordinator"),
            from_version: String::from("1.2.0"),
            to_version: String::from("1.3.0"),
            requires: Vec::new(),
            allow_downgrade: false,
        });
        // Model v3 → v4，需要 OS >= 0.200.0
        m.add_rule(CompatRule {
            rollout_type: RolloutType::Model,
            component: String::from("llm-7b"),
            from_version: String::from("v3"),
            to_version: String::from("v4"),
            requires: vec![(String::from("eneros-kernel"), String::from("0.200.0"))],
            allow_downgrade: false,
        });
        // Config 任意版本降级允许（配置回滚）
        m.add_rule(CompatRule {
            rollout_type: RolloutType::Config,
            component: String::from("site-config"),
            from_version: String::from("2.0"),
            to_version: String::from("1.0"),
            requires: Vec::new(),
            allow_downgrade: true,
        });
        m
    }

    #[test]
    fn os_upgrade_with_dependency_satisfied() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.199.0"),
            target_version: String::from("0.200.0"),
        };
        let installed = vec![
            (String::from("eneros-kernel"), String::from("0.199.0")),
            (String::from("agent-runtime"), String::from("0.51.0")), // >= 0.50.0
        ];
        assert!(matrix.check(&target, &installed).is_ok());
    }

    #[test]
    fn os_upgrade_with_missing_dependency_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.199.0"),
            target_version: String::from("0.200.0"),
        };
        let installed = vec![
            (String::from("eneros-kernel"), String::from("0.199.0")),
            (String::from("agent-runtime"), String::from("0.49.0")), // < 0.50.0
        ];
        let result = matrix.check(&target, &installed);
        assert!(matches!(result, Err(CompatError::MissingDependency { .. })));
    }

    #[test]
    fn os_upgrade_with_dependency_not_installed_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.199.0"),
            target_version: String::from("0.200.0"),
        };
        let installed = vec![(String::from("eneros-kernel"), String::from("0.199.0"))];
        let result = matrix.check(&target, &installed);
        assert!(matches!(result, Err(CompatError::MissingDependency { .. })));
    }

    #[test]
    fn agent_upgrade_no_dependency() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Agent,
            component: String::from("coordinator"),
            current_version: String::from("1.2.0"),
            target_version: String::from("1.3.0"),
        };
        let installed = vec![(String::from("coordinator"), String::from("1.2.0"))];
        assert!(matrix.check(&target, &installed).is_ok());
    }

    #[test]
    fn undeclared_upgrade_path_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.195.0"), // not in rules
            target_version: String::from("0.200.0"),
        };
        let installed = vec![(String::from("eneros-kernel"), String::from("0.195.0"))];
        let result = matrix.check(&target, &installed);
        assert_eq!(result.unwrap_err(), CompatError::NoMatchingRule);
    }

    #[test]
    fn downgrade_without_allow_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Agent,
            component: String::from("coordinator"),
            current_version: String::from("1.3.0"),
            target_version: String::from("1.2.0"), // downgrade
        };
        let installed = vec![(String::from("coordinator"), String::from("1.3.0"))];
        // No matching rule (rule is from 1.2.0 to 1.3.0, not reverse)
        let result = matrix.check(&target, &installed);
        assert_eq!(result.unwrap_err(), CompatError::NoMatchingRule);
    }

    #[test]
    fn config_downgrade_allowed() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Config,
            component: String::from("site-config"),
            current_version: String::from("2.0"),
            target_version: String::from("1.0"),
        };
        let installed = vec![(String::from("site-config"), String::from("2.0"))];
        assert!(matrix.check(&target, &installed).is_ok());
    }

    #[test]
    fn model_upgrade_requires_os_version() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Model,
            component: String::from("llm-7b"),
            current_version: String::from("v3"),
            target_version: String::from("v4"),
        };

        // OS too old
        let installed_old = vec![
            (String::from("llm-7b"), String::from("v3")),
            (String::from("eneros-kernel"), String::from("0.199.0")), // < 0.200.0
        ];
        assert!(matrix.check(&target, &installed_old).is_err());

        // OS ok
        let installed_ok = vec![
            (String::from("llm-7b"), String::from("v3")),
            (String::from("eneros-kernel"), String::from("0.200.0")),
        ];
        assert!(matrix.check(&target, &installed_ok).is_ok());
    }

    #[test]
    fn invalid_target_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::new(), // empty
            current_version: String::from("0.199.0"),
            target_version: String::from("0.200.0"),
        };
        let installed = Vec::new();
        let result = matrix.check(&target, &installed);
        assert!(matches!(result, Err(CompatError::InvalidTarget(_))));
    }

    #[test]
    fn noop_target_rejected() {
        let matrix = production_matrix();
        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.200.0"),
            target_version: String::from("0.200.0"), // same
        };
        let installed = Vec::new();
        let result = matrix.check(&target, &installed);
        assert!(matches!(result, Err(CompatError::InvalidTarget(_))));
    }

    #[test]
    fn rollout_manager_lifecycle() {
        let matrix = production_matrix();
        let mut mgr = RolloutManager::new(matrix);

        let target = RolloutTarget {
            rollout_type: RolloutType::Agent,
            component: String::from("coordinator"),
            current_version: String::from("1.2.0"),
            target_version: String::from("1.3.0"),
        };
        let installed = vec![(String::from("coordinator"), String::from("1.2.0"))];

        // Approve
        let status = mgr.approve(target.clone(), &installed).unwrap();
        assert_eq!(status, RolloutStatus::Approved);

        // Start
        mgr.start("coordinator", "1.3.0").unwrap();
        assert_eq!(
            mgr.rollouts()
                .iter()
                .find(|(t, _)| t.component == "coordinator")
                .unwrap()
                .1,
            RolloutStatus::InProgress
        );

        // Complete
        mgr.complete("coordinator", "1.3.0").unwrap();
        assert_eq!(
            mgr.rollouts()
                .iter()
                .find(|(t, _)| t.component == "coordinator")
                .unwrap()
                .1,
            RolloutStatus::Completed
        );
    }

    #[test]
    fn rollout_manager_reject_records() {
        let matrix = production_matrix();
        let mut mgr = RolloutManager::new(matrix);

        let target = RolloutTarget {
            rollout_type: RolloutType::Os,
            component: String::from("eneros-kernel"),
            current_version: String::from("0.195.0"), // no matching rule
            target_version: String::from("0.200.0"),
        };
        let installed = vec![(String::from("eneros-kernel"), String::from("0.195.0"))];

        let result = mgr.approve(target.clone(), &installed);
        assert!(result.is_err());

        // Manually reject
        let status = mgr.reject(target, &result.unwrap_err());
        assert_eq!(status, RolloutStatus::Rejected);
    }

    #[test]
    fn rollback_marks_status() {
        let matrix = production_matrix();
        let mut mgr = RolloutManager::new(matrix);

        let target = RolloutTarget {
            rollout_type: RolloutType::Agent,
            component: String::from("coordinator"),
            current_version: String::from("1.2.0"),
            target_version: String::from("1.3.0"),
        };
        let installed = vec![(String::from("coordinator"), String::from("1.2.0"))];

        mgr.approve(target.clone(), &installed).unwrap();
        mgr.start("coordinator", "1.3.0").unwrap();
        mgr.rollback("coordinator", "1.3.0").unwrap();
        assert_eq!(
            mgr.rollouts()
                .iter()
                .find(|(t, _)| t.component == "coordinator")
                .unwrap()
                .1,
            RolloutStatus::RolledBack
        );
    }

    #[test]
    fn wildcard_component_matches() {
        let mut m = CompatibilityMatrix::new();
        m.add_rule(CompatRule {
            rollout_type: RolloutType::Config,
            component: String::new(), // wildcard
            from_version: String::from("1.0"),
            to_version: String::from("2.0"),
            requires: Vec::new(),
            allow_downgrade: false,
        });

        let target = RolloutTarget {
            rollout_type: RolloutType::Config,
            component: String::from("any-component"),
            current_version: String::from("1.0"),
            target_version: String::from("2.0"),
        };
        let installed = vec![(String::from("any-component"), String::from("1.0"))];
        assert!(m.check(&target, &installed).is_ok());
    }
}
