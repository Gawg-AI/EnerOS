//! 滑动窗口异常检测（v0.118.0）.
//!
//! [`StatAnomalyDetector`] 以定容滑动窗口（64 条，满则淘汰最旧）缓存近期
//! 审计事件，对每条新事件执行三项检查：
//!
//! 1. **请求频率**：窗口内最近 60 秒事件数与 [`BehaviorDim::RequestRate`]
//!    基线比对——正偏离分级产生 [`AnomalyType::RateSpike`]，负偏离产生
//!    [`AnomalyType::RateDrop`]（仅当窗口时间跨度已覆盖完整一分钟后判定，
//!    避免冷启动窗口未满时误报骤降）。
//! 2. **非工作时段**：事件小时（`(ts / 3_600_000) % 24`，UTC 口径）
//!    < 6 或 > 22 且事件类型为控制命令 → Low 级 [`AnomalyType::OffHours`]。
//! 3. **越权访问**（偏差 D3 最小规则）：特权事件类型（ControlCommand /
//!    ConfigChange / KeyUse）的 target 在窗口其余事件（排除自身）中从未
//!    出现 → Low 级 [`AnomalyType::ScopeExpansion`]。
//!
//! 偏离分级由 [`StatAnomalyDetector::classify_deviation`] 完成：
//! `|dev| > sigma_high·stddev` → High，`> sigma_low·stddev` → Low；
//! `stddev < 0.001` 时退化为均值 50% 偏离 → Medium（蓝图 §4.5 既定逻辑）。

use alloc::vec::Vec;
use core::fmt::Write;

use crate::baseline::{
    Alert, AlertLevel, AnomalyType, BehaviorBaseline, BehaviorDim, StatBaseline,
};
use crate::events::{AuditEvent, AuditEventType};
use crate::AnomalyError;

/// 一分钟的毫秒数（速率统计与 RateDrop 判定的时间跨度阈值）.
const MINUTE_MS: u64 = 60_000;

/// 一小时的毫秒数（非工作时段小时计算）.
const HOUR_MS: u64 = 3_600_000;

/// 滑动窗口定容上限（与 [`ThresholdConfig::window_size`] 默认值一致）.
const WINDOW_CAP: usize = 64;

/// 退化分支的标准差阈值：低于此值视为「零方差基线」.
const STDDEV_EPS: f64 = 0.001;

/// 检测阈值配置（σ 分级阈值 + 训练/窗口参数）.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThresholdConfig {
    /// 低危告警 σ 阈值：偏离超过 `sigma_low` 倍标准差 → Low.
    pub sigma_low: f64,
    /// 高危告警 σ 阈值：偏离超过 `sigma_high` 倍标准差 → High.
    pub sigma_high: f64,
    /// 基线训练所需的最少样本数（与 `baseline::MIN_TRAIN_SAMPLES` 对齐）.
    pub min_samples: u32,
    /// 滑动窗口容量（受 [`WINDOW_CAP`] 定容上限钳制）.
    pub window_size: usize,
}

impl Default for ThresholdConfig {
    /// 蓝图 §4.3 默认阈值：2σ/4σ 分级，100 样本，64 条窗口.
    fn default() -> Self {
        ThresholdConfig {
            sigma_low: 2.0,
            sigma_high: 4.0,
            min_samples: 100,
            window_size: WINDOW_CAP,
        }
    }
}

/// 异常检测器抽象（实时单条 + 批量检测）.
pub trait AnomalyDetector {
    /// 对单条事件执行全部检测规则，返回本次产生的告警（可为空）.
    ///
    /// 基线未加载时返回 [`AnomalyError::BaselineNotTrained`].
    fn check(&mut self, event: &AuditEvent) -> Result<Vec<Alert>, AnomalyError>;
    /// 逐条处理事件流并汇总全部告警；任一事件检测出错即返回该错误.
    fn check_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<Alert>, AnomalyError>;
}

/// 统计异常检测器（滑动窗口 + 基线偏离）.
#[derive(Debug, Clone)]
pub struct StatAnomalyDetector {
    /// 已加载的行为基线；`None` 时 `check` 返回
    /// [`AnomalyError::BaselineNotTrained`].
    baseline: Option<BehaviorBaseline>,
    /// 事件滑动窗口（定容 64，满则 `remove(0)` 淘汰最旧）.
    window: heapless::Vec<AuditEvent, WINDOW_CAP>,
    /// 告警序号自增计数（从 1 开始，与审计 seq 语义对齐）.
    alert_seq: u64,
    /// 检测阈值配置.
    thresholds: ThresholdConfig,
}

impl StatAnomalyDetector {
    /// 构造空检测器（无基线、空窗口、告警序号从 1 起）.
    pub fn new(cfg: ThresholdConfig) -> Self {
        StatAnomalyDetector {
            baseline: None,
            window: heapless::Vec::new(),
            alert_seq: 1,
            thresholds: cfg,
        }
    }

    /// 加载行为基线（覆盖旧基线，供基线重训后热更新）.
    pub fn load_baseline(&mut self, bl: BehaviorBaseline) {
        self.baseline = Some(bl);
    }

    /// 窗口内最近 60 秒（`(now_ms - 60_000, now_ms]`）的事件数.
    pub fn compute_rate(&self, now_ms: u64) -> f64 {
        self.window
            .iter()
            .filter(|ev| now_ms >= ev.timestamp_ms && now_ms - ev.timestamp_ms <= MINUTE_MS)
            .count() as f64
    }

    /// 按行为维度查找统计基线.
    pub fn find_baseline(&self, dim: BehaviorDim) -> Option<&StatBaseline> {
        self.baseline
            .as_ref()
            .and_then(|b| b.baselines.iter().find(|(d, _)| *d == dim))
            .map(|(_, st)| st)
    }

    /// 偏离分级：`|dev| > sigma_high·stddev` → High，`> sigma_low·stddev` → Low；
    /// `stddev < 0.001` 时退化为均值 50% 偏离 → Medium（蓝图 §4.5 既定逻辑）.
    ///
    /// 返回 `(级别, 偏离倍数)`：正常分支为标准差倍数（σ），退化分支为
    /// 「均值 50%」单位的倍数（供告警 detail 展示）；未越限返回 `None`。
    pub fn classify_deviation(
        value: f64,
        bl: &StatBaseline,
        cfg: &ThresholdConfig,
    ) -> Option<(AlertLevel, f64)> {
        let dev = (value - bl.mean).abs();
        if bl.stddev < STDDEV_EPS {
            // 零方差退化：偏离超过均值绝对值的 50% 视为异常（蓝图 §4.5）
            let unit = (bl.mean.abs() * 0.5).max(1e-9);
            if dev > bl.mean.abs() * 0.5 {
                return Some((AlertLevel::Medium, dev / unit));
            }
            return None;
        }
        let sigma = dev / bl.stddev;
        if dev > cfg.sigma_high * bl.stddev {
            Some((AlertLevel::High, sigma))
        } else if dev > cfg.sigma_low * bl.stddev {
            Some((AlertLevel::Low, sigma))
        } else {
            None
        }
    }

    /// 组装告警：时间戳取触发事件的 `timestamp_ms`（偏差 D1，no_std 无系统
    /// 时钟），id 自增，`agent_id` 取 `actor` 数值解析（失败兜底 0，不 panic），
    /// `evidence_seq` 指向触发事件序号，`detail` 为中文描述 + 数值.
    fn make_alert(
        &mut self,
        event: &AuditEvent,
        anomaly: AnomalyType,
        level: AlertLevel,
        context: &str,
    ) -> Alert {
        let mut detail = heapless::String::<256>::new();
        // 定容 256 且 context 均短于此限，写入必然成功；超长时忽略尾部
        let _ = detail.push_str(context);
        let id = self.alert_seq;
        self.alert_seq = self.alert_seq.saturating_add(1);
        Alert {
            id,
            timestamp_ms: event.timestamp_ms,
            agent_id: event.actor.as_str().parse::<u32>().unwrap_or(0),
            anomaly,
            level,
            detail,
            evidence_seq: event.seq,
        }
    }

    /// 事件入窗口：满（达到 `window_size` 与定容上限的较小值）则淘汰最旧.
    fn push_window(&mut self, event: &AuditEvent) {
        let cap = self.thresholds.window_size.min(WINDOW_CAP);
        if !self.window.is_empty() && self.window.len() >= cap {
            let _ = self.window.remove(0);
        }
        // 淘汰后 len < 64，push 必然成功；定容满写忽略以避免 unwrap
        let _ = self.window.push(event.clone());
    }

    /// 窗口时间跨度（末事件 - 首事件时间戳，单调外乱序时饱和为 0）.
    fn window_span_ms(&self) -> u64 {
        match (self.window.first(), self.window.last()) {
            (Some(first), Some(last)) => last.timestamp_ms.saturating_sub(first.timestamp_ms),
            _ => 0,
        }
    }

    /// 检查①请求频率：正偏离 → RateSpike；负偏离 → RateDrop（仅当窗口
    /// 时间跨度 ≥ 60s 后判定，避免冷启动窗口未满时把低速率误报为骤降）.
    fn check_rate(&mut self, event: &AuditEvent, alerts: &mut Vec<Alert>) {
        let Some(bl) = self.find_baseline(BehaviorDim::RequestRate).cloned() else {
            return;
        };
        let rate = self.compute_rate(event.timestamp_ms);
        let Some((level, sigma)) = Self::classify_deviation(rate, &bl, &self.thresholds) else {
            return;
        };
        let positive = rate >= bl.mean;
        if !positive && self.window_span_ms() < MINUTE_MS {
            // RateDrop 冷启动抑制：窗口尚未积累满一分钟数据，速率天然偏低
            return;
        }
        let (anomaly, kind) = if positive {
            (AnomalyType::RateSpike, "频率突增")
        } else {
            (AnomalyType::RateDrop, "频率骤降")
        };
        let mut ctx = heapless::String::<256>::new();
        let _ = write!(
            ctx,
            "{kind}：当前速率 {rate:.1} 次/分，基线 {:.1}±{:.1}，偏离 {sigma:.1}σ",
            bl.mean, bl.stddev
        );
        let alert = self.make_alert(event, anomaly, level, ctx.as_str());
        alerts.push(alert);
    }

    /// 检查②非工作时段：事件小时（UTC）< 6 或 > 22 且为控制命令
    /// → Low 级 OffHours.
    fn check_off_hours(&mut self, event: &AuditEvent, alerts: &mut Vec<Alert>) {
        let hour = (event.timestamp_ms / HOUR_MS) % 24;
        if (6..=22).contains(&hour) {
            return;
        }
        if event.event_type != AuditEventType::ControlCommand {
            return;
        }
        let mut ctx = heapless::String::<256>::new();
        let _ = write!(ctx, "非工作时段控制命令：{hour} 时（允许时段 6~22 时）");
        let alert = self.make_alert(event, AnomalyType::OffHours, AlertLevel::Low, ctx.as_str());
        alerts.push(alert);
    }

    /// 检查③越权访问（偏差 D3 最小规则）：特权事件类型的 target 在窗口
    /// 其余事件（排除事件自身）中从未出现 → Low 级 ScopeExpansion.
    fn check_scope(&mut self, event: &AuditEvent, alerts: &mut Vec<Alert>) {
        let privileged = matches!(
            event.event_type,
            AuditEventType::ControlCommand | AuditEventType::ConfigChange | AuditEventType::KeyUse
        );
        if !privileged {
            return;
        }
        // 事件已入窗口且位于末尾，排除自身后检索历史 target
        let known = self
            .window
            .iter()
            .take(self.window.len().saturating_sub(1))
            .any(|ev| ev.target == event.target);
        if known {
            return;
        }
        let mut ctx = heapless::String::<256>::new();
        let _ = write!(ctx, "越权访问迹象：特权操作作用于陌生目标 {}", event.target);
        let alert = self.make_alert(
            event,
            AnomalyType::ScopeExpansion,
            AlertLevel::Low,
            ctx.as_str(),
        );
        alerts.push(alert);
    }
}

impl AnomalyDetector for StatAnomalyDetector {
    /// 单条检测流程：基线检查 → 事件入窗口 → 频率/非工作时段/越权三项检查.
    fn check(&mut self, event: &AuditEvent) -> Result<Vec<Alert>, AnomalyError> {
        if self.baseline.is_none() {
            return Err(AnomalyError::BaselineNotTrained);
        }
        self.push_window(event);
        let mut alerts = Vec::new();
        self.check_rate(event, &mut alerts);
        self.check_off_hours(event, &mut alerts);
        self.check_scope(event, &mut alerts);
        Ok(alerts)
    }

    /// 批量检测：逐条 `check` 并汇总告警；任一事件出错即终止并返回该错误.
    fn check_batch(&mut self, events: &[AuditEvent]) -> Result<Vec<Alert>, AnomalyError> {
        let mut all = Vec::new();
        for ev in events {
            let alerts = self.check(ev)?;
            all.extend(alerts);
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造指定时间戳/序号的测试事件.
    fn make_event(ty: AuditEventType, actor: &str, target: &str, ts: u64, seq: u64) -> AuditEvent {
        let mut ev = AuditEvent::new(ty, actor, target, "test");
        ev.timestamp_ms = ts;
        ev.seq = seq;
        ev
    }

    /// 手工组装 RequestRate 基线（绕过训练，便于精确控制 σ 倍数）.
    fn manual_baseline(mean: f64, stddev: f64) -> BehaviorBaseline {
        let mut baselines: heapless::Vec<(BehaviorDim, StatBaseline), 8> = heapless::Vec::new();
        let _ = baselines.push((
            BehaviorDim::RequestRate,
            StatBaseline {
                mean,
                stddev,
                min: mean - 3.0 * stddev,
                max: mean + 3.0 * stddev,
                p95: mean + 2.0 * stddev,
                sample_count: 120,
            },
        ));
        BehaviorBaseline {
            agent_id: 42,
            baselines,
            trained_at: 12 * HOUR_MS,
            window_hours: 24,
        }
    }

    /// 构造已加载 mean/stddev 基线的检测器.
    fn detector_with_baseline(mean: f64, stddev: f64) -> StatAnomalyDetector {
        let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
        det.load_baseline(manual_baseline(mean, stddev));
        det
    }

    /// 提取 StatBaseline 供 classify_deviation 直测.
    fn stat_bl(mean: f64, stddev: f64) -> StatBaseline {
        StatBaseline {
            mean,
            stddev,
            min: mean - 3.0 * stddev,
            max: mean + 3.0 * stddev,
            p95: mean + 2.0 * stddev,
            sample_count: 120,
        }
    }

    /// DET8：基线未加载时 check 返回 BaselineNotTrained.
    #[test]
    fn det8_no_baseline_returns_not_trained() {
        let mut det = StatAnomalyDetector::new(ThresholdConfig::default());
        let ev = make_event(AuditEventType::AuthLogin, "42", "meter-1", 12 * HOUR_MS, 1);
        let err = det.check(&ev).expect_err("无基线应报错");
        assert_eq!(err, AnomalyError::BaselineNotTrained);
        // check_batch 同样传播错误
        let err = det.check_batch(&[ev]).expect_err("无基线批量应报错");
        assert_eq!(err, AnomalyError::BaselineNotTrained);
    }

    /// DET9：±1σ 波动无告警（分级函数直测 + 稳态事件流不误报）.
    #[test]
    fn det9_within_1sigma_no_alert() {
        let cfg = ThresholdConfig::default();
        let bl = stat_bl(10.0, 2.0);
        // ±1σ 内取值：8.0 / 10.0 / 12.0 均不应分级
        assert_eq!(
            StatAnomalyDetector::classify_deviation(8.0, &bl, &cfg),
            None
        );
        assert_eq!(
            StatAnomalyDetector::classify_deviation(10.0, &bl, &cfg),
            None
        );
        assert_eq!(
            StatAnomalyDetector::classify_deviation(12.0, &bl, &cfg),
            None
        );

        // 稳态事件流：5s 一条（12/分，+1σ），白天时段、非特权类型
        let mut det = detector_with_baseline(10.0, 2.0);
        let base = 12 * HOUR_MS;
        for k in 0..24u64 {
            let ev = make_event(
                AuditEventType::AuthLogin,
                "42",
                "meter-1",
                base + k * 5_000,
                100 + k,
            );
            let alerts = det.check(&ev).expect("已加载基线应检测成功");
            assert!(alerts.is_empty(), "±1σ 波动不应告警，k={k}");
        }
    }

    /// DET10：2.5σ 偏离分级为 Low，返回倍数 2.5.
    #[test]
    fn det10_2_5_sigma_low_alert() {
        let cfg = ThresholdConfig::default();
        let bl = stat_bl(10.0, 2.0);
        // mean + 2.5σ = 15.0
        let (level, mult) =
            StatAnomalyDetector::classify_deviation(15.0, &bl, &cfg).expect("2.5σ 偏离应分级");
        assert_eq!(level, AlertLevel::Low);
        assert!((mult - 2.5).abs() < 1e-9);
        // 负向对称：mean - 2.5σ = 5.0 同为 Low
        let (level, _) =
            StatAnomalyDetector::classify_deviation(5.0, &bl, &cfg).expect("负向 2.5σ 偏离应分级");
        assert_eq!(level, AlertLevel::Low);
    }

    /// DET11：4.5σ 偏离分级为 High，返回倍数 4.5.
    #[test]
    fn det11_4_5_sigma_high_alert() {
        let cfg = ThresholdConfig::default();
        let bl = stat_bl(10.0, 2.0);
        // mean + 4.5σ = 19.0
        let (level, mult) =
            StatAnomalyDetector::classify_deviation(19.0, &bl, &cfg).expect("4.5σ 偏离应分级");
        assert_eq!(level, AlertLevel::High);
        assert!((mult - 4.5).abs() < 1e-9);
    }

    /// DET12：stddev≈0 退化为均值 50% 偏离 → Medium；未达 50% 不告警.
    #[test]
    fn det12_zero_stddev_fallback_medium() {
        let cfg = ThresholdConfig::default();
        let bl = stat_bl(10.0, 0.0005);
        // 偏离 6.0 > 均值 50%（5.0）→ Medium
        let (level, _) =
            StatAnomalyDetector::classify_deviation(16.0, &bl, &cfg).expect("退化分支应分级");
        assert_eq!(level, AlertLevel::Medium);
        // 偏离 2.0 < 均值 50% → 不告警
        assert_eq!(
            StatAnomalyDetector::classify_deviation(12.0, &bl, &cfg),
            None
        );
    }

    /// DET13：突发流量产生 RateSpike，evidence_seq 指向触发事件，级别随
    /// σ 升高从 Low 升 High.
    #[test]
    fn det13_rate_spike_evidence_seq_points_to_trigger() {
        let mut det = detector_with_baseline(10.0, 2.0);
        let base = 12 * HOUR_MS; // 正午，避开非工作时段
        let mut all: Vec<Alert> = Vec::new();
        // 1s 一条注入 19 条：第 k 条时窗口速率 = k+1
        for k in 0..19u64 {
            let ev = make_event(
                AuditEventType::AuthLogin,
                "42",
                "meter-1",
                base + k * 1_000,
                100 + k,
            );
            let alerts = det.check(&ev).expect("已加载基线应检测成功");
            all.extend(alerts);
        }
        // 首条告警应在第 15 条事件（速率 15 → 2.5σ，Low）
        let first = all.first().expect("应产生 RateSpike 告警");
        assert_eq!(first.anomaly, AnomalyType::RateSpike);
        assert_eq!(first.level, AlertLevel::Low);
        assert_eq!(first.evidence_seq, 114);
        assert_eq!(first.timestamp_ms, base + 14_000);
        assert_eq!(first.agent_id, 42);
        assert_eq!(first.id, 1);
        // 速率 19 → 4.5σ → High，evidence_seq 指向末事件
        let high = all
            .iter()
            .find(|a| a.level == AlertLevel::High)
            .expect("应升级出 High 告警");
        assert_eq!(high.anomaly, AnomalyType::RateSpike);
        assert_eq!(high.evidence_seq, 118);
        assert_eq!(high.timestamp_ms, base + 18_000);
        // 告警 id 严格自增
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.id, i as u64 + 1);
        }
    }

    /// DET14：凌晨 3 点 ControlCommand 触发 Low 级 OffHours；上午 10 点不触发.
    #[test]
    fn det14_off_hours_control_command() {
        // 凌晨 3 点：触发 OffHours（单事件窗口跨度 < 60s，RateDrop 被抑制）
        let mut det = detector_with_baseline(10.0, 2.0);
        let night = make_event(
            AuditEventType::ControlCommand,
            "42",
            "breaker-1",
            3 * HOUR_MS + 1_000,
            200,
        );
        let alerts = det.check(&night).expect("检测应成功");
        let off = alerts
            .iter()
            .find(|a| a.anomaly == AnomalyType::OffHours)
            .expect("凌晨 3 点控制命令应触发 OffHours");
        assert_eq!(off.level, AlertLevel::Low);
        assert_eq!(off.evidence_seq, 200);
        // 同事件因陌生 target 也触发 ScopeExpansion（D3 最小规则），但不应有 RateDrop
        assert!(alerts
            .iter()
            .any(|a| a.anomaly == AnomalyType::ScopeExpansion));
        assert!(!alerts.iter().any(|a| a.anomaly == AnomalyType::RateDrop));

        // 上午 10 点：不触发 OffHours
        let mut det = detector_with_baseline(10.0, 2.0);
        let day = make_event(
            AuditEventType::ControlCommand,
            "42",
            "breaker-1",
            10 * HOUR_MS + 1_000,
            201,
        );
        let alerts = det.check(&day).expect("检测应成功");
        assert!(!alerts.iter().any(|a| a.anomaly == AnomalyType::OffHours));
    }
}
