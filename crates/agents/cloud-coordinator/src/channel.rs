//! EnerOS v0.95.0 Cloud Coordinator 云边通道抽象.
//!
//! [`CloudChannel`] 为 sync trait（D3/D8：no_std 无 async runtime，单线程惯例
//! 不要求 Send + Sync），[`MockCloudChannel`] 支持故障注入（broadcast 前 N 次
//! 失败）与 ack 预置，供重试/断网补发/集成测试使用（v0.86.0 D11 BidPublisher
//! 模式；Socket v0.29.0 / DDS 适配器后续注入，不在本版本）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D3** | sync 方法 — no_std 无 async runtime（v0.93.0 D5 惯例）；`timeout_ms: u64` 参数注入 |
//! | **D6** | `CloudError` 单变体 MVP — 重试耗尽即广播失败（蓝图未定义） |
//! | **D8** | 本地 sync trait + Mock（v0.86.0 D11 BidPublisher 模式；真实 Socket/DDS 适配器后续注入） |

use alloc::vec::Vec;

use crate::strategy::{EdgeAck, Strategy};

/// 云端通道错误（D6：蓝图未定义 → 单变体 MVP，重试耗尽即广播失败）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudError {
    /// 策略广播失败（网络断开/对端不可达）.
    BroadcastFailed,
}

/// 云边通道抽象（D3/D8：sync，no_std 单线程惯例，不要求 Send + Sync）.
pub trait CloudChannel {
    /// 广播策略到目标边缘；失败返回 [`CloudError::BroadcastFailed`].
    fn broadcast(&mut self, strategy: &Strategy) -> Result<(), CloudError>;

    /// 收集指定策略的边缘 Ack（`timeout_ms` 参数注入，D3；返回已到达的 Ack，可能为空）.
    fn collect_acks(&mut self, strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck>;
}

/// Mock 云边通道（故障注入 + ack 预置，D8）.
///
/// - `fail_times > 0`：broadcast 失败并将 `fail_times` 减一（不记录已发）；
/// - `fail_times == 0`：broadcast 成功，策略克隆记入 `sent`；
/// - `collect_acks`：从预置 `acks` 过滤 `strategy_id` 匹配项克隆返回
///   （不消耗预置 acks，重复调用行为一致）。
pub struct MockCloudChannel {
    /// 剩余失败注入次数（每次失败 broadcast 减一）.
    pub fail_times: u32,
    /// 已成功广播的策略记录.
    pub sent: Vec<Strategy>,
    /// 预置 Ack 池（collect_acks 只读过滤，不消耗）.
    pub acks: Vec<EdgeAck>,
}

impl MockCloudChannel {
    /// 创建空 Mock（无故障注入、无预置 acks）.
    pub fn new() -> Self {
        Self {
            fail_times: 0,
            sent: Vec::new(),
            acks: Vec::new(),
        }
    }

    /// 创建带故障注入的 Mock（前 `fail_times` 次 broadcast 失败）.
    pub fn with_fail_times(fail_times: u32) -> Self {
        Self {
            fail_times,
            ..Self::new()
        }
    }

    /// 创建带预置 acks 的 Mock.
    pub fn with_acks(acks: Vec<EdgeAck>) -> Self {
        Self {
            acks,
            ..Self::new()
        }
    }
}

impl Default for MockCloudChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudChannel for MockCloudChannel {
    fn broadcast(&mut self, strategy: &Strategy) -> Result<(), CloudError> {
        if self.fail_times > 0 {
            self.fail_times -= 1;
            return Err(CloudError::BroadcastFailed);
        }
        self.sent.push(strategy.clone());
        Ok(())
    }

    fn collect_acks(&mut self, strategy_id: u64, _timeout_ms: u64) -> Vec<EdgeAck> {
        self.acks
            .iter()
            .filter(|a| a.strategy_id == strategy_id)
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_coordinator::Priority;
    use eneros_energy_market_agent::Objective;

    use super::*;
    use crate::strategy::{RejectReason, StrategyContent};

    /// 辅助：构造简单策略.
    fn strategy(id: u64) -> Strategy {
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Safety, 0.8);
        Strategy {
            strategy_id: id,
            version: 1,
            targets: vec![10],
            content: StrategyContent::OptimizationWeights(weights),
            deadline: 60_000,
            priority: Priority::Normal,
        }
    }

    /// 辅助：构造 Ack.
    fn ack(strategy_id: u64, edge_id: u64, accepted: bool) -> EdgeAck {
        EdgeAck {
            strategy_id,
            edge_id,
            accepted,
            reason: if accepted {
                None
            } else {
                Some(RejectReason::ExceedsCapacity)
            },
        }
    }

    #[test]
    fn t15_fail_injection_then_success() {
        // 故障注入 2 次 → 前 2 次 Err，第 3 次 Ok 且入 sent.
        let mut ch = MockCloudChannel::with_fail_times(2);
        let s = strategy(1);
        assert_eq!(ch.broadcast(&s), Err(CloudError::BroadcastFailed));
        assert_eq!(ch.broadcast(&s), Err(CloudError::BroadcastFailed));
        assert_eq!(ch.fail_times, 0);
        assert_eq!(ch.broadcast(&s), Ok(()));
        assert_eq!(ch.sent.len(), 1);
    }

    #[test]
    fn t16_broadcast_records_sent() {
        let mut ch = MockCloudChannel::new();
        let s1 = strategy(1);
        let s2 = strategy(2);
        assert_eq!(ch.broadcast(&s1), Ok(()));
        assert_eq!(ch.broadcast(&s2), Ok(()));
        assert_eq!(ch.sent.len(), 2);
        assert_eq!(ch.sent[0], s1);
        assert_eq!(ch.sent[1], s2);
    }

    #[test]
    fn t17_collect_acks_filters_strategy_id() {
        let mut ch = MockCloudChannel::with_acks(vec![
            ack(1, 10, true),
            ack(2, 10, true),
            ack(1, 11, false),
        ]);
        let got = ch.collect_acks(1, 10_000);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ack(1, 10, true));
        assert_eq!(got[1], ack(1, 11, false));
    }

    #[test]
    fn t18_collect_acks_unmatched_id_empty() {
        let mut ch = MockCloudChannel::with_acks(vec![ack(1, 10, true)]);
        assert!(ch.collect_acks(99, 10_000).is_empty());
    }

    #[test]
    fn t19_collect_acks_without_preset_empty() {
        let mut ch = MockCloudChannel::new();
        assert!(ch.collect_acks(1, 10_000).is_empty());
    }

    #[test]
    fn t20_repeated_collect_consistent() {
        // 不消耗预置 acks：重复调用返回一致.
        let mut ch = MockCloudChannel::with_acks(vec![ack(1, 10, true), ack(1, 11, true)]);
        let first = ch.collect_acks(1, 10_000);
        let second = ch.collect_acks(1, 10_000);
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert_eq!(ch.acks.len(), 2);
    }

    #[test]
    fn t21_zero_fail_times_first_success() {
        let mut ch = MockCloudChannel::with_fail_times(0);
        assert_eq!(ch.broadcast(&strategy(1)), Ok(()));
        assert_eq!(ch.sent.len(), 1);
        // with_fail_times(0) 与 new() 行为一致.
        let mut ch2 = MockCloudChannel::new();
        assert_eq!(ch2.broadcast(&strategy(1)), Ok(()));
    }

    #[test]
    fn t22_cloud_error_derive_semantics() {
        let e = CloudError::BroadcastFailed;
        let copied = e; // Copy 语义.
        assert_eq!(e, copied);
        assert_eq!(e, CloudError::BroadcastFailed);
        // Debug 派生可用（输出含变体名）.
        let dbg = alloc::format!("{e:?}");
        assert!(dbg.contains("BroadcastFailed"));
        let _: Vec<CloudError> = vec![e];
    }
}
