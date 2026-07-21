//! EnerOS v0.95.0 Cloud Coordinator 策略发布器.
//!
//! [`StrategyPublisher`] 组合 [`CloudChannel`] 提供：超时重试（§4.4 下发超时重试）、
//! `pending` 断网补发队列（§6.5 网络断开 → 重连补发）、4 个 pub 可观测计数器
//! （§9 Ack/拒绝 metric，D9）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D3** | sync `publish` / `collect_acks` — no_std 无 async runtime（v0.93.0 D5 惯例） |
//! | **D8** | `Box<dyn CloudChannel>` — no_std 单线程所有权（v0.87.0 D5 惯例） |
//! | **D9** | 4 个 pub 计数器 `published_count`/`ack_count`/`reject_count`/`retry_count` + `pending: Vec<Strategy>` 待补发队列与 `republish_pending() -> u32`（补发成功数） |

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::channel::{CloudChannel, CloudError};
use crate::strategy::{EdgeAck, Strategy, DEFAULT_MAX_RETRIES};

/// 策略发布器（D9：字段全 pub 可观测；超时重试 + 断网补发）.
pub struct StrategyPublisher {
    /// 云边通道（D8：Box 单线程所有权）.
    pub channel: Box<dyn CloudChannel>,
    /// 单次下发最大尝试次数（默认 [`DEFAULT_MAX_RETRIES`] = 3）.
    pub max_retries: u32,
    /// 成功下发计数（含补发成功）.
    pub published_count: u64,
    /// 重试计数（broadcast 每次失败 +1）.
    pub retry_count: u64,
    /// 累计接受的 Ack 数.
    pub ack_count: u64,
    /// 累计拒绝的 Ack 数.
    pub reject_count: u64,
    /// 待补发队列（§6.5：下发失败策略缓存，重连后补发）.
    pub pending: Vec<Strategy>,
}

impl StrategyPublisher {
    /// 创建发布器（`max_retries` = [`DEFAULT_MAX_RETRIES`]，计数器全零，pending 空）.
    pub fn new(channel: Box<dyn CloudChannel>) -> Self {
        Self {
            channel,
            max_retries: DEFAULT_MAX_RETRIES,
            published_count: 0,
            retry_count: 0,
            ack_count: 0,
            reject_count: 0,
            pending: Vec::new(),
        }
    }

    /// 下发策略：至多 `max_retries` 次尝试（每次失败 `retry_count += 1`）；
    /// 成功 → `published_count += 1` 返回 `Ok`；耗尽 → 策略克隆入 `pending`
    /// （断网补发，§6.5）返回 `Err(BroadcastFailed)`。`max_retries == 0` 时
    /// 零次尝试直接入 pending（防御）.
    pub fn publish(&mut self, strategy: &Strategy) -> Result<(), CloudError> {
        for _ in 0..self.max_retries {
            match self.channel.broadcast(strategy) {
                Ok(()) => {
                    self.published_count += 1;
                    return Ok(());
                }
                Err(_) => {
                    self.retry_count += 1;
                }
            }
        }
        self.pending.push(strategy.clone());
        Err(CloudError::BroadcastFailed)
    }

    /// 重发 pending 队列（每条仍限 `max_retries` 次）；返回成功补发数，
    /// 成功者从 pending 移除，失败者保留在队列中.
    ///
    /// 实现取 pending 快照逐条内联重试（不复用 [`Self::publish`]，避免失败
    /// 策略被重复 push 到 pending 尾部造成重复）.
    pub fn republish_pending(&mut self) -> u32 {
        let items = core::mem::take(&mut self.pending);
        let mut ok_count = 0u32;
        for s in items {
            let mut ok = false;
            for _ in 0..self.max_retries {
                match self.channel.broadcast(&s) {
                    Ok(()) => {
                        self.published_count += 1;
                        ok = true;
                        break;
                    }
                    Err(_) => {
                        self.retry_count += 1;
                    }
                }
            }
            if ok {
                ok_count += 1;
            } else {
                self.pending.push(s);
            }
        }
        ok_count
    }

    /// 收集指定策略的边缘 Ack：委托 channel，accepted/rejected 分别累加
    /// 到 `ack_count` / `reject_count`（§9 可观测，D9）.
    pub fn collect_acks(&mut self, strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck> {
        let acks = self.channel.collect_acks(strategy_id, timeout_ms);
        for a in &acks {
            if a.accepted {
                self.ack_count += 1;
            } else {
                self.reject_count += 1;
            }
        }
        acks
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;
    use std::cell::Cell;
    use std::rc::Rc;

    use eneros_coordinator::Priority;
    use eneros_energy_market_agent::{DrSignal, Objective};

    use super::*;
    use crate::channel::MockCloudChannel;
    use crate::strategy::{
        validate_strategy, LocalState, RejectReason, StrategyContent, DEFAULT_ACK_TIMEOUT_MS,
    };

    /// 辅助：构造含 Safety 权重的合法策略.
    fn strategy(id: u64) -> Strategy {
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Safety, 0.8);
        Strategy {
            strategy_id: id,
            version: 1,
            targets: vec![10, 20],
            content: StrategyContent::OptimizationWeights(weights),
            deadline: 60_000,
            priority: Priority::Normal,
        }
    }

    /// 辅助：构造 DR 策略.
    fn dr_strategy(id: u64, target_mw: f32) -> Strategy {
        Strategy {
            strategy_id: id,
            version: 1,
            targets: vec![10],
            content: StrategyContent::DrResponse(DrSignal {
                event_id: id,
                target_mw,
                start: 1_000,
                end: 2_000,
                reward: 5.0,
            }),
            deadline: 60_000,
            priority: Priority::High,
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
                Some(RejectReason::SafetyWeightTooLow)
            },
        }
    }

    #[test]
    fn t23_new_default_values() {
        let p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        assert_eq!(p.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(p.published_count, 0);
        assert_eq!(p.retry_count, 0);
        assert_eq!(p.ack_count, 0);
        assert_eq!(p.reject_count, 0);
        assert!(p.pending.is_empty());
    }

    #[test]
    fn t24_publish_first_success() {
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        assert_eq!(p.publish(&strategy(1)), Ok(()));
        assert_eq!(p.published_count, 1);
        assert_eq!(p.retry_count, 0);
        assert!(p.pending.is_empty());
    }

    #[test]
    fn t25_publish_retry_then_success() {
        // 前 1 次失败 → 第 2 次成功：retry_count=1，published_count=1，pending 空.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(1)));
        assert_eq!(p.publish(&strategy(1)), Ok(()));
        assert_eq!(p.published_count, 1);
        assert_eq!(p.retry_count, 1);
        assert!(p.pending.is_empty());
    }

    #[test]
    fn t26_publish_exhausted_into_pending() {
        // 恒失败（fail_times 远大于 max_retries）：retry_count=3，策略入 pending.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(100)));
        let s = strategy(1);
        assert_eq!(p.publish(&s), Err(CloudError::BroadcastFailed));
        assert_eq!(p.retry_count, 3);
        assert_eq!(p.published_count, 0);
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.pending[0], s);
    }

    #[test]
    fn t27_publish_zero_retries_into_pending() {
        // max_retries=0 防御：零次尝试直接入 pending.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        p.max_retries = 0;
        assert_eq!(p.publish(&strategy(1)), Err(CloudError::BroadcastFailed));
        assert_eq!(p.retry_count, 0);
        assert_eq!(p.published_count, 0);
        assert_eq!(p.pending.len(), 1);
    }

    #[test]
    fn t28_republish_success_clears_pending() {
        // 断网 publish 失败入 pending → 恢复 → republish 成功清空.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(3)));
        assert_eq!(p.publish(&strategy(1)), Err(CloudError::BroadcastFailed));
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.retry_count, 3);
        // 网络恢复（故障注入已耗尽）.
        assert_eq!(p.republish_pending(), 1);
        assert!(p.pending.is_empty());
        assert_eq!(p.published_count, 1);
        assert_eq!(p.retry_count, 3);
    }

    #[test]
    fn t29_republish_partial_failure_keeps() {
        // max_retries=1：s1 补发失败保留，s2 补发成功移除.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        p.max_retries = 1;
        p.pending.push(strategy(1));
        p.pending.push(strategy(2));
        // 恢复 Mock 故障：仅第 1 次 broadcast 失败.
        p.channel = Box::new(MockCloudChannel::with_fail_times(1));
        assert_eq!(p.republish_pending(), 1);
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.pending[0].strategy_id, 1);
        assert_eq!(p.published_count, 1);
        assert_eq!(p.retry_count, 1);
    }

    #[test]
    fn t30_republish_per_item_retry_limit() {
        // 每条仍限 max_retries：2 条恒失败 → retry_count 累加 2*3=6.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(100)));
        let s1 = strategy(1);
        let s2 = strategy(2);
        assert_eq!(p.publish(&s1), Err(CloudError::BroadcastFailed));
        assert_eq!(p.publish(&s2), Err(CloudError::BroadcastFailed));
        assert_eq!(p.retry_count, 6);
        assert_eq!(p.pending.len(), 2);
        // 仍恒失败 → 返回 0，两条保留，retry_count 再 +6.
        assert_eq!(p.republish_pending(), 0);
        assert_eq!(p.pending.len(), 2);
        assert_eq!(p.retry_count, 12);
        assert_eq!(p.published_count, 0);
    }

    #[test]
    fn t31_republish_empty_pending_zero() {
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        assert_eq!(p.republish_pending(), 0);
        assert!(p.pending.is_empty());
        assert_eq!(p.published_count, 0);
        assert_eq!(p.retry_count, 0);
    }

    #[test]
    fn t32_collect_acks_counts_accepted_rejected() {
        // 预置 3 条 ack（2 accepted / 1 rejected）→ ack_count=2，reject_count=1.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_acks(vec![
            ack(1, 10, true),
            ack(1, 11, true),
            ack(1, 12, false),
        ])));
        let got = p.collect_acks(1, DEFAULT_ACK_TIMEOUT_MS);
        assert_eq!(got.len(), 3);
        assert_eq!(p.ack_count, 2);
        assert_eq!(p.reject_count, 1);
    }

    #[test]
    fn t33_collect_acks_empty_no_count() {
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::new()));
        assert!(p.collect_acks(1, 10_000).is_empty());
        assert_eq!(p.ack_count, 0);
        assert_eq!(p.reject_count, 0);
    }

    /// 辅助：记录 timeout_ms 透传的通道桩.
    struct TimeoutRecorder {
        last: Rc<Cell<Option<u64>>>,
    }

    impl CloudChannel for TimeoutRecorder {
        fn broadcast(&mut self, _strategy: &Strategy) -> Result<(), CloudError> {
            Ok(())
        }

        fn collect_acks(&mut self, _strategy_id: u64, timeout_ms: u64) -> Vec<EdgeAck> {
            self.last.set(Some(timeout_ms));
            Vec::new()
        }
    }

    #[test]
    fn t34_collect_acks_forwards_timeout() {
        let last = Rc::new(Cell::new(None));
        let mut p = StrategyPublisher::new(Box::new(TimeoutRecorder { last: last.clone() }));
        assert!(p.collect_acks(7, 12_345).is_empty());
        assert_eq!(last.get(), Some(12_345));
    }

    #[test]
    fn t35_offline_republish_integration() {
        // 断网补发集成：publish 失败入 pending → 恢复 → republish → collect_acks.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(3)));
        let s = strategy(1);
        // 断网：3 次重试耗尽入 pending.
        assert_eq!(p.publish(&s), Err(CloudError::BroadcastFailed));
        assert_eq!(p.pending.len(), 1);
        // 恢复（注入耗尽）+ 预置 ack：republish 成功，ack 收集 2 accepted.
        p.channel = Box::new(MockCloudChannel {
            fail_times: 0,
            sent: Vec::new(),
            acks: vec![ack(1, 10, true), ack(1, 11, true)],
        });
        assert_eq!(p.republish_pending(), 1);
        assert!(p.pending.is_empty());
        assert_eq!(p.published_count, 1);
        let got = p.collect_acks(1, DEFAULT_ACK_TIMEOUT_MS);
        assert_eq!(got.len(), 2);
        assert_eq!(p.ack_count, 2);
        assert_eq!(p.reject_count, 0);
    }

    #[test]
    fn t36_multi_strategy_publish_counts() {
        // 多策略连续 publish 计数：s1 耗尽 3 次入 pending，s2/s3 首次成功，
        // 随后 republish 清空 pending（Mock 前 N 次失败为全局连续语义）.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(3)));
        assert_eq!(p.publish(&strategy(1)), Err(CloudError::BroadcastFailed)); // 3 attempts
        assert_eq!(p.publish(&strategy(2)), Ok(())); // 1 attempt
        assert_eq!(p.publish(&strategy(3)), Ok(())); // 1 attempt
        assert_eq!(p.published_count, 2);
        assert_eq!(p.retry_count, 3);
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.pending[0].strategy_id, 1);
        // 故障注入已耗尽：republish 成功，计数继续累加.
        assert_eq!(p.republish_pending(), 1);
        assert!(p.pending.is_empty());
        assert_eq!(p.published_count, 3);
        assert_eq!(p.retry_count, 3);
    }

    #[test]
    fn t37_full_pipeline_validate_publish_collect() {
        // 全链路：validate_strategy 通过 → publish → collect_acks.
        let s = strategy(1);
        let local = LocalState {
            edge_id: 10,
            max_capacity_mw: 20.0,
        };
        assert_eq!(validate_strategy(&s, &local), Ok(()));
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_acks(vec![ack(
            1, 10, true,
        )])));
        assert_eq!(p.publish(&s), Ok(()));
        assert_eq!(p.published_count, 1);
        let got = p.collect_acks(1, DEFAULT_ACK_TIMEOUT_MS);
        assert_eq!(got.len(), 1);
        assert!(got[0].accepted);
        assert_eq!(p.ack_count, 1);
    }

    #[test]
    fn t38_nan_storm_defense() {
        // NaN 风暴：weight NaN 拒绝、DR target NaN 拒绝、capacity NaN/0 拒绝.
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Safety, f32::NAN);
        let nan_weight = Strategy {
            content: StrategyContent::OptimizationWeights(weights),
            ..strategy(9)
        };
        let local = LocalState {
            edge_id: 10,
            max_capacity_mw: 20.0,
        };
        assert_eq!(
            validate_strategy(&nan_weight, &local),
            Err(RejectReason::SafetyWeightTooLow)
        );
        assert_eq!(
            validate_strategy(&dr_strategy(10, f32::NAN), &local),
            Err(RejectReason::ExceedsCapacity)
        );
        let nan_cap = LocalState {
            edge_id: 10,
            max_capacity_mw: f32::NAN,
        };
        assert_eq!(
            validate_strategy(&dr_strategy(10, 1.0), &nan_cap),
            Err(RejectReason::ExceedsCapacity)
        );
        let zero_cap = LocalState {
            edge_id: 10,
            max_capacity_mw: 0.0,
        };
        assert_eq!(
            validate_strategy(&dr_strategy(10, 1.0), &zero_cap),
            Err(RejectReason::ExceedsCapacity)
        );
    }

    #[test]
    fn t39_validate_reject_reason_semantics() {
        // 拒绝原因与 RejectReason 精确匹配（机读审计，D7）.
        let local = LocalState {
            edge_id: 10,
            max_capacity_mw: 10.0,
        };
        // safety 不足 → SafetyWeightTooLow（非 ExceedsCapacity）.
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Safety, 0.1);
        let s = Strategy {
            content: StrategyContent::OptimizationWeights(weights),
            ..strategy(11)
        };
        let err = validate_strategy(&s, &local).unwrap_err();
        assert_eq!(err, RejectReason::SafetyWeightTooLow);
        assert_ne!(err, RejectReason::ExceedsCapacity);
        // DR 超容量 → ExceedsCapacity（非 SafetyWeightTooLow）.
        let err2 = validate_strategy(&dr_strategy(12, 15.0), &local).unwrap_err();
        assert_eq!(err2, RejectReason::ExceedsCapacity);
        assert_ne!(err2, RejectReason::SafetyWeightTooLow);
    }

    #[test]
    fn t40_pending_strategy_full_clone() {
        // pending 中策略为完整深克隆：字段逐一相等，且修改原值不影响 pending.
        let mut p = StrategyPublisher::new(Box::new(MockCloudChannel::with_fail_times(10)));
        let mut s = strategy(77);
        s.targets.push(99);
        assert_eq!(p.publish(&s), Err(CloudError::BroadcastFailed));
        assert_eq!(p.pending.len(), 1);
        let inq = &p.pending[0];
        assert_eq!(inq.strategy_id, 77);
        assert_eq!(inq.version, 1);
        assert_eq!(inq.targets, vec![10, 20, 99]);
        assert_eq!(inq.content, s.content);
        assert_eq!(inq.deadline, 60_000);
        assert_eq!(inq.priority, Priority::Normal);
        // 修改原策略不影响 pending 内克隆.
        s.targets.clear();
        assert_eq!(p.pending[0].targets, vec![10, 20, 99]);
    }
}
