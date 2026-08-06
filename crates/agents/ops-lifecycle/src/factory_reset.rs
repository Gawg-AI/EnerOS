//! 工厂重置 (OPS-012).
//!
//! MASTER §21.14 OPS-012: "factory reset — 保留/删除规则准确"
//!
//! 原则：工厂重置必须精确区分"保留"和"删除"的数据。
//! 保留：设备身份、校准数据、审计日志、许可证。
//! 删除：用户配置、业务数据、AI 模型、会话密钥、日志。
//!
//! 重置后必须验证：保留的数据完整，删除的数据不可恢复。

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// 数据类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataCategory {
    /// 设备身份证书。
    DeviceIdentity,
    /// 校准数据。
    Calibration,
    /// 审计日志。
    AuditLog,
    /// 许可证。
    License,
    /// 用户配置。
    UserConfig,
    /// 业务数据。
    BusinessData,
    /// AI 模型。
    AiModel,
    /// 会话密钥。
    SessionKeys,
    /// 运行日志。
    RuntimeLogs,
    /// 网络配置。
    NetworkConfig,
}

impl DataCategory {
    pub fn label(self) -> &'static str {
        match self {
            DataCategory::DeviceIdentity => "DEVICE_IDENTITY",
            DataCategory::Calibration => "CALIBRATION",
            DataCategory::AuditLog => "AUDIT_LOG",
            DataCategory::License => "LICENSE",
            DataCategory::UserConfig => "USER_CONFIG",
            DataCategory::BusinessData => "BUSINESS_DATA",
            DataCategory::AiModel => "AI_MODEL",
            DataCategory::SessionKeys => "SESSION_KEYS",
            DataCategory::RuntimeLogs => "RUNTIME_LOGS",
            DataCategory::NetworkConfig => "NETWORK_CONFIG",
        }
    }
}

/// 保留规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreserveRule {
    /// 保留的数据类别。
    pub category: DataCategory,
    /// 保留原因。
    pub reason: String,
}

impl PreserveRule {
    pub fn new(category: DataCategory, reason: &str) -> Self {
        Self {
            category,
            reason: String::from(reason),
        }
    }
}

/// 删除规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRule {
    /// 删除的数据类别。
    pub category: DataCategory,
    /// 删除方式。
    pub method: DeleteMethod,
}

impl DeleteRule {
    pub fn new(category: DataCategory, method: DeleteMethod) -> Self {
        Self { category, method }
    }
}

/// 删除方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMethod {
    /// 快速删除（仅删除索引，数据可能残留 — 仅用于非敏感数据）。
    QuickDelete,
    /// 加密擦除（销毁密钥，数据变为不可解密）。
    CryptoErase,
    /// 深度覆写（多次覆写存储介质）。
    DeepOverwrite,
}

impl DeleteMethod {
    pub fn label(self) -> &'static str {
        match self {
            DeleteMethod::QuickDelete => "QUICK_DELETE",
            DeleteMethod::CryptoErase => "CRYPTO_ERASE",
            DeleteMethod::DeepOverwrite => "DEEP_OVERWRITE",
        }
    }
}

/// 重置策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetPolicy {
    /// 保留规则。
    pub preserve: Vec<PreserveRule>,
    /// 删除规则。
    pub delete: Vec<DeleteRule>,
}

impl Default for ResetPolicy {
    fn default() -> Self {
        Self::production()
    }
}

impl ResetPolicy {
    /// 生产策略。
    ///
    /// 保留：设备身份、校准、审计日志、许可证。
    /// 删除：用户配置、业务数据、AI模型、会话密钥、运行日志、网络配置。
    pub fn production() -> Self {
        Self {
            preserve: vec![
                PreserveRule::new(
                    DataCategory::DeviceIdentity,
                    "device identity must survive reset",
                ),
                PreserveRule::new(
                    DataCategory::Calibration,
                    "factory calibration is irreversible",
                ),
                PreserveRule::new(
                    DataCategory::AuditLog,
                    "audit log must be preserved for compliance",
                ),
                PreserveRule::new(DataCategory::License, "license entitlement persists"),
            ],
            delete: vec![
                DeleteRule::new(DataCategory::UserConfig, DeleteMethod::CryptoErase),
                DeleteRule::new(DataCategory::BusinessData, DeleteMethod::DeepOverwrite),
                DeleteRule::new(DataCategory::AiModel, DeleteMethod::QuickDelete),
                DeleteRule::new(DataCategory::SessionKeys, DeleteMethod::CryptoErase),
                DeleteRule::new(DataCategory::RuntimeLogs, DeleteMethod::QuickDelete),
                DeleteRule::new(DataCategory::NetworkConfig, DeleteMethod::CryptoErase),
            ],
        }
    }

    /// 完全擦除策略（RMA/退役 — 删除所有数据）。
    pub fn full_wipe() -> Self {
        Self {
            preserve: Vec::new(),
            delete: vec![
                DeleteRule::new(DataCategory::DeviceIdentity, DeleteMethod::DeepOverwrite),
                DeleteRule::new(DataCategory::Calibration, DeleteMethod::DeepOverwrite),
                DeleteRule::new(DataCategory::AuditLog, DeleteMethod::DeepOverwrite),
                DeleteRule::new(DataCategory::License, DeleteMethod::CryptoErase),
                DeleteRule::new(DataCategory::UserConfig, DeleteMethod::CryptoErase),
                DeleteRule::new(DataCategory::BusinessData, DeleteMethod::DeepOverwrite),
                DeleteRule::new(DataCategory::AiModel, DeleteMethod::QuickDelete),
                DeleteRule::new(DataCategory::SessionKeys, DeleteMethod::CryptoErase),
                DeleteRule::new(DataCategory::RuntimeLogs, DeleteMethod::QuickDelete),
                DeleteRule::new(DataCategory::NetworkConfig, DeleteMethod::CryptoErase),
            ],
        }
    }

    /// 校验策略一致性（保留和删除不能重叠）。
    pub fn validate(&self) -> Result<(), &'static str> {
        for p in &self.preserve {
            if self.delete.iter().any(|d| d.category == p.category) {
                return Err("category appears in both preserve and delete rules");
            }
        }
        Ok(())
    }

    /// 检查类别是否应保留。
    pub fn should_preserve(&self, category: DataCategory) -> bool {
        self.preserve.iter().any(|r| r.category == category)
    }

    /// 检查类别是否应删除。
    pub fn should_delete(&self, category: DataCategory) -> bool {
        self.delete.iter().any(|r| r.category == category)
    }
}

/// 重置结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetResult {
    /// 是否成功。
    pub success: bool,
    /// 保留的数据（类别 → 是否验证完好）。
    pub preserved: Vec<(DataCategory, bool)>,
    /// 删除的数据（类别 → 是否验证已删除）。
    pub deleted: Vec<(DataCategory, bool)>,
    /// 错误信息。
    pub errors: Vec<String>,
}

/// 工厂重置执行器。
pub struct FactoryReset {
    policy: ResetPolicy,
}

impl FactoryReset {
    pub fn new(policy: ResetPolicy) -> Result<Self, &'static str> {
        policy.validate()?;
        Ok(Self { policy })
    }

    /// 执行工厂重置。
    ///
    /// `preserved_intact` 和 `deleted_gone` 为调用方提供的验证结果
    ///（实际由存储层验证，这里做逻辑编排）。
    pub fn execute(
        &self,
        preserved_intact: &[(DataCategory, bool)],
        deleted_gone: &[(DataCategory, bool)],
    ) -> ResetResult {
        let mut errors = Vec::new();
        let mut preserved = Vec::new();
        let mut deleted = Vec::new();

        // 验证保留的数据完整
        for rule in &self.policy.preserve {
            let intact = preserved_intact
                .iter()
                .find(|(c, _)| *c == rule.category)
                .map(|(_, b)| *b)
                .unwrap_or(false);

            if !intact {
                errors.push(format!(
                    "preserved category {} was not intact after reset",
                    rule.category.label()
                ));
            }
            preserved.push((rule.category, intact));
        }

        // 验证删除的数据已清除
        for rule in &self.policy.delete {
            let gone = deleted_gone
                .iter()
                .find(|(c, _)| *c == rule.category)
                .map(|(_, b)| *b)
                .unwrap_or(false);

            if !gone {
                errors.push(format!(
                    "deleted category {} was still present after reset",
                    rule.category.label()
                ));
            }
            deleted.push((rule.category, gone));
        }

        let success = errors.is_empty();

        ResetResult {
            success,
            preserved,
            deleted,
            errors,
        }
    }

    /// 策略引用。
    pub fn policy(&self) -> &ResetPolicy {
        &self.policy
    }
}

/// 所有数据类别。
impl DataCategory {
    pub fn all() -> &'static [DataCategory] {
        &[
            DataCategory::DeviceIdentity,
            DataCategory::Calibration,
            DataCategory::AuditLog,
            DataCategory::License,
            DataCategory::UserConfig,
            DataCategory::BusinessData,
            DataCategory::AiModel,
            DataCategory::SessionKeys,
            DataCategory::RuntimeLogs,
            DataCategory::NetworkConfig,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_policy_validate_ok() {
        let policy = ResetPolicy::production();
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn full_wipe_validate_ok() {
        let policy = ResetPolicy::full_wipe();
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn conflicting_rules_rejected() {
        let policy = ResetPolicy {
            preserve: vec![PreserveRule::new(DataCategory::UserConfig, "test")],
            delete: vec![DeleteRule::new(
                DataCategory::UserConfig,
                DeleteMethod::CryptoErase,
            )],
        };
        assert!(FactoryReset::new(policy).is_err());
    }

    #[test]
    fn factory_reset_success() {
        let reset = FactoryReset::new(ResetPolicy::production()).unwrap();

        let preserved_intact = vec![
            (DataCategory::DeviceIdentity, true),
            (DataCategory::Calibration, true),
            (DataCategory::AuditLog, true),
            (DataCategory::License, true),
        ];
        let deleted_gone = vec![
            (DataCategory::UserConfig, true),
            (DataCategory::BusinessData, true),
            (DataCategory::AiModel, true),
            (DataCategory::SessionKeys, true),
            (DataCategory::RuntimeLogs, true),
            (DataCategory::NetworkConfig, true),
        ];

        let result = reset.execute(&preserved_intact, &deleted_gone);
        assert!(result.success);
        assert!(result.errors.is_empty());
        assert_eq!(result.preserved.len(), 4);
        assert_eq!(result.deleted.len(), 6);
    }

    #[test]
    fn factory_reset_preserved_data_lost_fails() {
        let reset = FactoryReset::new(ResetPolicy::production()).unwrap();

        let preserved_intact = vec![
            (DataCategory::DeviceIdentity, false), // identity lost!
            (DataCategory::Calibration, true),
            (DataCategory::AuditLog, true),
            (DataCategory::License, true),
        ];
        let deleted_gone = vec![
            (DataCategory::UserConfig, true),
            (DataCategory::BusinessData, true),
            (DataCategory::AiModel, true),
            (DataCategory::SessionKeys, true),
            (DataCategory::RuntimeLogs, true),
            (DataCategory::NetworkConfig, true),
        ];

        let result = reset.execute(&preserved_intact, &deleted_gone);
        assert!(!result.success);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn factory_reset_deleted_data_remains_fails() {
        let reset = FactoryReset::new(ResetPolicy::production()).unwrap();

        let preserved_intact = vec![
            (DataCategory::DeviceIdentity, true),
            (DataCategory::Calibration, true),
            (DataCategory::AuditLog, true),
            (DataCategory::License, true),
        ];
        let deleted_gone = vec![
            (DataCategory::UserConfig, true),
            (DataCategory::BusinessData, false), // data still present!
            (DataCategory::AiModel, true),
            (DataCategory::SessionKeys, true),
            (DataCategory::RuntimeLogs, true),
            (DataCategory::NetworkConfig, true),
        ];

        let result = reset.execute(&preserved_intact, &deleted_gone);
        assert!(!result.success);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn full_wipe_deletes_identity() {
        let reset = FactoryReset::new(ResetPolicy::full_wipe()).unwrap();

        // Full wipe: nothing preserved, everything deleted
        let preserved_intact: Vec<(DataCategory, bool)> = Vec::new();
        let deleted_gone: Vec<(DataCategory, bool)> =
            DataCategory::all().iter().map(|c| (*c, true)).collect();

        let result = reset.execute(&preserved_intact, &deleted_gone);
        assert!(result.success);
        assert!(result.preserved.is_empty());
        assert_eq!(result.deleted.len(), 10);
    }

    #[test]
    fn should_preserve_and_should_delete() {
        let policy = ResetPolicy::production();
        assert!(policy.should_preserve(DataCategory::DeviceIdentity));
        assert!(!policy.should_preserve(DataCategory::UserConfig));
        assert!(policy.should_delete(DataCategory::UserConfig));
        assert!(!policy.should_delete(DataCategory::DeviceIdentity));
    }

    #[test]
    fn missing_preserved_data_marked_error() {
        let reset = FactoryReset::new(ResetPolicy::production()).unwrap();

        // Only 2/4 preserved items reported
        let preserved_intact = vec![
            (DataCategory::DeviceIdentity, true),
            (DataCategory::Calibration, true),
            // AuditLog and License missing
        ];
        let deleted_gone = vec![
            (DataCategory::UserConfig, true),
            (DataCategory::BusinessData, true),
            (DataCategory::AiModel, true),
            (DataCategory::SessionKeys, true),
            (DataCategory::RuntimeLogs, true),
            (DataCategory::NetworkConfig, true),
        ];

        let result = reset.execute(&preserved_intact, &deleted_gone);
        assert!(!result.success);
        // AuditLog and License should be marked as not intact
        assert!(result
            .preserved
            .iter()
            .any(|(c, ok)| *c == DataCategory::AuditLog && !ok));
        assert!(result
            .preserved
            .iter()
            .any(|(c, ok)| *c == DataCategory::License && !ok));
    }
}
