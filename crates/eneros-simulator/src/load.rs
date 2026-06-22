//! 负荷曲线生成器
//!
//! 生成典型日/周负荷曲线，支持新能源出力（光伏、风电）与随机噪声叠加。
//!
//! ## 功能
//!
//! - 按区域类型（工业/商业/居民）生成不同形态的日负荷曲线
//! - 按季节（春/夏/秋/冬）调整负荷水平（夏冬空调负荷更高）
//! - 可选叠加光伏、风电出力，得到净负荷曲线
//! - 可选叠加确定性伪随机噪声，模拟负荷波动
//!
//! ## 模型说明
//!
//! 负荷模型为简化经验模型，用于仿真测试，不代表精确的负荷预测：
//! - 工业负荷：白天高、夜间低，整体波动小
//! - 商业负荷：9-21 点高，其余时段低
//! - 居民负荷：早晚双高峰（7 点、19 点）
//! - 光伏：6-18 点正弦出力，正午峰值，受天气因子影响
//! - 风电：简化 Weibull 模型，夜间略高，受天气因子影响

use serde::{Deserialize, Serialize};

/// 季节
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    /// 春季
    Spring,
    /// 夏季
    Summer,
    /// 秋季
    Autumn,
    /// 冬季
    Winter,
}

/// 区域类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RegionType {
    /// 工业
    Industrial,
    /// 商业
    Commercial,
    /// 居民
    Residential,
}

/// 负荷曲线点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadPoint {
    /// 时间（小时，0.0-24.0）
    pub hour: f64,
    /// 有功功率（MW）
    pub p_mw: f64,
    /// 无功功率（MVar）
    pub q_mvar: f64,
}

/// 新能源配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenewableConfig {
    /// 光伏装机容量（MW）
    pub solar_capacity_mw: f64,
    /// 风电装机容量（MW）
    pub wind_capacity_mw: f64,
    /// 天气因子（0.0-1.0，1.0 为晴天）
    #[serde(default = "default_weather_factor")]
    pub weather_factor: f64,
}

fn default_weather_factor() -> f64 {
    0.8
}

/// 负荷曲线生成器
pub struct LoadProfileGenerator {
    /// 基准负荷（MW）
    base_load_mw: f64,
    /// 季节
    season: Season,
    /// 区域类型
    region_type: RegionType,
    /// 新能源配置
    renewable: Option<RenewableConfig>,
    /// 噪声标准差（MW）
    noise_stddev: f64,
}

impl LoadProfileGenerator {
    /// 创建负荷曲线生成器
    ///
    /// - `base_load_mw`：基准负荷（MW），负荷曲线在此基础上按负荷因子缩放
    /// - `season`：季节，影响整体负荷水平
    /// - `region_type`：区域类型，决定负荷曲线形态
    pub fn new(base_load_mw: f64, season: Season, region_type: RegionType) -> Self {
        Self {
            base_load_mw,
            season,
            region_type,
            renewable: None,
            noise_stddev: 0.0,
        }
    }

    /// 配置新能源（builder 风格）
    pub fn with_renewable(mut self, config: RenewableConfig) -> Self {
        self.renewable = Some(config);
        self
    }

    /// 配置噪声标准差（builder 风格）
    pub fn with_noise(mut self, stddev: f64) -> Self {
        self.noise_stddev = stddev;
        self
    }

    /// 生成典型日负荷曲线（96 点，15分钟间隔）
    ///
    /// 返回 96 个 [`LoadPoint`]，时间从 0.0 到 23.75 小时。
    /// 若配置了新能源，则返回净负荷（扣除光伏、风电出力）。
    /// 若配置了噪声，则叠加确定性伪随机噪声。
    pub fn daily_typical(&self) -> Vec<LoadPoint> {
        let mut points = Vec::with_capacity(96);
        for i in 0..96 {
            let hour = i as f64 * 0.25;
            let load_factor = self.load_factor(hour);
            let p_mw = self.base_load_mw * load_factor;

            let mut net_load = p_mw;

            // 扣除新能源出力
            if let Some(ref renewable) = self.renewable {
                let solar = self.solar_output(hour, renewable);
                let wind = self.wind_output(hour, renewable);
                net_load = (p_mw - solar - wind).max(0.0);
            }

            // 添加噪声
            let noise = if self.noise_stddev > 0.0 {
                self.pseudo_random(hour) * self.noise_stddev
            } else {
                0.0
            };

            // 功率因数约 0.957，对应 q = p * tan(acos(0.957)) ≈ p * 0.3
            let final_p = (net_load + noise).max(0.0);
            points.push(LoadPoint {
                hour,
                p_mw: final_p,
                q_mvar: final_p * 0.3,
            });
        }
        points
    }

    /// 生成典型周负荷曲线（7 天 × 96 点）
    ///
    /// 周一至周五（day 0-4）为工作日，周六周日（day 5-6）负荷按 0.7 系数降低。
    pub fn weekly_typical(&self) -> Vec<Vec<LoadPoint>> {
        (0..7)
            .map(|day| {
                let mut day_profile = self.daily_typical();
                // 周末负荷降低
                if day >= 5 {
                    for point in &mut day_profile {
                        point.p_mw *= 0.7;
                        point.q_mvar *= 0.7;
                    }
                }
                day_profile
            })
            .collect()
    }

    /// 负荷因子（基于时间和区域类型）
    fn load_factor(&self, hour: f64) -> f64 {
        let base_factor = match self.region_type {
            RegionType::Industrial => self.industrial_load_factor(hour),
            RegionType::Commercial => self.commercial_load_factor(hour),
            RegionType::Residential => self.residential_load_factor(hour),
        };

        // 季节调整
        let season_factor = match self.season {
            Season::Summer | Season::Winter => 1.1, // 夏冬空调负荷
            Season::Spring | Season::Autumn => 0.9,
        };

        base_factor * season_factor
    }

    /// 工业负荷因子
    fn industrial_load_factor(&self, hour: f64) -> f64 {
        // 工业负荷：白天高、夜间低，但波动小
        0.7 + 0.3 * ((hour - 6.0) / 12.0 * std::f64::consts::PI)
            .sin()
            .max(0.0)
    }

    /// 商业负荷因子
    fn commercial_load_factor(&self, hour: f64) -> f64 {
        // 商业负荷：9-21 点高
        if (9.0..=21.0).contains(&hour) {
            0.6 + 0.4 * ((hour - 9.0) / 12.0 * std::f64::consts::PI).sin()
        } else {
            0.3
        }
    }

    /// 居民负荷因子
    fn residential_load_factor(&self, hour: f64) -> f64 {
        // 居民负荷：早晚高峰
        let morning_peak = (-((hour - 7.0) / 1.5).powi(2)).exp();
        let evening_peak = (-((hour - 19.0) / 2.0).powi(2)).exp();
        0.3 + 0.5 * morning_peak + 0.7 * evening_peak
    }

    /// 光伏出力（正弦模型 + 天气因子）
    fn solar_output(&self, hour: f64, config: &RenewableConfig) -> f64 {
        // 光伏：6-18 点出力，正午峰值
        if !(6.0..=18.0).contains(&hour) {
            return 0.0;
        }
        let solar_factor = ((hour - 6.0) / 12.0 * std::f64::consts::PI).sin();
        config.solar_capacity_mw * solar_factor * config.weather_factor
    }

    /// 风电出力（简化 Weibull 模型）
    fn wind_output(&self, hour: f64, config: &RenewableConfig) -> f64 {
        // 风电：夜间略高，简化模型
        let wind_factor = 0.4 + 0.2 * (hour / 24.0 * 2.0 * std::f64::consts::PI).cos();
        config.wind_capacity_mw * wind_factor * config.weather_factor
    }

    /// 伪随机数生成（基于时间的确定性噪声）
    fn pseudo_random(&self, hour: f64) -> f64 {
        // 简化的伪随机：基于 sin 的哈希，范围约 [-1, 1]
        let x = (hour * 12345.6789).sin();
        x * x * x.signum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试创建负荷曲线生成器
    #[test]
    fn test_load_profile_generator_new() {
        let gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        // 创建成功后应能生成日曲线
        let daily = gen.daily_typical();
        assert!(!daily.is_empty(), "日曲线不应为空");
        // 基准负荷 100 MW，工业负荷因子最小 0.7 * 季节 1.1 = 0.77，故最小值约 77 MW
        let min_p = daily
            .iter()
            .map(|p| p.p_mw)
            .fold(f64::INFINITY, f64::min);
        assert!(min_p > 0.0, "最小负荷应大于 0: {}", min_p);
    }

    /// 测试日曲线长度为 96 点（15 分钟间隔）
    #[test]
    fn test_daily_typical_length() {
        let gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let daily = gen.daily_typical();
        assert_eq!(daily.len(), 96, "日曲线应有 96 个点");
        // 验证时间间隔为 0.25 小时
        assert!((daily[0].hour - 0.0).abs() < 1e-9, "首点时间应为 0.0");
        assert!((daily[1].hour - 0.25).abs() < 1e-9, "第二点时间应为 0.25");
        assert!(
            (daily[95].hour - 23.75).abs() < 1e-9,
            "末点时间应为 23.75"
        );
    }

    /// 测试日曲线值在合理范围
    #[test]
    fn test_daily_typical_values() {
        let gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let daily = gen.daily_typical();
        for point in &daily {
            assert!(point.p_mw >= 0.0, "p_mw 不应为负: {}", point.p_mw);
            assert!(point.q_mvar >= 0.0, "q_mvar 不应为负: {}", point.q_mvar);
            // 负荷不应超过基准的 2 倍（含季节与负荷因子上限）
            assert!(
                point.p_mw <= 200.0,
                "p_mw 超出合理范围: {}",
                point.p_mw
            );
            // 无功约为有功的 0.3 倍
            assert!(
                (point.q_mvar - point.p_mw * 0.3).abs() < 1e-6,
                "q_mvar 应约为 p_mw 的 0.3 倍"
            );
        }
    }

    /// 测试周曲线长度为 7 天，每天 96 点
    #[test]
    fn test_weekly_typical_length() {
        let gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let weekly = gen.weekly_typical();
        assert_eq!(weekly.len(), 7, "周曲线应有 7 天");
        for (i, day) in weekly.iter().enumerate() {
            assert_eq!(day.len(), 96, "第 {} 天应有 96 个点", i);
        }
    }

    /// 测试周末负荷低于工作日
    #[test]
    fn test_weekly_weekend_reduction() {
        let gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let weekly = gen.weekly_typical();
        // 工作日（周一）总负荷
        let weekday_total: f64 = weekly[0].iter().map(|p| p.p_mw).sum();
        // 周末（周六）总负荷
        let weekend_total: f64 = weekly[5].iter().map(|p| p.p_mw).sum();
        assert!(
            weekend_total < weekday_total,
            "周末负荷应低于工作日: weekday={}, weekend={}",
            weekday_total,
            weekend_total
        );
        // 周末约为工作日的 70%
        let ratio = weekend_total / weekday_total;
        assert!(
            (ratio - 0.7).abs() < 1e-6,
            "周末负荷比例应接近 0.7: {}",
            ratio
        );
    }

    /// 测试新能源出力降低净负荷
    #[test]
    fn test_with_renewable() {
        let base_gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let renewable_gen =
            LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial)
                .with_renewable(RenewableConfig {
                    solar_capacity_mw: 50.0,
                    wind_capacity_mw: 30.0,
                    weather_factor: 1.0,
                });
        let base_total: f64 = base_gen.daily_typical().iter().map(|p| p.p_mw).sum();
        let renewable_total: f64 = renewable_gen.daily_typical().iter().map(|p| p.p_mw).sum();
        assert!(
            renewable_total < base_total,
            "新能源应降低净负荷: base={}, renewable={}",
            base_total,
            renewable_total
        );
    }

    /// 测试噪声对负荷曲线的影响
    #[test]
    fn test_with_noise() {
        let base_gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let noise_gen =
            LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial)
                .with_noise(5.0);
        let base_daily = base_gen.daily_typical();
        let noise_daily = noise_gen.daily_typical();
        // 噪声应使曲线至少部分点发生变化
        let diff_count = base_daily
            .iter()
            .zip(noise_daily.iter())
            .filter(|(a, b)| (a.p_mw - b.p_mw).abs() > 1e-9)
            .count();
        assert!(diff_count > 0, "噪声应使负荷曲线发生变化");
        // 噪声不应使负荷变为负值
        for point in &noise_daily {
            assert!(point.p_mw >= 0.0, "加噪后负荷不应为负: {}", point.p_mw);
        }
    }

    /// 测试季节变化（夏冬负荷高于春秋）
    #[test]
    fn test_season_variation() {
        let summer_gen = LoadProfileGenerator::new(100.0, Season::Summer, RegionType::Industrial);
        let winter_gen = LoadProfileGenerator::new(100.0, Season::Winter, RegionType::Industrial);
        let spring_gen = LoadProfileGenerator::new(100.0, Season::Spring, RegionType::Industrial);
        let autumn_gen = LoadProfileGenerator::new(100.0, Season::Autumn, RegionType::Industrial);

        let summer_total: f64 = summer_gen.daily_typical().iter().map(|p| p.p_mw).sum();
        let winter_total: f64 = winter_gen.daily_typical().iter().map(|p| p.p_mw).sum();
        let spring_total: f64 = spring_gen.daily_typical().iter().map(|p| p.p_mw).sum();
        let autumn_total: f64 = autumn_gen.daily_typical().iter().map(|p| p.p_mw).sum();

        assert!(summer_total > spring_total, "夏季负荷应高于春季");
        assert!(summer_total > autumn_total, "夏季负荷应高于秋季");
        assert!(winter_total > spring_total, "冬季负荷应高于春季");
        assert!(winter_total > autumn_total, "冬季负荷应高于秋季");
    }
}
