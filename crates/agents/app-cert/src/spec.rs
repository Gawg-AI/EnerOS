//! 认证规范管理

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::types::{AppCertCategory, AppCertItem, AppCertSpec};

/// 生成默认应用认证规范（5 个强制项，每类别一个）
pub fn default_app_spec() -> AppCertSpec {
    let items: Vec<AppCertItem> = vec![
        AppCertItem {
            id: String::from("sdk-compat-001"),
            category: AppCertCategory::SdkCompat,
            mandatory: true,
        },
        AppCertItem {
            id: String::from("security-001"),
            category: AppCertCategory::Security,
            mandatory: true,
        },
        AppCertItem {
            id: String::from("performance-001"),
            category: AppCertCategory::Performance,
            mandatory: true,
        },
        AppCertItem {
            id: String::from("resource-limit-001"),
            category: AppCertCategory::ResourceLimit,
            mandatory: true,
        },
        AppCertItem {
            id: String::from("behavior-audit-001"),
            category: AppCertCategory::BehaviorAudit,
            mandatory: true,
        },
    ];

    AppCertSpec {
        version: String::from("1.0.0"),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_app_spec_has_5_items() {
        let spec = default_app_spec();
        assert_eq!(spec.items.len(), 5);
    }

    #[test]
    fn test_default_app_spec_version() {
        let spec = default_app_spec();
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_default_app_spec_all_mandatory() {
        let spec = default_app_spec();
        for item in &spec.items {
            assert!(item.mandatory);
        }
    }

    #[test]
    fn test_default_app_spec_covers_all_categories() {
        let spec = default_app_spec();
        let categories: Vec<AppCertCategory> = spec.items.iter().map(|i| i.category).collect();
        assert!(categories.contains(&AppCertCategory::SdkCompat));
        assert!(categories.contains(&AppCertCategory::Security));
        assert!(categories.contains(&AppCertCategory::Performance));
        assert!(categories.contains(&AppCertCategory::ResourceLimit));
        assert!(categories.contains(&AppCertCategory::BehaviorAudit));
    }

    #[test]
    fn test_default_app_spec_unique_ids() {
        let spec = default_app_spec();
        let mut ids: Vec<&str> = spec.items.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }
}
