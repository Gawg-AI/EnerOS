//! 运行场景定义（21-B A1：normal / fatal-soc）。
//!
//! 场景 = 调度配置 + 系统状态 + 语义描述。参数可被 CLI 覆盖（禁止硬编码
//! 于装配路径之外）。

use eneros_energy_lp_model::config::ScheduleConfig;
use eneros_fast_path::state::RealtimeState;

/// 场景标识.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// 正常链路：谷电时段充电至 80%（商业意图），期望 sink 写入命令。
    Normal,
    /// Fatal 阻断：终值 SOC 强制 0.02（< 0.05 安全下限），期望
    /// `SafetyFatal` + sink 零写入。
    FatalSoc,
}

impl Scenario {
    /// 从 CLI 字符串解析场景名.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "normal" => Ok(Self::Normal),
            "fatal-soc" => Ok(Self::FatalSoc),
            other => Err(format!(
                "unknown scenario '{other}'; accepted: normal, fatal-soc"
            )),
        }
    }

    /// 构建场景配置（`--soc-min` / `--price` 覆盖可经 `apply` 调整）.
    pub fn config(self) -> ScheduleConfig {
        match self {
            // 谷电时段充电至 80%：前 48 时段低价（谷电）充电，后 48 时段
            // 高价放电；终值 SOC 约束 0.8 体现"充电至 80%"意图。
            Scenario::Normal => ScheduleConfig {
                soc_final: Some(0.8),
                price: {
                    let mut p = vec![0.3; 48];
                    p.extend(vec![0.9; 48]);
                    p
                },
                ..ScheduleConfig::default()
            },
            // Fatal 阻断：终值 SOC=0.02（< ElectricalSafetyRule 的 0.05
            // Fatal 阈值，electrical.rs:109-120），求解末时段必然触发 Fatal。
            Scenario::FatalSoc => ScheduleConfig {
                soc_min: 0.02,
                soc_final: Some(0.02),
                price: {
                    let mut p = vec![0.3; 48];
                    p.extend(vec![0.9; 48]);
                    p
                },
                ..ScheduleConfig::default()
            },
        }
    }

    /// 场景语义描述（证据落盘 / stdout 展示）.
    pub fn description(self) -> &'static str {
        match self {
            Scenario::Normal => "谷电时段充电至 80%（期望：安全校验通过，sink 写入充电命令）",
            Scenario::FatalSoc => {
                "终值 SOC=0.02 低于安全下限 0.05（期望：SafetyFatal 阻断，sink 零写入）"
            }
        }
    }

    /// 场景初始系统状态.
    pub fn state(self) -> RealtimeState {
        RealtimeState {
            system: eneros_safety_validator::state::SystemState::default(),
            current_price: 0.3,
            load_demand: None,
        }
    }
}

/// CLI 覆盖项（可选参数，None = 用场景默认值）.
#[derive(Debug, Clone, Copy)]
pub struct Overrides {
    /// 覆盖 `ScheduleConfig.soc_min`.
    pub soc_min: Option<f64>,
}

impl Overrides {
    pub fn apply(self, mut config: ScheduleConfig) -> ScheduleConfig {
        if let Some(soc_min) = self.soc_min {
            config.soc_min = soc_min;
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_and_unknown() {
        assert_eq!(Scenario::parse("normal").unwrap(), Scenario::Normal);
        assert_eq!(Scenario::parse("fatal-soc").unwrap(), Scenario::FatalSoc);
        assert!(Scenario::parse("bogus").is_err());
    }

    #[test]
    fn normal_scenario_charges_to_80_percent() {
        let config = Scenario::Normal.config();
        assert_eq!(config.soc_final, Some(0.8));
        // 谷电语义：前半段低价，后半段高价。
        assert!(config.price[0] < config.price[95]);
        assert_eq!(config.price.len(), 96);
    }

    #[test]
    fn fatal_scenario_forces_soc_below_safety_floor() {
        let config = Scenario::FatalSoc.config();
        // 终值 SOC 0.02 < ElectricalSafetyRule Fatal 阈值 0.05。
        assert_eq!(config.soc_final, Some(0.02));
        assert!(config.soc_final.unwrap() < 0.05);
        assert!(config.soc_min < 0.05);
    }

    #[test]
    fn overrides_apply_soc_min() {
        let config = Overrides { soc_min: Some(0.3) }.apply(Scenario::Normal.config());
        assert_eq!(config.soc_min, 0.3);
        assert_eq!(config.soc_final, Some(0.8));
    }
}
