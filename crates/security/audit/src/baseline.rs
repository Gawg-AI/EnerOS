//! 行为基线建模与统计计算（v0.118.0）.
//!
//! 以审计事件流为数据源，按分钟分桶统计事件速率形成速率序列，经
//! [`compute_stats`] 计算均值/总体标准差/min/max/p95，组装
//! [`BehaviorBaseline`] 作为后续异常检测的比对照。
//! 时间语义：不依赖系统时钟（no_std），`trained_at` 取训练集末事件的
//! `timestamp_ms`（证据时间，对齐 v0.117.0 外部时间先例）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::events::AuditEvent;
use crate::AnomalyError;

/// 训练所需的最少事件样本数（低于此数拒绝训练）.
pub const MIN_TRAIN_SAMPLES: u32 = 100;

/// 默认训练窗口（小时），用于基线有效期判定.
pub const DEFAULT_WINDOW_HOURS: u16 = 24;

/// 一分钟的毫秒数（速率分桶粒度）.
const MINUTE_MS: u64 = 60_000;

/// 行为维度（参与基线统计的指标类别）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorDim {
    /// 请求频率（次/分钟）.
    RequestRate,
    /// 控制指令频率.
    ControlFrequency,
    /// 资源使用量.
    ResourceUsage,
    /// 访问范围.
    AccessScope,
    /// 错误率.
    ErrorRate,
}

/// 统计基线（单一维度的一组统计量）.
#[derive(Debug, Clone)]
pub struct StatBaseline {
    /// 均值.
    pub mean: f64,
    /// 总体标准差（除以 n，非样本标准差 n-1）.
    pub stddev: f64,
    /// 最小值.
    pub min: f64,
    /// 最大值.
    pub max: f64,
    /// 95 百分位（nearest-rank 口径，见 [`compute_stats`]）.
    pub p95: f64,
    /// 样本数.
    pub sample_count: u64,
}

/// Agent 行为基线（各维度统计基线的集合）.
#[derive(Debug, Clone)]
pub struct BehaviorBaseline {
    /// 基线所属的 Agent 标识.
    pub agent_id: u32,
    /// 各维度统计基线（定容 8，超出维度忽略）.
    pub baselines: heapless::Vec<(BehaviorDim, StatBaseline), 8>,
    /// 训练完成时间戳（取训练集末事件的 `timestamp_ms`）.
    pub trained_at: u64,
    /// 训练窗口（小时），用于基线有效期判定.
    pub window_hours: u16,
}

/// 异常类型.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnomalyType {
    /// 频率突增.
    RateSpike,
    /// 频率骤降.
    RateDrop,
    /// 访问范围扩大（越权访问迹象）.
    ScopeExpansion,
    /// 错误爆发.
    ErrorBurst,
    /// 非工作时段操作.
    OffHours,
    /// 异常目标.
    UnusualTarget,
}

/// 告警级别（数值越大越严重，可排序比较）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    /// 提示.
    Info = 0,
    /// 低危.
    Low = 1,
    /// 中危.
    Medium = 2,
    /// 高危.
    High = 3,
    /// 严重.
    Critical = 4,
}

/// 安全告警（`evidence_seq` 指向触发告警的审计日志序号，可回溯取证）.
#[derive(Debug, Clone)]
pub struct Alert {
    /// 告警序号（检测器内自增）.
    pub id: u64,
    /// 告警时间戳（毫秒，取触发事件的 `timestamp_ms`）.
    pub timestamp_ms: u64,
    /// 涉事 Agent 标识.
    pub agent_id: u32,
    /// 异常类型.
    pub anomaly: AnomalyType,
    /// 告警级别.
    pub level: AlertLevel,
    /// 告警详情（≤ 256 字节）.
    pub detail: heapless::String<256>,
    /// 关联审计日志序列号（证据指针）.
    pub evidence_seq: u64,
}

/// 基线训练器抽象（批量训练 + 增量更新）.
pub trait BaselineTrainer {
    /// 以历史事件批量训练行为基线；样本不足返回
    /// [`AnomalyError::InsufficientSamples`].
    fn train(&mut self, events: &[AuditEvent]) -> Result<BehaviorBaseline, AnomalyError>;
    /// 增量追加一条事件样本.
    fn update(&mut self, event: &AuditEvent);
}

/// 基线训练器实现（按分钟分桶的真实速率统计）.
#[derive(Debug, Clone)]
pub struct BaselineTrainerImpl {
    /// 增量样本（维度, 采样值）；`update` 每条事件记一次请求.
    samples: Vec<(BehaviorDim, f64)>,
    /// 目标 Agent 标识.
    agent_id: u32,
}

impl BaselineTrainerImpl {
    /// 构造针对 `agent_id` 的训练器（初始无样本）.
    pub fn new(agent_id: u32) -> Self {
        BaselineTrainerImpl {
            samples: Vec::new(),
            agent_id,
        }
    }

    /// 当前已累积的增量样本数.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

impl BaselineTrainer for BaselineTrainerImpl {
    /// 训练流程：样本数检查 → 事件按分钟分桶（`ts / 60_000`）→ 每分钟
    /// 事件数形成速率序列 → [`compute_stats`] → 组装 [`BehaviorBaseline`]
    /// （当前产出 `RequestRate` 单维度基线，其余维度预留扩展）.
    fn train(&mut self, events: &[AuditEvent]) -> Result<BehaviorBaseline, AnomalyError> {
        if events.len() < MIN_TRAIN_SAMPLES as usize {
            return Err(AnomalyError::InsufficientSamples {
                min: MIN_TRAIN_SAMPLES,
                got: events.len() as u32,
            });
        }
        // 按分钟分桶统计每分钟事件数；BTreeMap 保证桶键有序、重复桶合并
        let mut buckets: BTreeMap<u64, u64> = BTreeMap::new();
        for ev in events {
            let minute = ev.timestamp_ms / MINUTE_MS;
            *buckets.entry(minute).or_insert(0) += 1;
        }
        let rates: Vec<f64> = buckets.values().map(|&c| c as f64).collect();
        let stats = compute_stats(&rates);

        let mut baselines: heapless::Vec<(BehaviorDim, StatBaseline), 8> = heapless::Vec::new();
        // 容量 8 且当前仅 1 个维度，push 必然成功；忽略返回值避免 unwrap
        let _ = baselines.push((BehaviorDim::RequestRate, stats));
        // 训练时间戳取末事件时间（样本数 ≥ 100，末事件必然存在）
        let trained_at = events.last().map_or(0, |ev| ev.timestamp_ms);
        Ok(BehaviorBaseline {
            agent_id: self.agent_id,
            baselines,
            trained_at,
            window_hours: DEFAULT_WINDOW_HOURS,
        })
    }

    /// 增量更新：每条事件记为一次请求采样（RequestRate 维度）.
    fn update(&mut self, _event: &AuditEvent) {
        self.samples.push((BehaviorDim::RequestRate, 1.0));
    }
}

/// 牛顿迭代平方根（no_std 无 `f64::sqrt`；收敛阈值 1e-12，统计场景足够）.
///
/// `x <= 0` 或 NaN 时返回 0.0（方差理论上非负，防御性处理）。
fn sqrt_newton(x: f64) -> f64 {
    // 偏序比较语义显式化：仅当 x 明确 > 0 时迭代，否则（<=0 或 NaN）返回 0
    match x.partial_cmp(&0.0) {
        Some(core::cmp::Ordering::Greater) => {}
        _ => return 0.0,
    }
    let mut g = if x >= 1.0 { x } else { 1.0 };
    for _ in 0..64 {
        let next = 0.5 * (g + x / g);
        if (next - g).abs() < 1e-12 * g {
            g = next;
            break;
        }
        g = next;
    }
    g
}

/// 统计计算：均值 / 总体标准差（除以 n）/ min / max / p95.
///
/// p95 采用 nearest-rank 口径：升序排序后取
/// `idx = ceil(0.95 * n) - 1` 处的值（n ≥ 1 时 idx ∈ [0, n-1]）。
/// 空切片返回全零基线（`sample_count == 0`）。
pub fn compute_stats(values: &[f64]) -> StatBaseline {
    let n = values.len();
    if n == 0 {
        return StatBaseline {
            mean: 0.0,
            stddev: 0.0,
            min: 0.0,
            max: 0.0,
            p95: 0.0,
            sample_count: 0,
        };
    }
    let n_f = n as f64;
    let mean = values.iter().sum::<f64>() / n_f;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n_f;
    let stddev = sqrt_newton(var);

    let mut sorted: Vec<f64> = values.to_vec();
    // f64 无 Ord：partial_cmp 失败（NaN）时退化为相等，禁 unwrap
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    // 非空切片排序后首尾必然存在，用索引前先取拷贝避免 unwrap
    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    // nearest-rank：idx = ceil(0.95*n)-1 = ceil(19*n/20)-1
    // （整数 div_ceil 替代浮点 ceil，no_std 无 `f64::ceil`），钳制在 [0, n-1] 内
    let p95_idx = (19 * n).div_ceil(20).saturating_sub(1).min(n - 1);
    let p95 = sorted.get(p95_idx).copied().unwrap_or(0.0);

    StatBaseline {
        mean,
        stddev,
        min,
        max,
        p95,
        sample_count: n as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AuditEventType;

    /// 构造指定时间戳的测试事件.
    fn make_event(ts: u64) -> AuditEvent {
        let mut ev = AuditEvent::new(AuditEventType::AuthLogin, "agent-7", "meter-1", "read");
        ev.timestamp_ms = ts;
        ev
    }

    /// BSL1：已知序列的统计量与手算值一致.
    ///
    /// 序列 [2,4,4,4,5,5,7,9]：mean=5，总体方差=(9+1+1+1+0+0+4+16)/8=4，
    /// stddev=2，min=2，max=9；p95：ceil(0.95*8)-1=7 → 升序第 8 个值 9。
    #[test]
    fn bsl1_compute_stats_matches_hand_calculation() {
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let st = compute_stats(&values);
        assert!((st.mean - 5.0).abs() < 1e-9);
        assert!((st.stddev - 2.0).abs() < 1e-9);
        assert!((st.min - 2.0).abs() < 1e-9);
        assert!((st.max - 9.0).abs() < 1e-9);
        assert!((st.p95 - 9.0).abs() < 1e-9);
        assert_eq!(st.sample_count, 8);

        // 空切片返回全零
        let empty = compute_stats(&[]);
        assert_eq!(empty.sample_count, 0);
        assert!((empty.mean - 0.0).abs() < 1e-9);
    }

    /// BSL2：样本不足（50 条）拒绝训练并报 InsufficientSamples{min:100, got:50}.
    #[test]
    fn bsl2_insufficient_samples_rejected() {
        let events: Vec<AuditEvent> = (0..50).map(|i| make_event(i * 60_000)).collect();
        let mut trainer = BaselineTrainerImpl::new(7);
        let err = trainer.train(&events).expect_err("应拒绝训练");
        assert_eq!(err, AnomalyError::InsufficientSamples { min: 100, got: 50 });
    }

    /// BSL3：合成 120 条事件（60 分钟 × 2 条/分钟）训练产出 RequestRate
    /// 基线，mean=2（>0）、sample_count=60、trained_at 取末事件时间戳.
    #[test]
    fn bsl3_train_produces_request_rate_baseline() {
        // 每分钟恰好 2 条，共 60 分钟
        let events: Vec<AuditEvent> = (0..120u64)
            .map(|i| make_event((i / 2) * 60_000 + (i % 2) * 1_000))
            .collect();
        let last_ts = events.last().map_or(0, |ev| ev.timestamp_ms);
        let mut trainer = BaselineTrainerImpl::new(7);
        let bl = trainer.train(&events).expect("样本充足应训练成功");
        assert_eq!(bl.agent_id, 7);
        assert_eq!(bl.window_hours, 24);
        assert_eq!(bl.trained_at, last_ts);
        let (dim, st) = bl
            .baselines
            .iter()
            .find(|(d, _)| *d == BehaviorDim::RequestRate)
            .expect("应含 RequestRate 基线");
        assert_eq!(*dim, BehaviorDim::RequestRate);
        assert!(st.mean > 0.0);
        assert!((st.mean - 2.0).abs() < 1e-9);
        assert_eq!(st.sample_count, 60);
    }

    /// BSL4：`update` 后增量样本计数 +1.
    #[test]
    fn bsl4_update_appends_one_sample() {
        let mut trainer = BaselineTrainerImpl::new(7);
        assert_eq!(trainer.sample_count(), 0);
        trainer.update(&make_event(1_000));
        assert_eq!(trainer.sample_count(), 1);
        trainer.update(&make_event(2_000));
        assert_eq!(trainer.sample_count(), 2);
    }
}
