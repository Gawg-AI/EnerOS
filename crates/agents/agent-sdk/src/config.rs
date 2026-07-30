//! 配置 API（SDK v1.0 稳定 API）。
//!
//! 提供 Agent 配置读写接口。SDK 层提供 Mock 实现（内存键值对），
//! 真实配置由 Agent Runtime 从 TOML 文件加载注入。

use heapless::String as HString;

use crate::agent::AgentError;

/// 最大配置项数。
pub const MAX_CONFIG_ENTRIES: usize = 32;

/// 配置项。
#[derive(Debug, Clone)]
struct ConfigEntry {
    key: HString<64>,
    value: HString<256>,
}

/// 配置 API。
///
/// Agent 通过此结构读取运行时配置。
pub struct Config {
    /// 配置项列表
    entries: [Option<ConfigEntry>; MAX_CONFIG_ENTRIES],
    /// 配置项数量
    count: usize,
}

impl Config {
    /// 创建空配置。
    pub fn new() -> Self {
        const NONE: Option<ConfigEntry> = None;
        Self {
            entries: [NONE; MAX_CONFIG_ENTRIES],
            count: 0,
        }
    }

    /// 设置配置项（内部/测试用）。
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), AgentError> {
        let hkey = HString::<64>::try_from(key).map_err(|_| {
            AgentError::ConfigError(HString::<128>::try_from("key too long").unwrap_or_default())
        })?;
        let hval = HString::<256>::try_from(value).map_err(|_| {
            AgentError::ConfigError(HString::<128>::try_from("value too long").unwrap_or_default())
        })?;

        // 查找已存在的键
        for e in self.entries.iter_mut().flatten() {
            if e.key.as_str() == key {
                e.value = hval;
                return Ok(());
            }
        }

        // 插入新键
        if self.count >= MAX_CONFIG_ENTRIES {
            return Err(AgentError::ConfigError(
                HString::<128>::try_from("config full").unwrap_or_default(),
            ));
        }
        for entry in &mut self.entries {
            if entry.is_none() {
                *entry = Some(ConfigEntry {
                    key: hkey,
                    value: hval,
                });
                self.count += 1;
                return Ok(());
            }
        }
        Err(AgentError::ConfigError(
            HString::<128>::try_from("config full").unwrap_or_default(),
        ))
    }

    /// 获取字符串配置。
    pub fn get_str(&self, key: &str) -> Result<&str, AgentError> {
        for e in self.entries.iter().flatten() {
            if e.key.as_str() == key {
                return Ok(e.value.as_str());
            }
        }
        Err(AgentError::ConfigError(
            HString::<128>::try_from("key not found").unwrap_or_default(),
        ))
    }

    /// 获取 u32 配置。
    pub fn get_u32(&self, key: &str) -> Result<u32, AgentError> {
        let s = self.get_str(key)?;
        s.parse::<u32>().map_err(|_| {
            AgentError::ConfigError(
                HString::<128>::try_from("parse u32 failed").unwrap_or_default(),
            )
        })
    }

    /// 获取 u64 配置。
    pub fn get_u64(&self, key: &str) -> Result<u64, AgentError> {
        let s = self.get_str(key)?;
        s.parse::<u64>().map_err(|_| {
            AgentError::ConfigError(
                HString::<128>::try_from("parse u64 failed").unwrap_or_default(),
            )
        })
    }

    /// 获取 bool 配置。
    pub fn get_bool(&self, key: &str) -> Result<bool, AgentError> {
        let s = self.get_str(key)?;
        match s {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err(AgentError::ConfigError(
                HString::<128>::try_from("parse bool failed").unwrap_or_default(),
            )),
        }
    }

    /// 获取配置项数量。
    pub fn count(&self) -> usize {
        self.count
    }

    /// 检查键是否存在。
    pub fn contains(&self, key: &str) -> bool {
        for e in self.entries.iter().flatten() {
            if e.key.as_str() == key {
                return true;
            }
        }
        false
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// CFG1：设置和获取字符串。
    #[test]
    fn cfg1_set_get_str() {
        let mut config = Config::new();
        config.set("name", "test-agent").expect("设置应成功");
        assert_eq!(config.get_str("name").expect("获取应成功"), "test-agent");
    }

    /// CFG2：获取 u32。
    #[test]
    fn cfg2_get_u32() {
        let mut config = Config::new();
        config.set("interval_ms", "1000").expect("设置应成功");
        assert_eq!(config.get_u32("interval_ms").expect("获取应成功"), 1000);
    }

    /// CFG3：获取 u64。
    #[test]
    fn cfg3_get_u64() {
        let mut config = Config::new();
        config.set("timeout_us", "5000000").expect("设置应成功");
        assert_eq!(config.get_u64("timeout_us").expect("获取应成功"), 5000000);
    }

    /// CFG4：获取 bool。
    #[test]
    fn cfg4_get_bool() {
        let mut config = Config::new();
        config.set("enabled", "true").expect("设置应成功");
        assert!(config.get_bool("enabled").expect("获取应成功"));

        config.set("disabled", "false").expect("设置应成功");
        assert!(!config.get_bool("disabled").expect("获取应成功"));
    }

    /// CFG5：键不存在。
    #[test]
    fn cfg5_key_not_found() {
        let config = Config::new();
        let err = config.get_str("nonexistent").expect_err("应失败");
        assert!(matches!(err, AgentError::ConfigError(_)));
    }

    /// CFG6：覆盖配置。
    #[test]
    fn cfg6_overwrite() {
        let mut config = Config::new();
        config.set("key", "v1").expect("设置应成功");
        config.set("key", "v2").expect("覆盖应成功");
        assert_eq!(config.get_str("key").expect("获取应成功"), "v2");
        assert_eq!(config.count(), 1);
    }

    /// CFG7：contains 检查。
    #[test]
    fn cfg7_contains() {
        let mut config = Config::new();
        assert!(!config.contains("key"));
        config.set("key", "v").expect("设置应成功");
        assert!(config.contains("key"));
    }

    /// CFG8：解析失败。
    #[test]
    fn cfg8_parse_failure() {
        let mut config = Config::new();
        config.set("bad", "not_a_number").expect("设置应成功");
        let err = config.get_u32("bad").expect_err("应失败");
        assert!(matches!(err, AgentError::ConfigError(_)));
    }
}
