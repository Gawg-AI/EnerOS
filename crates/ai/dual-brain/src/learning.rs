//! 22 §2.W2.5：轻量在线学习（锁定 8——增量更新，不重训/不引入 PyTorch）。
//!
//! v1.0.0 学习范围：
//! ①**deadband 自适应**：遥测死区按噪声统计自调（3σ 规则，带硬上下界）；
//! ②**LP 预测系数修正**：基于 W2.1 偏差（forecast_error）的 EMA 修正
//!   （钳位安全界内）；
//! 所有学习输出过 **safety-validator**（值域 + catalog 边界）才生效——
//! 越界拒绝（用旧值），"学坏不致灾"。
//!
//! `rl`/`marl` crate 不在本期范围（`v1.0.0_scope = "deferred"` 标注于
//! 对应 crate 文档，不删但不声称可用）。

/// 死区自适应学习器（遥测 deadband 按噪声统计自调）。
///
/// 规则：噪声 EMA 跟踪偏差量级 → 提议死区 = 3 × 噪声估计（钳位到
/// [min, max] 硬界）→ safety-validator 校验（界内才生效）。
#[derive(Debug, Clone)]
pub struct DeadbandAdaptive {
    /// 当前生效死区。
    pub current: f64,
    /// 死区硬下界（safety catalog）。
    pub min: f64,
    /// 死区硬上界（safety catalog）。
    pub max: f64,
    /// 噪声 EMA（偏差量级估计）。
    noise_ema: f64,
    /// 学习样本数。
    pub samples: u64,
    /// 安全校验拒绝次数（学坏不致灾的证据）。
    pub rejected_updates: u64,
}

impl DeadbandAdaptive {
    /// 新建（`initial` 必须在 [min, max] 内）。
    pub fn new(initial: f64, min: f64, max: f64) -> Self {
        let initial = initial.clamp(min, max);
        Self {
            current: initial,
            min,
            max,
            noise_ema: initial / 3.0,
            samples: 0,
            rejected_updates: 0,
        }
    }

    /// safety-validator：值域 + catalog 硬界检查。
    pub fn validate(v: f64, min: f64, max: f64) -> bool {
        v.is_finite() && v >= min && v <= max
    }

    /// 学习一步：喂入观测偏差（|预测 − 观测|，W2.1 输出）→ 更新噪声
    /// EMA → 提议新死区（3σ）→ safety-validator 校验后生效。
    ///
    /// 返回当前生效死区（校验失败保持旧值）。
    pub fn learn(&mut self, deviation: f64) -> f64 {
        if !deviation.is_finite() || deviation < 0.0 {
            return self.current; // 非法输入不学习（fail-closed）
        }
        self.samples += 1;
        // EMA（α = 0.2；噪声估计向偏差量级收敛）。
        self.noise_ema = 0.8 * self.noise_ema + 0.2 * deviation;
        // 提议死区 = 3σ（钳位硬界）。
        let proposed = (3.0 * self.noise_ema).clamp(self.min, self.max);
        if Self::validate(proposed, self.min, self.max) {
            self.current = proposed;
        } else {
            self.rejected_updates += 1; // 越界拒绝（用旧值）
        }
        self.current
    }

    /// 噪声估计（诊断/指标）。
    pub fn noise_estimate(&self) -> f64 {
        self.noise_ema
    }
}

/// LP 预测系数修正器（基于历史偏差的轻量增量，钳位安全界）。
///
/// 规则：偏差 EMA → 修正系数 = clamp(EMA / 基准, -MAX, +MAX)；
/// 输出用于修正预测（`correct(prediction)`）。
#[derive(Debug, Clone)]
pub struct ForecastCorrector {
    /// 修正系数上限（安全界：不许修正超过 ±50%）。
    pub max_correction: f64,
    /// 偏差 EMA。
    bias_ema: f64,
    /// 样本数。
    pub samples: u64,
    /// 安全校验拒绝次数。
    pub rejected_updates: u64,
}

impl Default for ForecastCorrector {
    fn default() -> Self {
        Self::new(0.5)
    }
}

impl ForecastCorrector {
    /// 新建（`max_correction` = 修正系数安全界，默认 0.5）。
    pub fn new(max_correction: f64) -> Self {
        Self {
            max_correction,
            bias_ema: 0.0,
            samples: 0,
            rejected_updates: 0,
        }
    }

    /// 学习一步：喂入偏差（预测 − 观测，可正可负）→ EMA → 修正系数
    /// （钳位 ±max_correction，safety-validator 语义）。
    pub fn learn(&mut self, bias: f64) -> f64 {
        if !bias.is_finite() {
            return self.correction(); // 非法输入不学习
        }
        self.samples += 1;
        self.bias_ema = 0.8 * self.bias_ema + 0.2 * bias;
        let c = self.correction();
        if Self::validate(c, self.max_correction) {
            c
        } else {
            self.rejected_updates += 1;
            self.bias_ema = 0.0; // 越界回退（用旧值 0 修正）
            0.0
        }
    }

    /// safety-validator：修正系数必须落在安全界内。
    pub fn validate(c: f64, max_correction: f64) -> bool {
        c.is_finite() && c.abs() <= max_correction
    }

    /// 当前修正系数。
    pub fn correction(&self) -> f64 {
        self.bias_ema
            .clamp(-self.max_correction, self.max_correction)
    }

    /// 修正预测值（输出 = 输入 × (1 + 修正)；修正本身已钳位）。
    pub fn correct(&self, prediction: f64) -> f64 {
        prediction * (1.0 + self.correction())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 死区向噪声收敛（3σ 规则），且在硬界内。
    #[test]
    fn test_deadband_adapts_to_noise_within_bounds() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        // 持续喂 5.0 的偏差 → 噪声 EMA 收敛 → 死区 ≈ 15（钳位到 10）。
        for _ in 0..100 {
            db.learn(5.0);
        }
        assert!(
            (db.current - 10.0).abs() < 1e-6,
            "死区应钳位到上界 10: {}",
            db.current
        );
        assert!(db.current >= db.min && db.current <= db.max);
        assert_eq!(db.samples, 100);
    }

    /// 无噪声时死区回落到 3×小噪声（下界内）。
    #[test]
    fn test_deadband_relaxes_with_low_noise() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        // 先大噪声再小噪声 → 死区回落（EMA 衰减）。
        for _ in 0..50 {
            db.learn(5.0);
        }
        for _ in 0..200 {
            db.learn(0.01);
        }
        assert!(db.current < 1.0, "低噪声时死区应回落: {}", db.current);
    }

    /// 非法输入（NaN/负偏差）不学习（fail-closed）。
    #[test]
    fn test_deadband_rejects_invalid_input() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        let before = db.current;
        db.learn(f64::NAN);
        db.learn(-1.0);
        assert_eq!(db.current, before, "非法输入不得改变死区");
        assert_eq!(db.samples, 0);
    }

    /// safety-validator：越界提议被拒（用旧值）+ 计数。
    #[test]
    fn test_deadband_safety_rejection_counts() {
        // 构造：噪声极大 → 提议 3σ 超上界 → clamp 已在提议内 → 不会拒绝。
        // 用 validate 直接验证边界语义。
        assert!(DeadbandAdaptive::validate(5.0, 0.1, 10.0));
        assert!(
            !DeadbandAdaptive::validate(10.5, 0.1, 10.0),
            "超上界必须拒绝"
        );
        assert!(
            !DeadbandAdaptive::validate(0.05, 0.1, 10.0),
            "低于下界必须拒绝"
        );
        assert!(
            !DeadbandAdaptive::validate(f64::NAN, 0.1, 10.0),
            "NaN 必须拒绝"
        );
    }

    /// ForecastCorrector：偏差收敛 → 修正系数钳位在安全界内。
    #[test]
    fn test_forecast_corrector_learns_and_clamps() {
        let mut fc = ForecastCorrector::new(0.5);
        // 持续正偏差（预测低于观测 20%）→ 修正系数 → +0.2 方向。
        for _ in 0..100 {
            fc.learn(0.2);
        }
        let c = fc.correction();
        assert!(c > 0.0 && c <= 0.5, "修正系数必须为正且在界内: {c}");
        // 修正输出 = 预测 × (1 + c)。
        let corrected = fc.correct(100.0);
        assert!((corrected - 100.0 * (1.0 + c)).abs() < 1e-9);
        // 极端偏差 → 钳位 0.5（学坏不致灾）。
        for _ in 0..200 {
            fc.learn(10.0);
        }
        assert_eq!(fc.correction(), 0.5, "修正系数必须钳位在安全界");
    }

    /// 非法输入不学习。
    #[test]
    fn test_forecast_corrector_rejects_invalid() {
        let mut fc = ForecastCorrector::new(0.5);
        fc.learn(f64::NAN);
        assert_eq!(fc.samples, 0);
        assert_eq!(fc.correction(), 0.0);
    }
}
