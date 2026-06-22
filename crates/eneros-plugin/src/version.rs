//! API 版本兼容性检查
//!
//! 基于 SemVer 规则判断插件声明的 API 版本是否与当前系统 API 版本兼容：
//! - 主版本号为 0 时（预发布阶段），次版本号必须相同（0.27.x 与 0.27.y 兼容）。
//! - 主版本号 >= 1 时，主版本号相同即兼容（1.x.y 与 1.a.b 兼容）。
//! - 不同主版本号不兼容。

use crate::error::PluginError;

/// 当前系统插件 API 版本
///
/// v0.28.0 Task 11：供 `PluginLoader::load_with_mode` 在 inline 模式下
/// 统一执行版本兼容性检查使用。与 EnerOS 主版本保持同步。
pub const CURRENT_API_VERSION: &str = "0.28.0";

/// 解析版本号的 (主版本号, 次版本号) 部分
///
/// 支持 `v` 前缀与预发布后缀（如 `0.27.0-rc.1`）。
/// 解析失败返回 `None`。
fn parse_version(version: &str) -> Option<(u32, u32)> {
    let version = version.trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    // 去除预发布后缀
    let main_part = version.split('-').next().unwrap_or(version);
    let parts: Vec<&str> = main_part.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some((major, minor))
}

/// 检查插件 API 版本与当前 API 版本的兼容性
///
/// 兼容规则：
/// - 任一方主版本号为 0（预发布）：双方 (major, minor) 必须完全相同。
/// - 双方主版本号 >= 1：主版本号相同即兼容。
/// - 解析失败返回 `PluginError::IncompatibleVersion`。
pub fn check_compatibility(
    plugin_api_version: &str,
    current_api_version: &str,
) -> Result<(), PluginError> {
    let plugin_ver = parse_version(plugin_api_version).ok_or_else(|| {
        PluginError::IncompatibleVersion {
            plugin: plugin_api_version.to_string(),
            current: current_api_version.to_string(),
        }
    })?;
    let current_ver = parse_version(current_api_version).ok_or_else(|| {
        PluginError::IncompatibleVersion {
            plugin: plugin_api_version.to_string(),
            current: current_api_version.to_string(),
        }
    })?;

    let compatible = if plugin_ver.0 == 0 || current_ver.0 == 0 {
        // 预发布版本：次版本号必须相同
        plugin_ver == current_ver
    } else {
        // 正式版本：主版本号相同即可
        plugin_ver.0 == current_ver.0
    };

    if compatible {
        Ok(())
    } else {
        Err(PluginError::IncompatibleVersion {
            plugin: plugin_api_version.to_string(),
            current: current_api_version.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compatible_same_minor_zero_series() {
        assert!(check_compatibility("0.27.0", "0.27.0").is_ok());
        assert!(check_compatibility("0.27.1", "0.27.5").is_ok());
        assert!(check_compatibility("0.27.0", "0.27.3").is_ok());
    }

    #[test]
    fn test_incompatible_different_minor_zero_series() {
        assert!(check_compatibility("0.27.0", "0.28.0").is_err());
        assert!(check_compatibility("0.26.0", "0.27.0").is_err());
    }

    #[test]
    fn test_compatible_same_major_v1() {
        assert!(check_compatibility("1.0.0", "1.5.3").is_ok());
        assert!(check_compatibility("1.2.0", "1.0.0").is_ok());
        assert!(check_compatibility("1.0.0", "1.0.0").is_ok());
    }

    #[test]
    fn test_incompatible_different_major_v1() {
        assert!(check_compatibility("1.0.0", "2.0.0").is_err());
        assert!(check_compatibility("2.0.0", "1.0.0").is_err());
    }

    #[test]
    fn test_incompatible_zero_vs_one() {
        assert!(check_compatibility("0.27.0", "1.0.0").is_err());
        assert!(check_compatibility("1.0.0", "0.27.0").is_err());
    }

    #[test]
    fn test_invalid_version_format() {
        assert!(check_compatibility("invalid", "0.27.0").is_err());
        assert!(check_compatibility("0.27.0", "invalid").is_err());
        assert!(check_compatibility("", "0.27.0").is_err());
        assert!(check_compatibility("1", "1.0.0").is_err());
    }

    #[test]
    fn test_version_with_v_prefix() {
        assert!(check_compatibility("v0.27.0", "0.27.0").is_ok());
        assert!(check_compatibility("0.27.0", "v0.27.1").is_ok());
    }

    #[test]
    fn test_version_with_pre_release_suffix() {
        assert!(check_compatibility("0.27.0-alpha", "0.27.0").is_ok());
        assert!(check_compatibility("0.27.0", "0.27.1-beta.1").is_ok());
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.27.0"), Some((0, 27)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2)));
        assert_eq!(parse_version("v2.0.0"), Some((2, 0)));
        assert_eq!(parse_version("0.27.0-rc.1"), Some((0, 27)));
        assert_eq!(parse_version("invalid"), None);
        assert_eq!(parse_version("1"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn test_error_contains_versions() {
        let result = check_compatibility("0.26.0", "0.27.0");
        match result {
            Err(PluginError::IncompatibleVersion { plugin, current }) => {
                assert_eq!(plugin, "0.26.0");
                assert_eq!(current, "0.27.0");
            }
            other => panic!("expected IncompatibleVersion, got {:?}", other),
        }
    }
}
