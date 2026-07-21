//! Agent ID 生成 — 基于原子计数器的全局唯一 ID
//!
//! 使用 `AtomicU64` 计数器从 1 开始递增，上 64 位预留为 epoch（当前为 0）。
//! 零外部依赖，no_std 兼容。

use core::sync::atomic::{AtomicU64, Ordering};

/// Agent 唯一标识符.
///
/// 128 位宽度：上 64 位为 epoch（预留，当前为 0），下 64 位为计数器值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(pub u128);

impl AgentId {
    /// 无效 ID 常量（全零）.
    pub const ZERO: AgentId = AgentId(0);

    /// 生成全局唯一的 Agent ID.
    ///
    /// 基于 `AtomicU64` 计数器，从 1 开始递增。
    /// 上 64 位为 0（预留 epoch），下 64 位为计数器值。
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        AgentId(id as u128)
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::*;

    #[test]
    fn test_generate_nonzero() {
        let id = AgentId::generate();
        assert_ne!(id, AgentId::ZERO);
        assert_ne!(id.0, 0);
    }

    #[test]
    fn test_generate_unique_100() {
        let mut seen = BTreeSet::new();
        for _ in 0..100 {
            let id = AgentId::generate();
            assert!(seen.insert(id.0), "duplicate id generated");
        }
        assert_eq!(seen.len(), 100);
    }

    #[test]
    fn test_zero_constant() {
        assert_eq!(AgentId::ZERO.0, 0);
        assert_eq!(AgentId::ZERO, AgentId(0));
    }
}
