//! BSP 认证规范管理

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::types::{BspCategory, BspItem, BspSpec};
use crate::CertError;

/// 返回默认 BSP 认证规范（每类别一个强制项，共 9 项）
pub fn default_spec() -> BspSpec {
    let items: Vec<BspItem> = vec![
        BspItem {
            id: String::from("cpu-basic"),
            category: BspCategory::Cpu,
            mandatory: true,
        },
        BspItem {
            id: String::from("mem-basic"),
            category: BspCategory::Mem,
            mandatory: true,
        },
        BspItem {
            id: String::from("clock-basic"),
            category: BspCategory::Clock,
            mandatory: true,
        },
        BspItem {
            id: String::from("irq-basic"),
            category: BspCategory::Irq,
            mandatory: true,
        },
        BspItem {
            id: String::from("serial-basic"),
            category: BspCategory::Serial,
            mandatory: true,
        },
        BspItem {
            id: String::from("net-basic"),
            category: BspCategory::Net,
            mandatory: true,
        },
        BspItem {
            id: String::from("storage-basic"),
            category: BspCategory::Storage,
            mandatory: true,
        },
        BspItem {
            id: String::from("crypto-basic"),
            category: BspCategory::Crypto,
            mandatory: true,
        },
        BspItem {
            id: String::from("power-basic"),
            category: BspCategory::Power,
            mandatory: true,
        },
    ];
    BspSpec {
        version: String::from("1.0.0"),
        items,
    }
}

/// 校验认证规范合法性：至少包含 1 个强制项
pub fn validate_spec(spec: &BspSpec) -> Result<(), CertError> {
    if spec.items.is_empty() {
        return Err(CertError::InvalidSpec);
    }
    let has_mandatory = spec.items.iter().any(|item| item.mandatory);
    if !has_mandatory {
        return Err(CertError::InvalidSpec);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_spec_has_9_items() {
        let spec = default_spec();
        assert_eq!(spec.items.len(), 9);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_default_spec_all_mandatory() {
        let spec = default_spec();
        for item in &spec.items {
            assert!(item.mandatory);
        }
    }

    #[test]
    fn test_default_spec_covers_all_categories() {
        let spec = default_spec();
        let categories = [
            BspCategory::Cpu,
            BspCategory::Mem,
            BspCategory::Clock,
            BspCategory::Irq,
            BspCategory::Serial,
            BspCategory::Net,
            BspCategory::Storage,
            BspCategory::Crypto,
            BspCategory::Power,
        ];
        for cat in &categories {
            assert!(
                spec.items.iter().any(|i| i.category == *cat),
                "missing category {:?}",
                cat
            );
        }
    }

    #[test]
    fn test_validate_spec_ok() {
        let spec = default_spec();
        assert!(validate_spec(&spec).is_ok());
    }

    #[test]
    fn test_validate_spec_empty_items() {
        let spec = BspSpec {
            version: String::from("0.1.0"),
            items: Vec::new(),
        };
        assert_eq!(validate_spec(&spec), Err(CertError::InvalidSpec));
    }

    #[test]
    fn test_validate_spec_no_mandatory() {
        let spec = BspSpec {
            version: String::from("0.1.0"),
            items: vec![BspItem {
                id: String::from("opt-01"),
                category: BspCategory::Cpu,
                mandatory: false,
            }],
        };
        assert_eq!(validate_spec(&spec), Err(CertError::InvalidSpec));
    }

    #[test]
    fn test_validate_spec_with_optional_and_mandatory() {
        let spec = BspSpec {
            version: String::from("1.0.0"),
            items: vec![
                BspItem {
                    id: String::from("opt-01"),
                    category: BspCategory::Net,
                    mandatory: false,
                },
                BspItem {
                    id: String::from("man-01"),
                    category: BspCategory::Cpu,
                    mandatory: true,
                },
            ],
        };
        assert!(validate_spec(&spec).is_ok());
    }
}
