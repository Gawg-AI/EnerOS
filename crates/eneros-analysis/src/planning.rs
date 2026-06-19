//! Distribution network planning parameters and evaluation (inspired by cnpower's
//! `engineering/planning_library.py`).
//!
//! Provides voltage limits, loading limits, supply radius, load models,
//! renewable hosting capacity, energy storage planning, and candidate plan
//! generation for Chinese distribution network planning per GB/T standards.

use serde::{Deserialize, Serialize};

/// Supply area classification per DL/T 5729 (A/B/C/D/E).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupplyAreaClass {
    /// A类: 中心城区，高可靠性要求
    A,
    /// B类: 一般城区
    B,
    /// C类: 郊区
    C,
    /// D类: 农村地区
    D,
    /// E类: 偏远地区
    E,
}

impl SupplyAreaClass {
    /// N-1 通过率要求（%）
    pub fn n1_pass_rate_percent(&self) -> f64 {
        match self {
            Self::A | Self::B => 100.0,
            Self::C => 90.0,
            Self::D | Self::E => 0.0, // 不要求
        }
    }

    /// 供电可靠性 RS-1 要求（%）
    pub fn reliability_rs1_percent(&self) -> f64 {
        match self {
            Self::A => 99.99,
            Self::B => 99.95,
            Self::C => 99.9,
            Self::D => 99.5,
            Self::E => 99.0,
        }
    }
}

/// Voltage limits per GB/T 12325 (voltage deviation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoltageLimits {
    /// 额定电压 (kV)
    pub rated_voltage_kv: f64,
    /// 正偏差限值 (%)
    pub positive_deviation_percent: f64,
    /// 负偏差限值 (%)
    pub negative_deviation_percent: f64,
}

impl VoltageLimits {
    /// 按 GB/T 12325 获取电压偏差限值
    pub fn for_voltage_level(rated_voltage_kv: f64) -> Self {
        // 35 kV 及以上和 10–35 kV 均为 ±7%（GB/T 12325-2008）
        if rated_voltage_kv >= 10.0 {
            Self {
                rated_voltage_kv,
                positive_deviation_percent: 7.0,
                negative_deviation_percent: -7.0,
            }
        } else {
            // 0.4kV: +7% / -10%
            Self {
                rated_voltage_kv,
                positive_deviation_percent: 7.0,
                negative_deviation_percent: -10.0,
            }
        }
    }

    /// 检查电压是否在允许范围内
    pub fn check(&self, voltage_pu: f64) -> bool {
        let deviation = (voltage_pu - 1.0) * 100.0;
        deviation <= self.positive_deviation_percent && deviation >= self.negative_deviation_percent
    }
}

/// Loading limits for transformers and lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadingLimits {
    /// 正常运行负载率限值 (%)
    pub normal_percent: f64,
    /// 经济运行负载率范围 (min%, max%)
    pub economic_range: (f64, f64),
    /// N-1 事故短时负载率限值 (%)
    pub n1_emergency_percent: f64,
    /// 紧急负载率限值 (%)
    pub emergency_percent: f64,
}

impl LoadingLimits {
    /// 变压器负载率限值（按区域类型）
    pub fn for_transformer(area: SupplyAreaClass) -> Self {
        match area {
            SupplyAreaClass::A | SupplyAreaClass::B => Self::area_a_b(),
            _ => Self::area_c_d_e(),
        }
    }

    fn area_a_b() -> Self {
        Self {
            normal_percent: 70.0,
            economic_range: (40.0, 65.0),
            n1_emergency_percent: 130.0,
            emergency_percent: 140.0,
        }
    }

    fn area_c_d_e() -> Self {
        Self {
            normal_percent: 80.0,
            economic_range: (30.0, 70.0),
            n1_emergency_percent: 120.0,
            emergency_percent: 130.0,
        }
    }

    /// 线路负载率限值（含规划裕度）
    pub fn for_line() -> Self {
        Self {
            normal_percent: 85.0,
            economic_range: (50.0, 80.0),
            n1_emergency_percent: 100.0,
            emergency_percent: 110.0,
        }
    }
}

/// Supply radius limits per area class (km).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplyRadius {
    /// 电缆线路最大供电半径 (km)
    pub cable_max_km: f64,
    /// 架空线路最大供电半径 (km)
    pub overhead_max_km: f64,
}

impl SupplyRadius {
    /// 按供电区类型和电压等级获取供电半径限值
    pub fn for_area(area: SupplyAreaClass, voltage_kv: f64) -> Self {
        if voltage_kv >= 35.0 {
            // 35kV+ 供电半径较大
            Self {
                cable_max_km: match area {
                    SupplyAreaClass::A | SupplyAreaClass::B => 15.0,
                    SupplyAreaClass::C => 25.0,
                    SupplyAreaClass::D | SupplyAreaClass::E => 40.0,
                },
                overhead_max_km: match area {
                    SupplyAreaClass::A | SupplyAreaClass::B => 20.0,
                    SupplyAreaClass::C => 35.0,
                    SupplyAreaClass::D | SupplyAreaClass::E => 60.0,
                },
            }
        } else {
            // 10kV 供电半径
            Self {
                cable_max_km: match area {
                    SupplyAreaClass::A => 3.0,
                    SupplyAreaClass::B => 5.0,
                    SupplyAreaClass::C => 8.0,
                    SupplyAreaClass::D => 12.0,
                    SupplyAreaClass::E => 15.0,
                },
                overhead_max_km: match area {
                    SupplyAreaClass::A => 5.0,
                    SupplyAreaClass::B => 8.0,
                    SupplyAreaClass::C => 12.0,
                    SupplyAreaClass::D => 18.0,
                    SupplyAreaClass::E => 25.0,
                },
            }
        }
    }
}

/// Load model parameters for planning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModel {
    /// 默认功率因数
    pub default_power_factor: f64,
    /// 同时系数
    pub coincidence_factor: f64,
    /// 年负荷增长率 (%)
    pub annual_growth_percent: f64,
    /// 典型日最大负荷利用小时数 (h)
    pub max_load_hours: f64,
}

impl Default for LoadModel {
    fn default() -> Self {
        Self {
            default_power_factor: 0.9,
            coincidence_factor: 0.8,
            annual_growth_percent: 5.0,
            max_load_hours: 3500.0,
        }
    }
}

/// Renewable hosting capacity parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenewableHosting {
    /// 光伏接入电压等级选择阈值 (kW)
    pub pv_voltage_threshold_kw: f64,
    /// 光伏筛查限值（占变压器容量百分比）
    pub pv_screening_limit_percent: f64,
    /// 储能配置比例（光伏容量的百分比）
    pub storage_ratio_percent: f64,
}

impl Default for RenewableHosting {
    fn default() -> Self {
        Self {
            pv_voltage_threshold_kw: 8.0, // <8kW 接 0.4kV, >=8kW 接 10kV
            pv_screening_limit_percent: 25.0, // 25% of transformer capacity
            storage_ratio_percent: 20.0, // 20% of PV capacity
        }
    }
}

/// Energy storage planning scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageApplication {
    /// 削峰填谷
    PeakShaving,
    /// 光伏平滑
    PvSmoothing,
    /// 备用电源
    Backup,
    /// 电压支撑
    VoltageSupport,
}

/// Planning scenario types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanningScenario {
    /// 基准年最大负荷
    BaseYearPeak,
    /// 目标年最大负荷
    TargetYearPeak,
    /// 低负荷高光伏
    LowLoadHighPv,
    /// N-1 负荷转供
    N1Transfer,
    /// 短路电流最大/最小
    ShortCircuitMaxMin,
    /// 电动汽车晚高峰
    EvEveningPeak,
}

/// Candidate plan action types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateAction {
    /// 变压器增容
    TransformerUpgrade,
    /// 馈线加固
    FeederReinforcement,
    /// 新能源接入
    RenewableIntegration,
    /// 电动汽车接入
    EvIntegration,
}

/// Candidate plan for network expansion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidatePlan {
    pub action: CandidateAction,
    pub description: String,
    pub estimated_cost_million_cny: f64,
    pub trigger_condition: String,
}

/// Planning evaluator that generates candidate plans based on operating conditions.
pub struct PlanningEvaluator {
    pub area: SupplyAreaClass,
    pub voltage_kv: f64,
    pub load_model: LoadModel,
    pub renewable_hosting: RenewableHosting,
}

impl PlanningEvaluator {
    pub fn new(area: SupplyAreaClass, voltage_kv: f64) -> Self {
        Self {
            area,
            voltage_kv,
            load_model: LoadModel::default(),
            renewable_hosting: RenewableHosting::default(),
        }
    }

    /// Get voltage limits for this evaluator's voltage level.
    pub fn voltage_limits(&self) -> VoltageLimits {
        VoltageLimits::for_voltage_level(self.voltage_kv)
    }

    /// Get loading limits for transformers.
    pub fn transformer_loading_limits(&self) -> LoadingLimits {
        LoadingLimits::for_transformer(self.area)
    }

    /// Get loading limits for lines.
    pub fn line_loading_limits(&self) -> LoadingLimits {
        LoadingLimits::for_line()
    }

    /// Get supply radius limits.
    pub fn supply_radius(&self) -> SupplyRadius {
        SupplyRadius::for_area(self.area, self.voltage_kv)
    }

    /// Generate candidate plans based on current loading and conditions.
    pub fn generate_candidates(
        &self,
        current_loading_percent: f64,
        voltage_deviation_percent: f64,
        pv_penetration_percent: f64,
        ev_penetration_percent: f64,
    ) -> Vec<CandidatePlan> {
        let mut plans = Vec::new();
        let loading_limits = self.transformer_loading_limits();

        // 变压器增容：负载率超过正常限值
        if current_loading_percent > loading_limits.normal_percent {
            plans.push(CandidatePlan {
                action: CandidateAction::TransformerUpgrade,
                description: format!(
                    "变压器增容：当前负载率 {:.1}% 超过限值 {:.1}%",
                    current_loading_percent, loading_limits.normal_percent
                ),
                estimated_cost_million_cny: 2.5,
                trigger_condition: format!("loading > {:.0}%", loading_limits.normal_percent),
            });
        }

        // 馈线加固：电压偏差超标
        let v_limits = self.voltage_limits();
        if voltage_deviation_percent.abs() > v_limits.positive_deviation_percent {
            plans.push(CandidatePlan {
                action: CandidateAction::FeederReinforcement,
                description: format!(
                    "馈线加固：电压偏差 {:.1}% 超过限值 ±{:.0}%",
                    voltage_deviation_percent, v_limits.positive_deviation_percent
                ),
                estimated_cost_million_cny: 1.8,
                trigger_condition: format!("voltage_deviation > ±{:.0}%", v_limits.positive_deviation_percent),
            });
        }

        // 新能源接入：光伏渗透率超过筛查限值
        if pv_penetration_percent > self.renewable_hosting.pv_screening_limit_percent {
            let storage_size = pv_penetration_percent * self.renewable_hosting.storage_ratio_percent / 100.0;
            plans.push(CandidatePlan {
                action: CandidateAction::RenewableIntegration,
                description: format!(
                    "新能源接入：光伏渗透率 {:.1}% 超过筛查限值 {:.0}%，建议配置 {:.1}% 储能",
                    pv_penetration_percent,
                    self.renewable_hosting.pv_screening_limit_percent,
                    storage_size
                ),
                estimated_cost_million_cny: 3.2,
                trigger_condition: format!("pv_penetration > {:.0}%", self.renewable_hosting.pv_screening_limit_percent),
            });
        }

        // 电动汽车接入：EV 渗透率超过 15%
        if ev_penetration_percent > 15.0 {
            plans.push(CandidatePlan {
                action: CandidateAction::EvIntegration,
                description: format!(
                    "电动汽车接入：EV 渗透率 {:.1}% 超过 15%，需增容配变和有序充电",
                    ev_penetration_percent
                ),
                estimated_cost_million_cny: 1.5,
                trigger_condition: "ev_penetration > 15%".to_string(),
            });
        }

        plans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voltage_limits_10kv() {
        let limits = VoltageLimits::for_voltage_level(10.0);
        assert_eq!(limits.positive_deviation_percent, 7.0);
        assert!(limits.check(1.05)); // +5% within ±7%
        assert!(!limits.check(1.10)); // +10% exceeds ±7%
        assert!(limits.check(0.95)); // -5% within ±7%
    }

    #[test]
    fn test_voltage_limits_04kv() {
        let limits = VoltageLimits::for_voltage_level(0.4);
        assert_eq!(limits.positive_deviation_percent, 7.0);
        assert_eq!(limits.negative_deviation_percent, -10.0);
        assert!(limits.check(0.92)); // -8% within -10%
        assert!(!limits.check(0.85)); // -15% exceeds -10%
    }

    #[test]
    fn test_supply_area_n1_requirements() {
        assert_eq!(SupplyAreaClass::A.n1_pass_rate_percent(), 100.0);
        assert_eq!(SupplyAreaClass::C.n1_pass_rate_percent(), 90.0);
        assert_eq!(SupplyAreaClass::D.n1_pass_rate_percent(), 0.0);
    }

    #[test]
    fn test_supply_radius_10kv() {
        let radius_a = SupplyRadius::for_area(SupplyAreaClass::A, 10.0);
        assert_eq!(radius_a.cable_max_km, 3.0);
        let radius_d = SupplyRadius::for_area(SupplyAreaClass::D, 10.0);
        assert_eq!(radius_d.overhead_max_km, 18.0);
    }

    #[test]
    fn test_loading_limits_transformer() {
        let limits = LoadingLimits::for_transformer(SupplyAreaClass::A);
        assert_eq!(limits.normal_percent, 70.0);
        assert_eq!(limits.economic_range, (40.0, 65.0));
    }

    #[test]
    fn test_candidate_plan_generation() {
        let evaluator = PlanningEvaluator::new(SupplyAreaClass::B, 10.0);

        // 高负载 + 电压偏差 + 高光伏 + 高EV → 4 个候选方案
        let plans = evaluator.generate_candidates(85.0, 8.0, 30.0, 20.0);
        assert_eq!(plans.len(), 4);

        // 正常运行 → 无候选方案
        let plans_normal = evaluator.generate_candidates(50.0, 3.0, 10.0, 5.0);
        assert_eq!(plans_normal.len(), 0);
    }

    #[test]
    fn test_planning_evaluator_aggregation() {
        let evaluator = PlanningEvaluator::new(SupplyAreaClass::A, 10.0);
        let v_limits = evaluator.voltage_limits();
        let t_limits = evaluator.transformer_loading_limits();
        let radius = evaluator.supply_radius();

        assert_eq!(v_limits.rated_voltage_kv, 10.0);
        assert_eq!(t_limits.normal_percent, 70.0);
        assert_eq!(radius.cable_max_km, 3.0);
    }
}
