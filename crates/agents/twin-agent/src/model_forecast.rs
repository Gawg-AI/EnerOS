//! 短期预测模型 — ForecastPoint / ForecastResult / ForecastError / ForecastModel + 基线模型（v0.90.0）.
//!
//! - **D2**：`target: &'static str` 替代蓝图 `String`（避免堆分配，默认 `"power"`）。
//! - **D3**：`horizon_ms: u64` 替代 `Duration`（全 crate 统一 u64 ms 外部时间注入惯例）。
//! - **D4**：[`ForecastModel`] 不要求 Send + Sync（no_std 单线程惯例，同 agent-bus-dds D2）。
//! - **D8**：统一兜底链——主模型 `Err` → [`PersistenceModel`]；[`MeanModel`] 为可选主模型
//!   （无历史缓冲时均值 ≡ 持续法单样本，v0.89.0 镜像仅存最新态）。
//! - **D10**：[`compute_confidence`] = base_confidence × 区间紧度，确定性计算可复现；
//!   `degraded` 标记走兜底链或置信低于阈值。
//! - **D12**：NaN/Inf 防御（v0.88.0 C140 教训）——输入功率非有限 → [`sanitize`] 按 0.0 处理，
//!   不传播、不 panic。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_bus_dds::DdsError;
use serde::{Deserialize, Serialize};

use crate::model::TwinModel;

/// 单条预测点数上限（D9：§43.6 内存预算防 OOM）.
const MAX_POINTS: usize = 96;

/// 预测区间点（某一未来时刻的预测值与置信上下界）.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ForecastPoint {
    /// 预测时刻（ms，外部时间基准）.
    pub time: u64,
    /// 预测值.
    pub value: f32,
    /// 置信下界.
    pub lower: f32,
    /// 置信上界.
    pub upper: f32,
}

/// 预测结果（D2/D3/D10）.
#[derive(Debug, Clone, Serialize)]
pub struct ForecastResult {
    /// 预测目标（D2：&'static str，默认 `"power"`）.
    pub target: &'static str,
    /// 预测时域（ms，D3：回显传入的 horizon_ms）.
    pub horizon_ms: u64,
    /// 预测点序列（≤ 96 点，D9 点数钳制）.
    pub points: Vec<ForecastPoint>,
    /// 置信度 ∈ [0, 1]（D10：base_confidence × 区间紧度）.
    pub confidence: f32,
    /// 降级标记（走兜底链 或 confidence < threshold 时置位，D10）.
    pub degraded: bool,
}

impl ForecastResult {
    /// 序列化为 JSON（D11：全量含 points 数组，≤96 点约 4KB）.
    ///
    /// serde_json 对纯数据 DTO 序列化不会失败；失败时兜底返回 `"{}"`。
    pub fn to_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(s) => s,
            Err(_) => String::from("{}"),
        }
    }
}

/// 预测错误（DDS 透传，同 v0.89.0 TwinError 模式）.
#[derive(Debug)]
pub enum ForecastError {
    /// DDS 中间件错误.
    Dds(DdsError),
}

impl From<DdsError> for ForecastError {
    fn from(e: DdsError) -> Self {
        ForecastError::Dds(e)
    }
}

/// 预测模型抽象（D4：无 Send + Sync 约束）.
pub trait ForecastModel {
    /// 基于孪生模型预测未来 `horizon_ms` 内、步长 `step_ms` 的点序列.
    fn predict(
        &self,
        input: &TwinModel,
        horizon_ms: u64,
        step_ms: u64,
    ) -> Result<Vec<ForecastPoint>, ForecastError>;

    /// 模型名称（如 `"persistence"` / `"mean"`）.
    fn name(&self) -> &'static str;

    /// 基础置信度（与区间紧度相乘得最终置信度，D10）.
    fn base_confidence(&self) -> f32;
}

/// NaN/Inf 防御（D12）：非有限值按 0.0 处理，不传播.
pub fn sanitize(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 提取当前功率（crate 内辅助）.
///
/// - `grid.timestamp > 0` → 电网有功（sanitize 后），has_data = true；
/// - 否则 devices 非空 → 设备功率求和回退（逐设备 sanitize），has_data = true；
/// - 全空 → (0.0, false)。
fn current_power(model: &TwinModel) -> (f32, bool) {
    if model.grid.timestamp > 0 {
        (sanitize(model.grid.active_power), true)
    } else if !model.devices.is_empty() {
        let sum: f32 = model
            .devices
            .values()
            .map(|t| sanitize(t.state.power as f32))
            .sum();
        (sum, true)
    } else {
        (0.0, false)
    }
}

/// 恒定值预测辅助（D8：持续法/均值法共用逻辑，仅带宽不同）.
///
/// 点数 = `ceil(horizon_ms / step_ms)`（step_ms == 0 按 1 处理防除零），钳制 `1..=96`。
/// point i（0-based）：`time = input.last_update + (i+1) * step_ms`。
/// 有数据：value 恒定 = current_power，lower/upper = value ∓/± |value|*band；
/// 无数据：全零点（value == lower == upper == 0.0）。
fn constant_predict(
    input: &TwinModel,
    horizon_ms: u64,
    step_ms: u64,
    band: f32,
) -> Vec<ForecastPoint> {
    let step = if step_ms == 0 { 1 } else { step_ms };
    let n = (horizon_ms.saturating_add(step - 1) / step).clamp(1, MAX_POINTS as u64) as usize;
    let (power, has_data) = current_power(input);
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        let time = input
            .last_update
            .saturating_add((i as u64 + 1).saturating_mul(step));
        let (value, lower, upper) = if has_data {
            let w = power.abs() * band;
            (power, power - w, power + w)
        } else {
            (0.0, 0.0, 0.0)
        };
        points.push(ForecastPoint {
            time,
            value,
            lower,
            upper,
        });
    }
    points
}

/// 持续法基线模型（D8：兜底链末端）.
///
/// 无历史缓冲时的最简预测：未来值 ≡ 当前值，±5% 置信带。
pub struct PersistenceModel;

impl ForecastModel for PersistenceModel {
    fn predict(
        &self,
        input: &TwinModel,
        horizon_ms: u64,
        step_ms: u64,
    ) -> Result<Vec<ForecastPoint>, ForecastError> {
        Ok(constant_predict(input, horizon_ms, step_ms, 0.05))
    }

    fn name(&self) -> &'static str {
        "persistence"
    }

    fn base_confidence(&self) -> f32 {
        0.6
    }
}

/// 均值法模型（D8：可选主模型；无历史缓冲时均值 ≡ 持续法单样本）.
///
/// 预测值与持续法相同，±3% 置信带（更窄区间 → 更高置信度）。
pub struct MeanModel;

impl ForecastModel for MeanModel {
    fn predict(
        &self,
        input: &TwinModel,
        horizon_ms: u64,
        step_ms: u64,
    ) -> Result<Vec<ForecastPoint>, ForecastError> {
        Ok(constant_predict(input, horizon_ms, step_ms, 0.03))
    }

    fn name(&self) -> &'static str {
        "mean"
    }

    fn base_confidence(&self) -> f32 {
        0.7
    }
}

/// 置信度计算（D10/D12）.
///
/// - points 空 → 0.0；
/// - 任一点 value/lower/upper 非有限 → 0.0（D12 不传播）；
/// - 全部点 value == lower == upper == 0.0（无数据标记）→ 0.0；
/// - 否则 `base * (1.0 - mean_rel_width)`，mean_rel_width 为各点
///   `(upper - lower) / 2 / (|value| + 1e-6)` 的平均，结果钳制 [0, 1]。
pub fn compute_confidence(base: f32, points: &[ForecastPoint]) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    let mut sum_rel = 0.0f32;
    let mut all_zero = true;
    for p in points {
        if !p.value.is_finite() || !p.lower.is_finite() || !p.upper.is_finite() {
            return 0.0;
        }
        if p.value != 0.0 || p.lower != 0.0 || p.upper != 0.0 {
            all_zero = false;
        }
        sum_rel += (p.upper - p.lower) / 2.0 / (p.value.abs() + 1e-6);
    }
    if all_zero {
        return 0.0;
    }
    let mean_rel_width = sum_rel / points.len() as f32;
    (base * (1.0 - mean_rel_width)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceTwin;

    /// 便捷构造：grid 有值（active_power=12.3, timestamp=100, last_update=1000）.
    fn grid_model() -> TwinModel {
        let mut m = TwinModel::default();
        m.grid.active_power = 12.3;
        m.grid.timestamp = 100;
        m.last_update = 1000;
        m
    }

    /// 便捷构造：单个指定功率的设备孪生.
    fn device(id: u64, power: f64) -> DeviceTwin {
        let mut d = DeviceTwin {
            device_id: id,
            ..DeviceTwin::default()
        };
        d.state.power = power;
        d
    }

    // ===== T1: ForecastPoint default 全零 + Copy 语义 =====
    #[test]
    fn t1_forecast_point_default_and_copy() {
        let a = ForecastPoint::default();
        assert_eq!(a.time, 0);
        assert!(a.value.abs() < 1e-9);
        assert!(a.lower.abs() < 1e-9);
        assert!(a.upper.abs() < 1e-9);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    // ===== T2: ForecastResult 构造 + Clone =====
    #[test]
    fn t2_forecast_result_construct_and_clone() {
        let r = ForecastResult {
            target: "power",
            horizon_ms: 60000,
            points: vec![ForecastPoint::default(); 3],
            confidence: 0.57,
            degraded: false,
        };
        let r2 = r.clone();
        assert_eq!(r2.target, "power");
        assert_eq!(r2.horizon_ms, 60000);
        assert_eq!(r2.points.len(), 3);
        assert!(!r2.degraded);
    }

    // ===== T3: to_json 可解析 + 含 target/horizon_ms/confidence/degraded/points 字段 =====
    #[test]
    fn t3_forecast_result_to_json_parseable() {
        let r = ForecastResult {
            target: "power",
            horizon_ms: 60000,
            points: vec![ForecastPoint {
                time: 1000,
                value: 12.3,
                lower: 11.685,
                upper: 12.915,
            }],
            confidence: 0.57,
            degraded: false,
        };
        let s = r.to_json();
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["target"], "power");
        assert_eq!(v["horizon_ms"], 60000);
        assert!(v.get("confidence").is_some());
        assert_eq!(v["degraded"], false);
        assert!(v["points"].is_array());
    }

    // ===== T4: ForecastError From<DdsError> 转换 + Debug 含 "Dds" =====
    #[test]
    fn t4_forecast_error_from_dds() {
        let e: ForecastError = DdsError::Closed.into();
        assert!(matches!(e, ForecastError::Dds(DdsError::Closed)));
        let s = format!("{:?}", e);
        assert!(s.contains("Dds"));
    }

    // ===== T5: sanitize — NaN/±Inf → 0.0，正常值不变 =====
    #[test]
    fn t5_sanitize_non_finite() {
        assert!(sanitize(f32::NAN).abs() < 1e-9);
        assert!(sanitize(f32::INFINITY).abs() < 1e-9);
        assert!(sanitize(f32::NEG_INFINITY).abs() < 1e-9);
        assert!((sanitize(1.5) - 1.5).abs() < 1e-9);
    }

    // ===== T6: current_power — grid 优先 / 设备求和回退 / 全空 / NaN 防御 =====
    #[test]
    fn t6_current_power_priority_and_fallback() {
        // grid.timestamp > 0 优先.
        let mut m = TwinModel::default();
        m.grid.active_power = 12.3;
        m.grid.timestamp = 5;
        let (p, has) = current_power(&m);
        assert!((p - 12.3).abs() < 1e-6);
        assert!(has);

        // grid 空 + 2 设备 → 求和 4.0.
        let mut m = TwinModel::default();
        m.devices.insert(1, device(1, 1.5));
        m.devices.insert(2, device(2, 2.5));
        let (p, has) = current_power(&m);
        assert!((p - 4.0).abs() < 1e-6);
        assert!(has);

        // 全空 → (0.0, false).
        let m = TwinModel::default();
        let (p, has) = current_power(&m);
        assert!(p.abs() < 1e-9);
        assert!(!has);

        // NaN active_power → sanitize 为 0.0，仍 has_data = true.
        let mut m = TwinModel::default();
        m.grid.active_power = f32::NAN;
        m.grid.timestamp = 5;
        let (p, has) = current_power(&m);
        assert!(p.abs() < 1e-9);
        assert!(has);
    }

    // ===== T7: PersistenceModel 60 点 + time 逐点 == last_update + (i+1)*step =====
    #[test]
    fn t7_persistence_60_points_time_sequence() {
        let m = grid_model();
        let pts = PersistenceModel.predict(&m, 60000, 1000).expect("predict");
        assert_eq!(pts.len(), 60);
        for (i, p) in pts.iter().enumerate() {
            assert_eq!(p.time, 1000 + (i as u64 + 1) * 1000);
        }
    }

    // ===== T8: PersistenceModel value 恒定 12.3 + 每点 lower < value < upper（±5%）=====
    #[test]
    fn t8_persistence_constant_value_band() {
        let m = grid_model();
        let pts = PersistenceModel.predict(&m, 60000, 1000).expect("predict");
        for p in &pts {
            assert!((p.value - 12.3).abs() < 1e-6);
            assert!(p.lower < p.value);
            assert!(p.value < p.upper);
            assert!((p.lower - (12.3 - 12.3 * 0.05)).abs() < 1e-4);
            assert!((p.upper - (12.3 + 12.3 * 0.05)).abs() < 1e-4);
        }
    }

    // ===== T9: PersistenceModel — grid 空 + 2 设备 → value == 4.0 =====
    #[test]
    fn t9_persistence_device_sum_fallback() {
        let mut m = TwinModel::default();
        m.devices.insert(1, device(1, 1.5));
        m.devices.insert(2, device(2, 2.5));
        m.last_update = 100;
        let pts = PersistenceModel.predict(&m, 10000, 1000).expect("predict");
        assert!(pts.iter().all(|p| (p.value - 4.0).abs() < 1e-6));
    }

    // ===== T10: PersistenceModel — 全空模型 → 全零点 =====
    #[test]
    fn t10_persistence_empty_model_all_zero() {
        let m = TwinModel::default();
        let pts = PersistenceModel.predict(&m, 10000, 1000).expect("predict");
        assert!(!pts.is_empty());
        for p in &pts {
            assert!(p.value.abs() < 1e-9);
            assert!(p.lower.abs() < 1e-9);
            assert!(p.upper.abs() < 1e-9);
        }
    }

    // ===== T11: 点数计算 — 不足一步 → 1；horizon 0 → 1；超大 horizon → 96 钳制 =====
    #[test]
    fn t11_persistence_point_count_clamp() {
        let m = grid_model();
        assert_eq!(
            PersistenceModel.predict(&m, 60000, 1000).expect("p").len(),
            60
        );
        assert_eq!(PersistenceModel.predict(&m, 500, 1000).expect("p").len(), 1);
        assert_eq!(PersistenceModel.predict(&m, 0, 1000).expect("p").len(), 1);
        assert_eq!(
            PersistenceModel
                .predict(&m, 10_000_000, 1)
                .expect("p")
                .len(),
            96
        );
    }

    // ===== T12: 模型名称与基础置信度 =====
    #[test]
    fn t12_model_name_and_base_confidence() {
        assert_eq!(PersistenceModel.name(), "persistence");
        assert!((PersistenceModel.base_confidence() - 0.6).abs() < 1e-9);
        assert_eq!(MeanModel.name(), "mean");
        assert!((MeanModel.base_confidence() - 0.7).abs() < 1e-9);
        assert!(MeanModel.base_confidence() > PersistenceModel.base_confidence());
    }

    // ===== T13: MeanModel 有值场景 value 与 PersistenceModel 相同（D8 等价）=====
    #[test]
    fn t13_mean_equivalent_value_to_persistence() {
        let m = grid_model();
        let p = PersistenceModel.predict(&m, 10000, 1000).expect("p");
        let q = MeanModel.predict(&m, 10000, 1000).expect("m");
        assert_eq!(p.len(), q.len());
        for (a, b) in p.iter().zip(q.iter()) {
            assert!((a.value - b.value).abs() < 1e-9);
            assert_eq!(a.time, b.time);
        }
    }

    // ===== T14: MeanModel 区间更窄（±3% < ±5%）；全空 → 全 0 =====
    #[test]
    fn t14_mean_narrower_band_and_empty_zero() {
        let m = grid_model();
        let p = PersistenceModel.predict(&m, 10000, 1000).expect("p");
        let q = MeanModel.predict(&m, 10000, 1000).expect("m");
        assert!((q[0].upper - q[0].lower) < (p[0].upper - p[0].lower));

        let e = TwinModel::default();
        let q = MeanModel.predict(&e, 10000, 1000).expect("m");
        assert!(!q.is_empty());
        assert!(q
            .iter()
            .all(|pt| pt.value.abs() < 1e-9 && pt.lower.abs() < 1e-9 && pt.upper.abs() < 1e-9));
    }

    // ===== T15: compute_confidence 空 points → 0.0 =====
    #[test]
    fn t15_confidence_empty_points() {
        assert!(compute_confidence(0.6, &[]).abs() < 1e-9);
    }

    // ===== T16: compute_confidence 含 NaN 点 → 0.0（D12 不传播）=====
    #[test]
    fn t16_confidence_nan_point_zero() {
        let pts = [ForecastPoint {
            time: 1,
            value: f32::NAN,
            lower: 0.0,
            upper: 0.0,
        }];
        assert!(compute_confidence(0.6, &pts).abs() < 1e-9);
    }

    // ===== T17: compute_confidence 全零值点（无数据标记）→ 0.0 =====
    #[test]
    fn t17_confidence_all_zero_points() {
        let pts = [ForecastPoint::default(); 4];
        assert!(compute_confidence(0.6, &pts).abs() < 1e-9);
    }

    // ===== T18: compute_confidence 正常 ∈ (0, base]；窄区间 > 宽区间 =====
    #[test]
    fn t18_confidence_narrower_band_higher() {
        let narrow = [ForecastPoint {
            time: 1,
            value: 100.0,
            lower: 99.0,
            upper: 101.0,
        }];
        let wide = [ForecastPoint {
            time: 1,
            value: 100.0,
            lower: 50.0,
            upper: 150.0,
        }];
        let cn = compute_confidence(0.9, &narrow);
        let cw = compute_confidence(0.9, &wide);
        assert!(cn > 0.0 && cn <= 0.9);
        assert!(cn > cw);
    }

    // ===== T19: compute_confidence 极端大区间 → 结果恒 ∈ [0, 1] =====
    #[test]
    fn t19_confidence_extreme_band_clamped() {
        let pts = [ForecastPoint {
            time: 1,
            value: 1.0,
            lower: -1e6,
            upper: 1e6,
        }];
        let c = compute_confidence(0.6, &pts);
        assert!((0.0..=1.0).contains(&c));
    }

    // ===== T20: PersistenceModel 输入 NaN active_power → 点 value 全为 0.0（有限），不 panic =====
    #[test]
    fn t20_persistence_nan_input_safe() {
        let mut m = TwinModel::default();
        m.grid.active_power = f32::NAN;
        m.grid.timestamp = 7;
        m.last_update = 100;
        let pts = PersistenceModel.predict(&m, 60000, 1000).expect("predict");
        assert!(!pts.is_empty());
        for p in &pts {
            assert!(p.value.is_finite());
            assert!(p.value.abs() < 1e-9);
            assert!(p.lower.is_finite());
            assert!(p.upper.is_finite());
        }
    }
}
