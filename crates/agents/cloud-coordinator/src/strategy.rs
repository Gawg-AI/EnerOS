//! EnerOS v0.95.0 Cloud Coordinator 策略数据结构与边缘安全校验.
//!
//! 云端全局策略（优化权重/电价预测/DR 响应/模型更新）的数据模型 +
//! 边缘侧 [`validate_strategy`] 安全校验（蓝图 §4.5 关键代码落地）：
//! **策略非强制、边缘主权保留**——safety 权重低于门限或 DR 目标超出本地容量时，
//! 边缘确定性拒绝（宁拒勿放，D10/D12 NaN 防御）。
//!
//! # 偏差声明
//! | 偏差 | 说明 |
//! |------|------|
//! | **D2** | `strategy_id` / `targets` / `edge_id` 全部 `u64` / `Vec<u64>` — 无堆字符串 + 确定性（v0.87.0 D3 / v0.94.0 D2 惯例） |
//! | **D3** | `DEFAULT_ACK_TIMEOUT_MS = 10_000` — 语义等价蓝图 `Duration::from_secs(10)`，u64 ms 参数注入 |
//! | **D4** | `OptimizationWeights(BTreeMap<Objective, f32>)` — no_std alloc 无 HashMap；BTreeMap 确定性迭代可重放 |
//! | **D5** | `priority` 复用 `eneros-coordinator::Priority`（v0.92.0，派生 Ord 序即优先级序，不重复定义） |
//! | **D6** | `ModelRef` / `LocalState` MVP 最小定义（蓝图未定义） |
//! | **D7** | `EdgeAck.reason: Option<RejectReason>` — 结构化无堆字符串，机读审计（与蓝图关键代码一致） |
//! | **D10** | 蓝图硬编码 `0.5` → 命名常量 [`SAFETY_WEIGHT_MIN`]；safety weight 缺失/NaN 按 0.0 → 拒绝（安全侧默认拒绝） |
//! | **D12** | NaN 防御：weight 非有限 → 0.0（触发安全拒绝）；DR `target_mw` 非有限 → `ExceedsCapacity`；`max_capacity_mw` 非有限或 ≤0 → 一切 DR 策略拒绝 |

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_coordinator::Priority;
use eneros_energy_market_agent::{DrSignal, Objective, PricePoint};

/// Safety 权重下限（D10：蓝图硬编码 0.5 → 命名常量；低于此值边缘拒绝）.
pub const SAFETY_WEIGHT_MIN: f32 = 0.5;

/// 默认 Ack 收集超时（u64 ms，D3：语义等价蓝图 `Duration::from_secs(10)`）.
pub const DEFAULT_ACK_TIMEOUT_MS: u64 = 10_000;

/// 默认下发最大重试次数（§4.4 下发超时重试）.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// 云端下发策略（D2：标识全部 u64；含 f32/Vec/BTreeMap，不可 derive Eq/Copy）.
#[derive(Debug, Clone, PartialEq)]
pub struct Strategy {
    /// 策略 ID.
    pub strategy_id: u64,
    /// 策略版本（§5.2/§8.4 多版本兼容）.
    pub version: u32,
    /// 目标 EdgeBox ID 列表.
    pub targets: Vec<u64>,
    /// 策略内容（4 变体）.
    pub content: StrategyContent,
    /// 截止时间（u64 ms）.
    pub deadline: u64,
    /// 优先级（复用 v0.92.0 [`Priority`]，D5）.
    pub priority: Priority,
}

/// 策略内容（D4：BTreeMap 替代蓝图 HashMap；§9 可扩展——新增策略类型加变体）.
#[derive(Debug, Clone, PartialEq)]
pub enum StrategyContent {
    /// 优化权重下发（Objective → 权重）.
    OptimizationWeights(BTreeMap<Objective, f32>),
    /// 电价预测序列.
    PriceForecast(Vec<PricePoint>),
    /// 需求响应信号.
    DrResponse(DrSignal),
    /// 模型更新引用.
    ModelUpdate(ModelRef),
}

/// 模型引用（D6：蓝图未定义 → MVP 最小定义）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelRef {
    /// 模型 ID.
    pub model_id: u64,
    /// 模型版本.
    pub version: u32,
}

/// 边缘 Ack（D7：`reason` 为结构化 [`RejectReason`]，机读审计）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeAck {
    /// 对应策略 ID.
    pub strategy_id: u64,
    /// 应答边缘 ID.
    pub edge_id: u64,
    /// 是否接受（false 时 reason 携带拒绝原因）.
    pub accepted: bool,
    /// 拒绝原因（接受时为 None）.
    pub reason: Option<RejectReason>,
}

/// 边缘拒绝原因（与蓝图 §4.5 关键代码一致）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Safety 权重低于 [`SAFETY_WEIGHT_MIN`]（含缺失/NaN 按 0.0，D10/D12）.
    SafetyWeightTooLow,
    /// DR 目标功率超出本地容量（含 target 非有限、容量非有限或 ≤0，D12）.
    ExceedsCapacity,
}

/// 边缘本地状态（D6：蓝图未定义 → MVP 最小定义）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LocalState {
    /// 边缘 ID.
    pub edge_id: u64,
    /// 本地最大可调容量（MW；非有限或 ≤0 → 一切 DR 策略拒绝，D12）.
    pub max_capacity_mw: f32,
}

/// 边缘安全校验（蓝图 §4.5 落地；D10/D12 安全侧默认拒绝，宁拒勿放）.
///
/// - `OptimizationWeights`：safety weight（缺失/非有限按 0.0）< [`SAFETY_WEIGHT_MIN`]
///   → `Err(SafetyWeightTooLow)`；
/// - `DrResponse`：`max_capacity_mw` 非有限或 ≤0 → `Err(ExceedsCapacity)`；
///   `target_mw` 非有限或 `abs() > max_capacity_mw` → `Err(ExceedsCapacity)`；
/// - `PriceForecast` / `ModelUpdate`：恒 `Ok(())`。
pub fn validate_strategy(
    strategy: &Strategy,
    local_state: &LocalState,
) -> Result<(), RejectReason> {
    match &strategy.content {
        StrategyContent::OptimizationWeights(weights) => {
            let safety = weights.get(&Objective::Safety).copied().unwrap_or(0.0);
            // D12：非有限按 0.0 → 触发安全拒绝.
            let safety = if safety.is_finite() { safety } else { 0.0 };
            if safety < SAFETY_WEIGHT_MIN {
                return Err(RejectReason::SafetyWeightTooLow);
            }
        }
        StrategyContent::DrResponse(dr) => {
            // D12：容量非法 → 一切 DR 策略拒绝（安全侧）.
            if !local_state.max_capacity_mw.is_finite() || local_state.max_capacity_mw <= 0.0 {
                return Err(RejectReason::ExceedsCapacity);
            }
            if !dr.target_mw.is_finite() || dr.target_mw.abs() > local_state.max_capacity_mw {
                return Err(RejectReason::ExceedsCapacity);
            }
        }
        StrategyContent::PriceForecast(_) | StrategyContent::ModelUpdate(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_energy_market_agent::Period;

    use super::*;

    /// 辅助：构造含 Safety 权重的策略.
    fn weights_strategy(safety: f32) -> Strategy {
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Economy, 0.3);
        weights.insert(Objective::Safety, safety);
        Strategy {
            strategy_id: 1,
            version: 1,
            targets: vec![10, 20],
            content: StrategyContent::OptimizationWeights(weights),
            deadline: 60_000,
            priority: Priority::Normal,
        }
    }

    /// 辅助：构造 DR 策略.
    fn dr_strategy(target_mw: f32) -> Strategy {
        Strategy {
            strategy_id: 2,
            version: 1,
            targets: vec![10],
            content: StrategyContent::DrResponse(DrSignal {
                event_id: 100,
                target_mw,
                start: 1_000,
                end: 2_000,
                reward: 5.0,
            }),
            deadline: 60_000,
            priority: Priority::High,
        }
    }

    /// 辅助：本地状态（capacity MW）.
    fn local(capacity: f32) -> LocalState {
        LocalState {
            edge_id: 10,
            max_capacity_mw: capacity,
        }
    }

    // ===== T1~T6：数据结构派生语义 =====

    #[test]
    fn t01_strategy_construct_clone_eq() {
        let s = weights_strategy(0.6);
        let c = s.clone();
        assert_eq!(s, c);
        assert_eq!(s.strategy_id, 1);
        assert_eq!(s.version, 1);
        assert_eq!(s.targets, vec![10, 20]);
        assert_eq!(s.deadline, 60_000);
        assert_eq!(s.priority, Priority::Normal);
        // 修改克隆不影响原值（深克隆）.
        let mut c2 = s.clone();
        c2.targets.push(30);
        assert_ne!(s, c2);
    }

    #[test]
    fn t02_strategy_content_variants_eq() {
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Safety, 0.8);
        let w = StrategyContent::OptimizationWeights(weights);
        assert_eq!(w, w.clone());

        let p = StrategyContent::PriceForecast(vec![PricePoint {
            time: 0,
            price: 0.5,
            period: Period::Peak,
        }]);
        assert_eq!(p, p.clone());

        let d = StrategyContent::DrResponse(DrSignal {
            event_id: 1,
            target_mw: 5.0,
            start: 0,
            end: 1,
            reward: 0.0,
        });
        assert_eq!(d, d.clone());

        let m = StrategyContent::ModelUpdate(ModelRef {
            model_id: 7,
            version: 3,
        });
        assert_eq!(m, m.clone());
        // 不同变体不相等.
        assert_ne!(w, p);
        assert_ne!(d, m);
    }

    #[test]
    fn t03_model_ref_semantics() {
        let m = ModelRef {
            model_id: 42,
            version: 2,
        };
        let copied = m;
        assert_eq!(m, copied); // Copy 语义.
        assert_eq!(m.model_id, 42);
        assert_eq!(m.version, 2);
        let d = ModelRef::default();
        assert_eq!(d.model_id, 0);
        assert_eq!(d.version, 0);
        assert_ne!(m, d);
    }

    #[test]
    fn t04_edge_ack_semantics() {
        let ack = EdgeAck {
            strategy_id: 1,
            edge_id: 10,
            accepted: false,
            reason: Some(RejectReason::SafetyWeightTooLow),
        };
        let copied = ack; // Copy 语义.
        assert_eq!(ack, copied);
        assert!(!ack.accepted);
        assert_eq!(ack.reason, Some(RejectReason::SafetyWeightTooLow));
        let ok = EdgeAck {
            reason: None,
            accepted: true,
            ..ack
        };
        assert_ne!(ack, ok);
        assert!(ok.accepted);
    }

    #[test]
    fn t05_reject_reason_eq() {
        assert_eq!(
            RejectReason::SafetyWeightTooLow,
            RejectReason::SafetyWeightTooLow
        );
        assert_eq!(RejectReason::ExceedsCapacity, RejectReason::ExceedsCapacity);
        assert_ne!(
            RejectReason::SafetyWeightTooLow,
            RejectReason::ExceedsCapacity
        );
    }

    #[test]
    fn t06_local_state_default_and_construct() {
        let d = LocalState::default();
        assert_eq!(d.edge_id, 0);
        assert_eq!(d.max_capacity_mw, 0.0);
        let l = local(20.0);
        assert_eq!(l.edge_id, 10);
        assert_eq!(l.max_capacity_mw, 20.0);
        let copied = l;
        assert_eq!(l, copied);
    }

    // ===== T7~T14：validate_strategy 安全校验 =====

    #[test]
    fn t07_validate_safety_pass() {
        // safety 0.6 ≥ 0.5 → 通过.
        let s = weights_strategy(0.6);
        assert_eq!(validate_strategy(&s, &local(10.0)), Ok(()));
    }

    #[test]
    fn t08_validate_safety_too_low_reject() {
        // safety 0.4 < 0.5 → 拒绝.
        let s = weights_strategy(0.4);
        assert_eq!(
            validate_strategy(&s, &local(10.0)),
            Err(RejectReason::SafetyWeightTooLow)
        );
    }

    #[test]
    fn t09_validate_safety_missing_reject() {
        // Safety 缺失 → 按 0.0 → 拒绝（D10 宁拒勿放）.
        let mut weights = BTreeMap::new();
        weights.insert(Objective::Economy, 1.0);
        let s = Strategy {
            content: StrategyContent::OptimizationWeights(weights),
            ..weights_strategy(0.6)
        };
        assert_eq!(
            validate_strategy(&s, &local(10.0)),
            Err(RejectReason::SafetyWeightTooLow)
        );
    }

    #[test]
    fn t10_validate_safety_nan_reject() {
        // Safety NaN → 按 0.0 → 拒绝（D12）.
        let s = weights_strategy(f32::NAN);
        assert_eq!(
            validate_strategy(&s, &local(10.0)),
            Err(RejectReason::SafetyWeightTooLow)
        );
    }

    #[test]
    fn t11_validate_dr_pass() {
        // target 15 ≤ capacity 20 → 通过；负值按 abs 判定同样通过.
        assert_eq!(validate_strategy(&dr_strategy(15.0), &local(20.0)), Ok(()));
        assert_eq!(validate_strategy(&dr_strategy(-15.0), &local(20.0)), Ok(()));
    }

    #[test]
    fn t12_validate_dr_exceeds_capacity_reject() {
        // target 15 > capacity 10 → 拒绝.
        assert_eq!(
            validate_strategy(&dr_strategy(15.0), &local(10.0)),
            Err(RejectReason::ExceedsCapacity)
        );
        // 负向超额同样拒绝（abs 判定）.
        assert_eq!(
            validate_strategy(&dr_strategy(-15.0), &local(10.0)),
            Err(RejectReason::ExceedsCapacity)
        );
    }

    #[test]
    fn t13_validate_dr_target_nan_reject() {
        // target NaN → 拒绝（D12）.
        assert_eq!(
            validate_strategy(&dr_strategy(f32::NAN), &local(20.0)),
            Err(RejectReason::ExceedsCapacity)
        );
    }

    #[test]
    fn t14_validate_capacity_nonpositive_and_other_variants_ok() {
        // capacity ≤ 0 → 一切 DR 策略拒绝（D12 安全侧）.
        assert_eq!(
            validate_strategy(&dr_strategy(0.0), &local(0.0)),
            Err(RejectReason::ExceedsCapacity)
        );
        assert_eq!(
            validate_strategy(&dr_strategy(1.0), &local(-5.0)),
            Err(RejectReason::ExceedsCapacity)
        );
        // PriceForecast / ModelUpdate 恒 Ok.
        let pf = Strategy {
            strategy_id: 3,
            version: 1,
            targets: Vec::new(),
            content: StrategyContent::PriceForecast(vec![PricePoint {
                time: 0,
                price: 0.5,
                period: Period::Valley,
            }]),
            deadline: 0,
            priority: Priority::Low,
        };
        assert_eq!(validate_strategy(&pf, &local(0.0)), Ok(()));
        let mu = Strategy {
            content: StrategyContent::ModelUpdate(ModelRef {
                model_id: 1,
                version: 1,
            }),
            ..pf.clone()
        };
        assert_eq!(validate_strategy(&mu, &local(0.0)), Ok(()));
    }
}
