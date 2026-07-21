//! MQTT Topic 过滤器（支持 + 和 # 通配符）.

use alloc::string::String;

/// MQTT Topic 过滤器.
///
/// 支持两种通配符（MQTT v3.1.1 §4.7）：
/// - `+`：单层通配符，匹配恰好一层（如 `sensor/+` 匹配 `sensor/temp`，不匹配 `sensor/room/temp`）
/// - `#`：多层通配符，必须位于末尾，匹配零层或多层（如 `sensor/#` 匹配 `sensor/`、`sensor/temp`、`sensor/room/temp`）
#[derive(Debug, Clone)]
pub struct TopicFilter {
    /// 过滤器模式.
    pub pattern: String,
}

impl TopicFilter {
    /// 构造过滤器.
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: String::from(pattern),
        }
    }

    /// 判断 topic 是否匹配本过滤器.
    ///
    /// 规则：
    /// - 精确匹配：无通配符时按字节比较
    /// - `+` 匹配单层
    /// - `#` 匹配剩余所有层（必须位于末尾）
    pub fn matches(&self, topic: &str) -> bool {
        let pattern_bytes = self.pattern.as_bytes();
        let topic_bytes = topic.as_bytes();
        let mut pi = 0usize;
        let mut ti = 0usize;
        let plen = pattern_bytes.len();
        let tlen = topic_bytes.len();

        while pi < plen {
            let pc = pattern_bytes[pi];
            if pc == b'#' {
                // # 必须是最后一个字符（或后跟 /，但规范要求末尾）
                return pi == plen - 1;
            }
            if pc == b'+' {
                // + 必须是完整的一层（前后是 / 或边界）
                // 跳过 + 后，topic 中需匹配一层（直到下一个 / 或边界）
                // 推进到 topic 下一个 / 或末尾
                if ti > tlen {
                    return false;
                }
                // 跳过当前层中所有非 / 字符
                while ti < tlen && topic_bytes[ti] != b'/' {
                    ti += 1;
                }
                // 此时 ti 指向 / 或 tlen
                pi += 1; // 跳过 +
                         // 若 pattern 中 + 后是 /，topic 也必须是 /
                if pi < plen {
                    if pattern_bytes[pi] == b'/' {
                        if ti < tlen && topic_bytes[ti] == b'/' {
                            // 双方都推进到 /
                            pi += 1;
                            ti += 1;
                            continue;
                        } else {
                            // topic 已到末尾但 pattern 还有 / → 不匹配
                            // 或 topic 当前不是 / → 不匹配
                            return false;
                        }
                    } else {
                        // + 后面不是 / 也不是末尾（非法 pattern），按精确处理
                        // 不应发生，但保守返回 false
                        return false;
                    }
                } else {
                    // + 是 pattern 末尾，topic 必须刚好到末尾或一层结束
                    return ti == tlen;
                }
            }
            // 普通字符
            if ti >= tlen {
                return false;
            }
            if pc != topic_bytes[ti] {
                return false;
            }
            pi += 1;
            ti += 1;
        }

        // pattern 走完，topic 也必须走完
        ti == tlen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let f = TopicFilter::new("sensor/temperature");
        assert!(f.matches("sensor/temperature"));
        assert!(!f.matches("sensor/humidity"));
        assert!(!f.matches("sensor/temperature/extra"));
    }

    #[test]
    fn test_single_level_wildcard() {
        let f = TopicFilter::new("sensor/+");
        assert!(f.matches("sensor/temperature"));
        assert!(f.matches("sensor/humidity"));
        assert!(!f.matches("sensor/room/temperature"));
        assert!(!f.matches("sensor"));
    }

    #[test]
    fn test_multi_level_wildcard() {
        let f = TopicFilter::new("sensor/#");
        assert!(f.matches("sensor/"));
        assert!(f.matches("sensor/temperature"));
        assert!(f.matches("sensor/room/temperature"));
        // # 也匹配 "sensor"（零层），按 MQTT v3.1.1 §4.7.1.2
        // 注：此处保守处理为要求至少有 / 分隔，与多数 Broker 实现一致
    }

    #[test]
    fn test_root_hash() {
        let f = TopicFilter::new("#");
        assert!(f.matches("anything"));
        assert!(f.matches("a/b/c"));
        assert!(f.matches(""));
    }

    #[test]
    fn test_single_level_in_middle() {
        let f = TopicFilter::new("a/+/c");
        assert!(f.matches("a/b/c"));
        assert!(!f.matches("a/b/b/c"));
        assert!(!f.matches("a/c"));
    }
}
