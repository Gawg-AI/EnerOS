//! 预测器 — Predictor 主模型 + 兜底链 + 周期步长配置 + `/power/twin/forecast` 发布（v0.90.0）.
//!
//! - **D8**：统一兜底链——主模型 `predict` 返回 `Err` 时自动切换 [`PersistenceModel`]
//!   兜底并置 `degraded = true`，结果仍完整可用。
//! - **D9**：`step_ms` / `max_points` / `confidence_threshold` 构造注入；
//!   点数按 `max_points` truncate 防御（§43.6 内存预算防 OOM）。
//! - **D10**：confidence = 所用模型 base_confidence × 区间紧度（[`compute_confidence`]），
//!   确定性可复现；confidence < threshold 时亦置 `degraded = true`。
//! - **D11**：[`publish_forecast`] 向 `/power/twin/forecast` 写入全量 JSON 样本
//!   （`ForecastResult::to_json`，≤96 点约 4KB），write 失败透传 [`ForecastError::Dds`]。

use alloc::boxed::Box;

use eneros_agent_bus_dds::{DdsNode, WriterId};

use crate::model::TwinModel;
use crate::model_forecast::{
    compute_confidence, ForecastError, ForecastModel, ForecastResult, PersistenceModel,
};

/// 短期预测器（主模型 + 持续法兜底链）.
///
/// 字段全 pub，便于测试与外部观测（与项目既有 Agent 风格一致，无不变量需保护）。
pub struct Predictor {
    /// 主预测模型（D4：Box<dyn ForecastModel>，无 Send + Sync 约束）.
    pub model: Box<dyn ForecastModel>,
    /// 预测步长（ms，构造时 0 钳制为 1）.
    pub step_ms: u64,
    /// 单条预测最大点数（构造时 0 钳制为 1；默认 96，D9 防 OOM）.
    pub max_points: usize,
    /// 置信度阈值 ∈ [0, 1]（NaN 按 0.5 处理；低于阈值置 degraded，D10）.
    pub confidence_threshold: f32,
}

impl Predictor {
    /// 创建预测器：step_ms == 0 → 1；max_points == 0 → 1；threshold 钳制 [0, 1]（NaN → 0.5）.
    pub fn new(
        model: Box<dyn ForecastModel>,
        step_ms: u64,
        max_points: usize,
        confidence_threshold: f32,
    ) -> Self {
        let step_ms = if step_ms == 0 { 1 } else { step_ms };
        let max_points = if max_points == 0 { 1 } else { max_points };
        let confidence_threshold = if confidence_threshold.is_nan() {
            0.5
        } else {
            confidence_threshold.clamp(0.0, 1.0)
        };
        Self {
            model,
            step_ms,
            max_points,
            confidence_threshold,
        }
    }

    /// 对孪生模型做 `horizon_ms` 短期预测.
    ///
    /// 主模型成功：用其输出（按 max_points truncate 防御），degraded = false；
    /// 主模型 `Err`：切换 [`PersistenceModel`] 兜底（同样 truncate），degraded = true。
    /// confidence = [`compute_confidence`]（所用模型 base, points）；
    /// confidence < threshold → degraded = true（D10）。
    pub fn forecast(
        &self,
        twin: &TwinModel,
        horizon_ms: u64,
    ) -> Result<ForecastResult, ForecastError> {
        let (mut points, mut degraded, base) =
            match self.model.predict(twin, horizon_ms, self.step_ms) {
                Ok(p) => (p, false, self.model.base_confidence()),
                Err(_) => (
                    PersistenceModel.predict(twin, horizon_ms, self.step_ms)?,
                    true,
                    PersistenceModel.base_confidence(),
                ),
            };
        points.truncate(self.max_points);
        let confidence = compute_confidence(base, &points);
        if confidence < self.confidence_threshold {
            degraded = true;
        }
        Ok(ForecastResult {
            target: "power",
            horizon_ms,
            points,
            confidence,
            degraded,
        })
    }

    /// 预测并发布到 `/power/twin/forecast`（D11）.
    ///
    /// 预测成功但发布失败时返回 `Err(ForecastError::Dds(_))`，预测结果不返回。
    pub fn forecast_and_publish(
        &self,
        twin: &TwinModel,
        horizon_ms: u64,
        node: &mut dyn DdsNode,
        writer: WriterId,
    ) -> Result<ForecastResult, ForecastError> {
        let r = self.forecast(twin, horizon_ms)?;
        publish_forecast(node, writer, &r)?;
        Ok(r)
    }
}

/// 发布预测结果到 `/power/twin/forecast`（D11）.
///
/// serde_json 对纯数据 DTO 序列化不会失败；失败时兜底用 `to_json()` 的字节。
/// `node.write` 失败返回 `Err(ForecastError::Dds(_))`。
pub fn publish_forecast(
    node: &mut dyn DdsNode,
    writer: WriterId,
    result: &ForecastResult,
) -> Result<(), ForecastError> {
    let bytes = match serde_json::to_vec(result) {
        Ok(b) => b,
        Err(_) => result.to_json().into_bytes(),
    };
    node.write(writer, &bytes).map_err(ForecastError::Dds)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use eneros_agent_bus_dds::{DdsError, MockDdsNode, QosPolicy};

    use super::*;
    use crate::model::DeviceTwin;
    use crate::model_forecast::{ForecastPoint, MeanModel};

    /// 恒失败主模型（兜底链测试用）.
    struct FailingModel;

    impl ForecastModel for FailingModel {
        fn predict(
            &self,
            _input: &TwinModel,
            _horizon_ms: u64,
            _step_ms: u64,
        ) -> Result<Vec<ForecastPoint>, ForecastError> {
            Err(ForecastError::Dds(DdsError::Closed))
        }

        fn name(&self) -> &'static str {
            "failing"
        }

        fn base_confidence(&self) -> f32 {
            0.9
        }
    }

    /// 便捷构造：grid 有值（active_power=12.3, timestamp=100, last_update=1000）.
    fn grid_model() -> TwinModel {
        let mut m = TwinModel::default();
        m.grid.active_power = 12.3;
        m.grid.timestamp = 100;
        m.last_update = 1000;
        m
    }

    /// 便捷构造：Mock 节点 + participant + `/power/twin/forecast` writer 与观察 reader.
    fn make_bus() -> (MockDdsNode, WriterId, eneros_agent_bus_dds::ReaderId) {
        let mut node = MockDdsNode::new_default();
        let p = node.create_participant().expect("participant");
        let w = node
            .create_writer(p, "/power/twin/forecast", QosPolicy::default())
            .expect("writer");
        let r = node
            .create_reader(p, "/power/twin/forecast", QosPolicy::default())
            .expect("reader");
        (node, w, r)
    }

    // ===== T21: new 钳制 — step_ms 0→1 / max_points 0→1 / threshold 1.5→1.0 / -0.5→0.0 / NaN→0.5 =====
    #[test]
    fn t21_new_clamps() {
        let p = Predictor::new(Box::new(PersistenceModel), 0, 0, 1.5);
        assert_eq!(p.step_ms, 1);
        assert_eq!(p.max_points, 1);
        assert!((p.confidence_threshold - 1.0).abs() < 1e-9);
        let p2 = Predictor::new(Box::new(PersistenceModel), 10, 10, -0.5);
        assert!(p2.confidence_threshold.abs() < 1e-9);
        let p3 = Predictor::new(Box::new(PersistenceModel), 10, 10, f32::NAN);
        assert!((p3.confidence_threshold - 0.5).abs() < 1e-9);
    }

    // ===== T22: 主模型成功（PersistenceModel 为主）→ 60 点 value==12.3 + degraded==false =====
    #[test]
    fn t22_forecast_primary_success() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&grid_model(), 60000).expect("forecast");
        assert_eq!(r.points.len(), 60);
        assert!(r.points.iter().all(|pt| (pt.value - 12.3).abs() < 1e-6));
        assert!(!r.degraded);
        // confidence ≈ 0.6 * (1 - 0.05) = 0.57 ≥ 阈值 0.5.
        assert!((r.confidence - 0.57).abs() < 1e-3);
    }

    // ===== T23: 主模型失败（FailingModel）→ 兜底持续法 points + degraded==true =====
    #[test]
    fn t23_forecast_fallback_on_error() {
        let p = Predictor::new(Box::new(FailingModel), 1000, 96, 0.5);
        let r = p.forecast(&grid_model(), 60000).expect("forecast");
        assert!(r.degraded);
        let direct = PersistenceModel
            .predict(&grid_model(), 60000, 1000)
            .expect("persistence");
        assert_eq!(r.points, direct);
    }

    // ===== T24: 空 TwinModel → 全 0 点 + confidence==0.0 + degraded==true =====
    #[test]
    fn t24_forecast_empty_model() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&TwinModel::default(), 60000).expect("forecast");
        assert!(!r.points.is_empty());
        assert!(r
            .points
            .iter()
            .all(|pt| pt.value.abs() < 1e-9 && pt.lower.abs() < 1e-9 && pt.upper.abs() < 1e-9));
        assert!(r.confidence.abs() < 1e-9);
        assert!(r.degraded);
    }

    // ===== T25: horizon_ms=0 → 1 点；horizon 500/step 1000 → 1 点 =====
    #[test]
    fn t25_forecast_min_one_point() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let m = grid_model();
        assert_eq!(p.forecast(&m, 0).expect("f").points.len(), 1);
        assert_eq!(p.forecast(&m, 500).expect("f").points.len(), 1);
    }

    // ===== T26: max_points=10, horizon 60000/step 1000 → 恰好 10 点（truncate 生效）=====
    #[test]
    fn t26_forecast_max_points_truncate() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 10, 0.5);
        let r = p.forecast(&grid_model(), 60000).expect("forecast");
        assert_eq!(r.points.len(), 10);
    }

    // ===== T27: 确定性 — 同输入两次 forecast 逐点一致 + confidence 位级一致 =====
    #[test]
    fn t27_forecast_deterministic() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let m = grid_model();
        let a = p.forecast(&m, 60000).expect("a");
        let b = p.forecast(&m, 60000).expect("b");
        assert_eq!(a.points, b.points);
        assert_eq!(a.confidence.to_bits(), b.confidence.to_bits());
    }

    // ===== T28: threshold=0.99 → 持续法正常数据 degraded==true（0.57 < 0.99）=====
    #[test]
    fn t28_low_confidence_marks_degraded() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.99);
        let r = p.forecast(&grid_model(), 60000).expect("forecast");
        assert!(r.degraded);
    }

    // ===== T29: threshold=0.0 → 持续法正常数据 degraded==false =====
    #[test]
    fn t29_zero_threshold_not_degraded() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.0);
        let r = p.forecast(&grid_model(), 60000).expect("forecast");
        assert!(!r.degraded);
    }

    // ===== T30: target=="power" + horizon_ms 回显 == 传入值 =====
    #[test]
    fn t30_target_and_horizon_echo() {
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&grid_model(), 45000).expect("forecast");
        assert_eq!(r.target, "power");
        assert_eq!(r.horizon_ms, 45000);
    }

    // ===== T31: 主 MeanModel — confidence > 同场景 PersistenceModel 主（0.7 vs 0.6 base）=====
    #[test]
    fn t31_mean_primary_higher_confidence() {
        let m = grid_model();
        let pm = Predictor::new(Box::new(MeanModel), 1000, 96, 0.0);
        let pp = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.0);
        let rm = pm.forecast(&m, 60000).expect("mean");
        let rp = pp.forecast(&m, 60000).expect("persistence");
        assert!(rm.confidence > rp.confidence);
        assert!((rm.points[0].value - 12.3).abs() < 1e-6);
    }

    // ===== T32: FailingModel 兜底结果 target=="power" / horizon 正确 / points 非空 =====
    #[test]
    fn t32_fallback_result_complete() {
        let p = Predictor::new(Box::new(FailingModel), 1000, 96, 0.5);
        let r = p.forecast(&grid_model(), 30000).expect("forecast");
        assert_eq!(r.target, "power");
        assert_eq!(r.horizon_ms, 30000);
        assert!(!r.points.is_empty());
    }

    // ===== T33: publish_forecast MockDdsNode 成功 — 外部 reader take 到 1 条样本 =====
    #[test]
    fn t33_publish_forecast_one_sample() {
        let (mut node, w, r) = make_bus();
        let result = ForecastResult {
            target: "power",
            horizon_ms: 1000,
            points: vec![ForecastPoint::default()],
            confidence: 0.5,
            degraded: false,
        };
        publish_forecast(&mut node, w, &result).expect("publish");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 1);
    }

    // ===== T34: publish payload 可解析 — target/points 数组/confidence 数值/degraded bool =====
    #[test]
    fn t34_publish_payload_parseable() {
        let (mut node, w, r) = make_bus();
        let pred = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let result = pred.forecast(&grid_model(), 10000).expect("forecast");
        publish_forecast(&mut node, w, &result).expect("publish");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 1);
        let v: serde_json::Value = serde_json::from_slice(&samples[0].payload).expect("valid json");
        assert_eq!(v["target"], "power");
        assert!(v["points"].is_array());
        assert!(!v["points"].as_array().expect("array").is_empty());
        assert!(v["confidence"].is_number());
        assert!(v["degraded"].is_boolean());
    }

    // ===== T35: node.shutdown() 后 publish_forecast → Err(ForecastError::Dds(_)) =====
    #[test]
    fn t35_publish_shutdown_node_fails() {
        let (mut node, w, _r) = make_bus();
        node.shutdown().expect("shutdown");
        let result = ForecastResult {
            target: "power",
            horizon_ms: 1000,
            points: vec![ForecastPoint::default()],
            confidence: 0.5,
            degraded: false,
        };
        let r = publish_forecast(&mut node, w, &result);
        assert!(matches!(r, Err(ForecastError::Dds(_))));
    }

    // ===== T36: forecast_and_publish 端到端 — 返回 points 数 == reader 样本 JSON points 数 =====
    #[test]
    fn t36_forecast_and_publish_end_to_end() {
        let (mut node, w, r) = make_bus();
        let pred = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let res = pred
            .forecast_and_publish(&grid_model(), 30000, &mut node, w)
            .expect("forecast_and_publish");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 1);
        let v: serde_json::Value = serde_json::from_slice(&samples[0].payload).expect("valid json");
        assert_eq!(
            v["points"].as_array().expect("array").len(),
            res.points.len()
        );
    }

    // ===== T37: 精度占位（D6）— 真实恒定 50.0，持续法 MAPE == 0.0（< 5%）=====
    #[test]
    fn t37_accuracy_constant_truth() {
        let mut m = TwinModel::default();
        m.grid.active_power = 50.0;
        m.grid.timestamp = 1;
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&m, 10000).expect("forecast");
        let mut mape = 0.0f32;
        for pt in &r.points {
            let truth = 50.0f32;
            mape += (pt.value - truth).abs() / truth.abs() / r.points.len() as f32;
        }
        assert!(mape.abs() < 1e-9);
        assert!(mape < 0.05);
    }

    // ===== T38: 缓变斜坡 v_k = 50*(1+0.001k)（k=1..=10）持续法 MAPE < 5% =====
    #[test]
    fn t38_accuracy_gentle_ramp() {
        let mut m = TwinModel::default();
        m.grid.active_power = 50.0;
        m.grid.timestamp = 1;
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&m, 10000).expect("forecast");
        assert_eq!(r.points.len(), 10);
        let mut mape = 0.0f32;
        for (i, pt) in r.points.iter().enumerate() {
            let k = (i + 1) as f32;
            let truth = 50.0 * (1.0 + 0.001 * k);
            mape += (pt.value - truth).abs() / truth.abs();
        }
        mape /= r.points.len() as f32;
        assert!(mape < 0.05);
    }

    // ===== T39: 3 设备（power 1.0/2.0/3.0）grid 无数据 → forecast value==6.0 =====
    #[test]
    fn t39_multi_device_sum_fallback() {
        let mut m = TwinModel::default();
        for (id, pw) in [(1u64, 1.0f64), (2, 2.0), (3, 3.0)] {
            let mut d = DeviceTwin {
                device_id: id,
                ..DeviceTwin::default()
            };
            d.state.power = pw;
            m.devices.insert(id, d);
        }
        let p = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        let r = p.forecast(&m, 10000).expect("forecast");
        assert!(r.points.iter().all(|pt| (pt.value - 6.0).abs() < 1e-6));
    }

    // ===== T40: 连续两次 forecast_and_publish → reader 累计 take 到 2 条样本 =====
    #[test]
    fn t40_two_publishes_two_samples() {
        let (mut node, w, r) = make_bus();
        let pred = Predictor::new(Box::new(PersistenceModel), 1000, 96, 0.5);
        pred.forecast_and_publish(&grid_model(), 10000, &mut node, w)
            .expect("publish 1");
        pred.forecast_and_publish(&grid_model(), 10000, &mut node, w)
            .expect("publish 2");
        let samples = node.take(r, 10).expect("take");
        assert_eq!(samples.len(), 2);
    }
}
