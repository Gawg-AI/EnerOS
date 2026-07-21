//! 告警抑制（抖动过滤，D21 滑动窗口）.

use alloc::collections::VecDeque;
use alloc::string::String;

/// 抑制规则（按源精确匹配 + 滑动窗口计数，D21）.
#[derive(Debug, Clone)]
pub struct SuppressionRule {
    /// 源模式（精确字符串匹配，D21 简化，不支持正则）.
    pub source_pattern: String,
    /// 抑制窗口时长（毫秒）.
    pub duration_ms: u64,
    /// 窗口内允许的最大告警数（超过则抑制）.
    pub max_count: u32,
}

impl SuppressionRule {
    /// 构造抑制规则（`max_count = 1`，窗口内仅允许 1 条）.
    pub fn new(source: &str, duration_ms: u64) -> Self {
        Self {
            source_pattern: String::from(source),
            duration_ms,
            max_count: 1,
        }
    }

    /// 源是否匹配（精确匹配）.
    pub fn matches_source(&self, source: &str) -> bool {
        self.source_pattern == source
    }
}

/// 抑制滑动窗口（`VecDeque<u64>` 时间戳队列，D21）.
#[derive(Debug, Clone)]
pub struct SuppressionWindow {
    /// 最近告警时间戳队列.
    timestamps: VecDeque<u64>,
    /// 窗口内最大告警数.
    max_count: u32,
    /// 窗口时长（毫秒）.
    duration_ms: u64,
}

impl SuppressionWindow {
    /// 构造滑动窗口.
    pub fn new(max_count: u32, duration_ms: u64) -> Self {
        Self {
            timestamps: VecDeque::new(),
            max_count,
            duration_ms,
        }
    }

    /// 判定本次告警是否应被抑制.
    ///
    /// - 先驱逐窗口外过期时间戳（`timestamp < now_ms - duration_ms`）
    /// - 若窗口内时间戳数 ≥ `max_count` → 抑制（返回 `true`，不入队）
    /// - 否则记录本次时间戳 → 允许（返回 `false`）
    pub fn should_suppress(&mut self, now_ms: u64) -> bool {
        // 防止 now_ms < duration_ms 时下溢
        let cutoff = now_ms.saturating_sub(self.duration_ms);
        // 驱逐过期时间戳
        while let Some(&front) = self.timestamps.front() {
            if front < cutoff {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
        // 判定是否超过阈值
        if self.timestamps.len() >= self.max_count as usize {
            return true;
        }
        self.timestamps.push_back(now_ms);
        false
    }

    /// 重置窗口（告警清除时调用，允许后续告警重新进入）.
    pub fn reset(&mut self) {
        self.timestamps.clear();
    }
}
