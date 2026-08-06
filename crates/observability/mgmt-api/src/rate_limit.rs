//! 滑动窗口限流器 (OBS-010).
//!
//! 按 (role, action) 维度限流。滑动窗口算法：
//! 记录窗口内的请求时间戳，超过 `max_requests` 则拒绝。
//!
//! 设计：
//! - 每个限流键维护一个时间戳队列。
//! - 请求到来时，先清除窗口外的旧时间戳。
//! - 如果队列长度 < max_requests，允许请求并添加时间戳。
//! - 否则拒绝，返回 retry_after_secs（窗口内最旧时间戳 + window_secs - now）。

use alloc::vec::Vec;

use crate::RateLimitKey;

/// 限流配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// 窗口内最大请求数。
    pub max_requests: u32,
    /// 窗口大小（秒）。
    pub window_secs: u64,
}

/// 限流错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitError {
    /// 超过限流，需在 `retry_after_secs` 秒后重试。
    TooManyRequests { retry_after_secs: u64 },
}

/// 滑动窗口限流器。
///
/// 存储每个限流键的时间戳队列。
/// 容量由 `max_keys` 限制，超过时驱逐最久未使用的键。
pub struct RateLimiter {
    config: RateLimitConfig,
    /// 限流键列表。
    keys: Vec<RateLimitKey>,
    /// 每个键的时间戳队列（与 keys 一一对应）。
    windows: Vec<Vec<u64>>,
    /// 最大键数。
    max_keys: usize,
}

impl RateLimiter {
    /// 创建限流器。
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            keys: Vec::new(),
            windows: Vec::new(),
            max_keys: 256,
        }
    }

    /// 设置最大键数。
    pub fn with_max_keys(mut self, max_keys: usize) -> Self {
        self.max_keys = max_keys;
        self
    }

    /// 检查请求是否被允许。
    ///
    /// 如果允许，记录时间戳并返回 Ok(())。
    /// 如果拒绝，返回 `TooManyRequests` 及建议重试时间。
    pub fn check(&mut self, key: &RateLimitKey, now_unix: u64) -> Result<(), RateLimitError> {
        let window_start = now_unix.saturating_sub(self.config.window_secs);

        // 查找或创建键的窗口（v0.239.0：真 LRU——命中提升到末尾，
        // 驱逐最久未用（头部））。
        let idx = match self.keys.iter().position(|k| k == key) {
            Some(idx) => {
                // LRU 提升：命中键移到末尾。
                let k = self.keys.remove(idx);
                let w = self.windows.remove(idx);
                self.keys.push(k);
                self.windows.push(w);
                self.keys.len() - 1
            }
            None => {
                // 容量满时驱逐最久未用（头部）。
                if self.keys.len() >= self.max_keys {
                    self.keys.remove(0);
                    self.windows.remove(0);
                }
                self.keys.push(key.clone());
                self.windows.push(Vec::new());
                self.keys.len() - 1
            }
        };

        let window = &mut self.windows[idx];

        // 清除窗口外的旧时间戳
        window.retain(|&ts| ts > window_start);

        // 检查是否超过限制
        if window.len() >= self.config.max_requests as usize {
            // 最旧时间戳 + window_secs = 该请求的可用时间
            let oldest = *window.first().unwrap_or(&now_unix);
            let retry_after = oldest
                .saturating_add(self.config.window_secs)
                .saturating_sub(now_unix);
            return Err(RateLimitError::TooManyRequests {
                retry_after_secs: retry_after,
            });
        }

        // 允许请求，记录时间戳
        window.push(now_unix);
        Ok(())
    }

    /// 获取当前键的数量。
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// 清空所有限流状态。
    pub fn clear(&mut self) {
        self.keys.clear();
        self.windows.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbac::Role;
    use crate::schema::ApiAction;

    fn make_key() -> RateLimitKey {
        RateLimitKey::new(Role::Admin, ApiAction::StatusQuery)
    }

    #[test]
    fn allows_under_limit() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 5,
            window_secs: 60,
        });
        let key = make_key();
        for _ in 0..5 {
            assert!(limiter.check(&key, 1000).is_ok());
        }
    }

    #[test]
    fn rejects_over_limit() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 3,
            window_secs: 60,
        });
        let key = make_key();
        for _ in 0..3 {
            assert!(limiter.check(&key, 1000).is_ok());
        }
        let err = limiter.check(&key, 1001).unwrap_err();
        match err {
            RateLimitError::TooManyRequests { retry_after_secs } => {
                // 最旧时间戳 1000 + 60 - 1001 = 59
                assert_eq!(retry_after_secs, 59);
            }
        }
    }

    #[test]
    fn window_slides_allows_after_expiry() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
        });
        let key = make_key();
        assert!(limiter.check(&key, 1000).is_ok());
        assert!(limiter.check(&key, 1001).is_ok());
        assert!(limiter.check(&key, 1002).is_err());

        // 61 秒后窗口滑动
        assert!(limiter.check(&key, 1061).is_ok());
    }

    #[test]
    fn different_keys_independent() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
        });
        let key1 = RateLimitKey::new(Role::Admin, ApiAction::StatusQuery);
        let key2 = RateLimitKey::new(Role::Operator, ApiAction::StatusQuery);

        assert!(limiter.check(&key1, 1000).is_ok());
        assert!(limiter.check(&key1, 1001).is_ok());
        assert!(limiter.check(&key1, 1002).is_err());

        // 不同角色不受影响
        assert!(limiter.check(&key2, 1002).is_ok());
    }

    #[test]
    fn different_actions_independent() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 1,
            window_secs: 60,
        });
        let key1 = RateLimitKey::new(Role::Admin, ApiAction::StatusQuery);
        let key2 = RateLimitKey::new(Role::Admin, ApiAction::ConfigSet);

        assert!(limiter.check(&key1, 1000).is_ok());
        assert!(limiter.check(&key1, 1001).is_err());

        // 不同动作不受影响
        assert!(limiter.check(&key2, 1002).is_ok());
    }

    #[test]
    fn max_keys_eviction() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 10,
            window_secs: 60,
        })
        .with_max_keys(3);

        // 创建 4 个不同的键
        for i in 0..4 {
            let key = RateLimitKey::new(Role::Viewer, ApiAction::StatusQuery);
            limiter.check(&key, 1000 + i).ok();
        }
        // 等等 — 同一个键。让我用不同角色
        let roles = [Role::Viewer, Role::Operator, Role::Admin, Role::Superuser];
        for (i, &role) in roles.iter().enumerate() {
            let key = RateLimitKey::new(role, ApiAction::StatusQuery);
            limiter.check(&key, 1000 + i as u64).ok();
        }

        // max_keys=3，应驱逐第一个
        assert_eq!(limiter.key_count(), 3);
    }

    #[test]
    fn clear_resets_state() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 1,
            window_secs: 60,
        });
        let key = make_key();
        assert!(limiter.check(&key, 1000).is_ok());
        assert!(limiter.check(&key, 1001).is_err());
        limiter.clear();
        assert!(limiter.check(&key, 1002).is_ok());
    }
}
