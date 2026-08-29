//! 22 §2.W2.5：轻量在线学习（锁定 8——增量更新，不重训/不引入 PyTorch）。
//!
//! v1.0.0 学习范围：
//! ①**deadband 自适应**：遥测死区按噪声统计自调（3σ 规则，带硬上下界）；
//! ②**LP 预测系数修正**：基于 W2.1 偏差（forecast_error）的 EMA 修正
//!   （钳位安全界内）；
//! 所有学习输出经 **safety-validator 的 `SafetyCatalog`**（W5.3）校验才
//! 生效——**越界拒绝（用旧值）+ 计数**，不静默钳位（红线4），
//! "学坏不致灾"。
//!
//! `rl`/`marl` crate 不在本期范围（`v1.0.0_scope = "deferred"` 标注于
//! 对应 crate 文档，不删但不声称可用）。

use eneros_safety_validator::SafetyCatalog;

/// 死区自适应学习器（遥测 deadband 按噪声统计自调）。
///
/// 规则：噪声 EMA 跟踪偏差量级 → 提议死区 = 3 × 噪声估计 →
/// safety catalog 校验（越界拒绝，用旧值 + 计数）。
#[derive(Debug, Clone)]
pub struct DeadbandAdaptive {
    /// 当前生效死区。
    pub current: f64,
    /// 死区硬下界（safety catalog）。
    pub min: f64,
    /// 死区硬上界（safety catalog）。
    pub max: f64,
    /// 安全目录（W5.3：越界拒绝的唯一权威）。
    catalog: Option<SafetyCatalog>,
    /// 噪声 EMA（偏差量级估计）。
    noise_ema: f64,
    /// 学习样本数。
    pub samples: u64,
    /// 安全校验拒绝次数（学坏不致灾的证据）。
    pub rejected_updates: u64,
}

impl DeadbandAdaptive {
    /// 新建（`initial` 必须在 [min, max] 内；无目录 = 以 [min, max] 为界）。
    pub fn new(initial: f64, min: f64, max: f64) -> Self {
        let initial = initial.clamp(min, max);
        Self {
            current: initial,
            min,
            max,
            catalog: None,
            noise_ema: initial / 3.0,
            samples: 0,
            rejected_updates: 0,
        }
    }

    /// 新建（W5.3：边界来自 `SafetyCatalog`，学习器不得自行放宽）。
    pub fn with_catalog(initial: f64, catalog: SafetyCatalog) -> Self {
        let mut s = Self::new(initial, catalog.deadband_min, catalog.deadband_max);
        s.catalog = Some(catalog);
        s
    }

    /// SPEC-SERIES-2026-08-27/E WP-E.3：从持久快照恢复（EMA/样本/拒绝计数
    /// 原样回载——重启后学习状态连续，而非 EMA 冷启动）。输入须已过
    /// catalog 界校验（快照写入口负责）；越界输入按界钳制（防御性）。
    pub fn restore_with_catalog(
        current: f64,
        noise_ema: f64,
        samples: u64,
        rejected_updates: u64,
        catalog: SafetyCatalog,
    ) -> Self {
        let mut s = Self::with_catalog(current, catalog);
        s.noise_ema = if noise_ema.is_finite() {
            noise_ema
        } else {
            current / 3.0
        };
        s.samples = samples;
        s.rejected_updates = rejected_updates;
        s
    }

    /// safety-validator：值域 + catalog 硬界检查。
    pub fn validate(v: f64, min: f64, max: f64) -> bool {
        v.is_finite() && v >= min && v <= max
    }

    /// 校验提议值（优先 catalog，否则内联界）。
    fn check(&self, v: f64) -> bool {
        match &self.catalog {
            Some(c) => c.check_deadband(v),
            None => Self::validate(v, self.min, self.max),
        }
    }

    /// 学习一步：喂入观测偏差（|预测 − 观测|，W2.1 输出）→ 更新噪声
    /// EMA → 提议新死区（3σ）→ safety catalog 校验后生效。
    ///
    /// 返回当前生效死区（**越界拒绝：用旧值 + 计数**，不静默钳位）。
    pub fn learn(&mut self, deviation: f64) -> f64 {
        if !deviation.is_finite() || deviation < 0.0 {
            return self.current; // 非法输入不学习（fail-closed）
        }
        self.samples += 1;
        // EMA（α = 0.2；噪声估计向偏差量级收敛）。
        self.noise_ema = 0.8 * self.noise_ema + 0.2 * deviation;
        // 提议死区 = 3σ（**不钳位**：越界必须被 catalog 拒绝，用旧值）。
        let proposed = 3.0 * self.noise_ema;
        if self.check(proposed) {
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

/// LP 预测系数修正器（基于历史偏差的轻量增量，安全界内）。
///
/// 规则：偏差 EMA → 修正系数；safety catalog 校验（超界拒绝，
/// 回退 0 修正 + 计数）。输出侧 `correction()` 仍钳位（输出防御）。
#[derive(Debug, Clone)]
pub struct ForecastCorrector {
    /// 修正系数上限（安全界：不许修正超过 ±50%）。
    pub max_correction: f64,
    /// 安全目录（W5.3）。
    catalog: Option<SafetyCatalog>,
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
            catalog: None,
            bias_ema: 0.0,
            samples: 0,
            rejected_updates: 0,
        }
    }

    /// 新建（W5.3：上限来自 `SafetyCatalog`）。
    pub fn with_catalog(catalog: SafetyCatalog) -> Self {
        let mut s = Self::new(catalog.max_correction);
        s.catalog = Some(catalog);
        s
    }

    /// safety-validator：修正系数必须落在安全界内。
    pub fn validate(c: f64, max_correction: f64) -> bool {
        c.is_finite() && c.abs() <= max_correction
    }

    /// 校验修正系数（优先 catalog，否则内联界）。
    fn check(&self, c: f64) -> bool {
        match &self.catalog {
            Some(cat) => cat.check_correction(c),
            None => Self::validate(c, self.max_correction),
        }
    }

    /// 学习一步：喂入偏差（预测 − 观测，可正可负）→ EMA → 修正系数。
    ///
    /// **越界拒绝：回退 0 修正 + 计数**（不静默钳位）。
    pub fn learn(&mut self, bias: f64) -> f64 {
        if !bias.is_finite() {
            return self.correction(); // 非法输入不学习
        }
        self.samples += 1;
        self.bias_ema = 0.8 * self.bias_ema + 0.2 * bias;
        if self.check(self.bias_ema) {
            self.bias_ema
        } else {
            self.rejected_updates += 1;
            self.bias_ema = 0.0; // 越界回退（用旧值 0 修正）
            0.0
        }
    }

    /// 当前修正系数（输出侧钳位防御；学习已保证 EMA 在界内或已回退）。
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

    /// 死区向噪声收敛（3σ 规则），且在硬界内；越界后拒绝计数。
    #[test]
    fn test_deadband_adapts_to_noise_within_bounds() {
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        // 持续喂 5.0 的偏差 → 噪声 EMA 渐进 → 死区接近 15 时被 catalog
        // 拒绝（3σ > 上界 10），当前值停留在界内。
        for _ in 0..100 {
            db.learn(5.0);
        }
        assert!(db.current > 1.0, "界内噪声应被学习: {}", db.current);
        assert!(db.current <= 10.0, "死区绝不越上界: {}", db.current);
        assert!(db.rejected_updates > 0, "越界拒绝必须计数");
        assert_eq!(db.samples, 100);
    }

    /// 无噪声时死区回落（EMA 衰减，仍校验）。
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
        let mut db = DeadbandAdaptive::new(1.0, 0.1, 10.0);
        // 持续大偏差 → 3σ 提议稳定超上界 → 每次拒绝计数。
        for _ in 0..5 {
            db.learn(50.0);
        }
        assert_eq!(db.current, 1.0, "旧值保持");
        assert_eq!(db.rejected_updates, 5, "每次越界都计数");
        // validate 边界语义。
        assert!(DeadbandAdaptive::validate(5.0, 0.1, 10.0));
        assert!(!DeadbandAdaptive::validate(10.5, 0.1, 10.0));
        assert!(!DeadbandAdaptive::validate(0.05, 0.1, 10.0));
        assert!(!DeadbandAdaptive::validate(f64::NAN, 0.1, 10.0));
    }

    /// ForecastCorrector：界内偏差收敛；极端偏差被拒（用旧值 0）。
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
        // 极端偏差 → 越界拒绝（回退 0 修正，学坏不致灾）。
        for _ in 0..200 {
            fc.learn(10.0);
        }
        assert_eq!(fc.correction(), 0.0, "越界必须回退旧值 0");
        assert!(fc.rejected_updates > 0, "越界拒绝必须计数");
    }

    /// 非法输入不学习。
    #[test]
    fn test_forecast_corrector_rejects_invalid() {
        let mut fc = ForecastCorrector::new(0.5);
        fc.learn(f64::NAN);
        assert_eq!(fc.samples, 0);
        assert_eq!(fc.correction(), 0.0);
    }

    // ===== W5.3 验收：学习输出经 SafetyCatalog 校验 =====

    /// 验收：学习输出越界被拒 + 用旧值（catalog 路径，不静默钳位）。
    #[test]
    fn test_w53_catalog_rejects_and_keeps_old() {
        let catalog = SafetyCatalog::default_eps();
        let mut db = DeadbandAdaptive::with_catalog(1.0, catalog);
        // 大偏差 → 3σ 提议首次即超目录上界 10 → 拒绝，旧值保持。
        for _ in 0..10 {
            db.learn(100.0);
        }
        assert_eq!(db.current, 1.0, "catalog 越界必须用旧值");
        assert_eq!(db.rejected_updates, 10);

        let mut fc = ForecastCorrector::with_catalog(catalog);
        for _ in 0..10 {
            fc.learn(5.0); // EMA → 1.0 超 ±0.5 → 拒绝回退
        }
        assert_eq!(fc.correction(), 0.0, "catalog 越界回退 0 修正");
        assert_eq!(fc.rejected_updates, 10);
    }

    /// 验收：不致灾——极端学习输入（最大噪声/偏差）下输出仍被 catalog
    /// 拒于界外，系统不采纳（catalog 是唯一权威）。
    #[test]
    fn test_w53_no_disaster_under_extreme_input() {
        let catalog = SafetyCatalog::default_eps();
        let mut db = DeadbandAdaptive::with_catalog(1.0, catalog);
        // 极端输入（无限偏差）：feed 大量大值 → 死区绝不越 catalog 界。
        for _ in 0..1000 {
            db.learn(1e9);
        }
        assert!(
            catalog.check_deadband(db.current),
            "死区必须始终在 catalog 界内"
        );
        assert_eq!(db.current, 1.0, "极端输入下保持初始安全值");

        let mut fc = ForecastCorrector::with_catalog(catalog);
        for _ in 0..1000 {
            fc.learn(1e9);
        }
        assert!(
            catalog.check_correction(fc.correction()),
            "修正系数必须始终在界内"
        );
        assert_eq!(fc.correction(), 0.0);
    }
}
