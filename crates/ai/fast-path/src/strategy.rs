//! 预计算策略表（D7：修复蓝图 bounds bug）.

use alloc::vec;
use alloc::vec::Vec;

use eneros_energy_lp_model::config::ScheduleConfig;

use crate::state::RealtimeState;

/// 预计算策略表（D7：修复蓝图 bounds bug）.
///
/// 蓝图使用 `vec![0.3, 0.6, 1.0]`（3 bounds → 4 桶）但矩阵只有 3 行，
/// `unwrap_or(len)` 会访问 `strategies[3]` 越界 panic。
/// 本版本改为 `vec![0.3, 0.7]`（2 bounds → 3 桶：谷/平/峰），3×3=9 策略。
///
/// 策略矩阵 `strategies[price_bucket][soc_bucket]`：
/// - `price_bucket` 0=谷 / 1=平 / 2=峰
/// - `soc_bucket` 0=低 / 1=中 / 2=高
pub struct StrategyTable {
    /// 电价分界线（谷/平、平/峰）.
    pub price_levels: Vec<f64>,
    /// SOC 分界线（低/中、中/高）.
    pub soc_levels: Vec<f64>,
    /// 策略矩阵（3×3）.
    pub strategies: Vec<Vec<ScheduleConfig>>,
}

impl StrategyTable {
    /// 创建策略表（3×3=9 策略）.
    ///
    /// 谷时（bucket 0）倾向充电（soc_final=0.8），
    /// 峰时（bucket 2）倾向放电（soc_final=0.3），
    /// 平时（bucket 1）自主调度（soc_final=None）。
    pub fn new(default: ScheduleConfig) -> Self {
        let price_levels = vec![0.3, 0.7]; // 谷/平分界，平/峰分界 → 3 桶
        let soc_levels = vec![0.3, 0.7]; // 低/中分界，中/高分界 → 3 桶

        let mut strategies = Vec::new();
        for price_bucket in 0..=price_levels.len() {
            // 0..=2 → 3 桶
            let mut row = Vec::new();
            for _ in 0..=soc_levels.len() {
                // 0..=2 → 3 桶
                let mut config = default.clone();
                match price_bucket {
                    0 => config.soc_final = Some(0.8), // 谷时：倾向充电
                    2 => config.soc_final = Some(0.3), // 峰时：倾向放电
                    _ => config.soc_final = None,      // 平时：自主调度
                }
                row.push(config);
            }
            strategies.push(row);
        }

        Self {
            price_levels,
            soc_levels,
            strategies,
        }
    }

    /// 根据实时状态查表获取基础配置.
    pub fn get_config(&self, state: &RealtimeState) -> ScheduleConfig {
        let price_idx = self
            .price_levels
            .iter()
            .position(|&b| state.current_price < b)
            .unwrap_or(self.price_levels.len());
        let soc_idx = self
            .soc_levels
            .iter()
            .position(|&b| state.system.soc_pct < b)
            .unwrap_or(self.soc_levels.len());
        self.strategies[price_idx][soc_idx].clone()
    }
}

impl Default for StrategyTable {
    fn default() -> Self {
        Self::new(ScheduleConfig::default())
    }
}
