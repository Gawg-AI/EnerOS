//! 路径选择器（D8/D10/D11）.

use core::time::Duration;

use crate::state::RealtimeState;

/// 路径类型（D8：派生 Debug + Clone + PartialEq，测试需要 == 比较）.
#[derive(Debug, Clone, PartialEq)]
pub enum PathType {
    /// 慢路径（LLM 路径，~2s）.
    SlowPath,
    /// 快路径（Solver only，<500ms）.
    FastPath,
}

/// 路径选择器.
///
/// 根据状态变化阈值与时间间隔决定走快/慢路径：
/// 1. 首次运行 → 慢路径（初始化基线）
/// 2. 距上次慢路径超过 `min_slow_path_interval` → 慢路径（周期性刷新）
/// 3. 电价变化 > `price_change_threshold` → 慢路径
/// 4. SOC 变化 > `soc_change_threshold`（百分比） → 慢路径
/// 5. 默认 → 快路径
pub struct PathSelector {
    /// 电价变化阈值（元/kWh）.
    pub price_change_threshold: f64,
    /// SOC 变化阈值（百分比，5.0 表示 5%）.
    pub soc_change_threshold: f64,
    /// 负荷变化阈值（kW）.
    pub load_change_threshold: f64,
    /// 上次慢路径时间戳（ms）（D10: 替代 Option<Instant>）.
    pub last_slow_path_ms: Option<u64>,
    /// 最小慢路径间隔（D11: core::time::Duration）.
    pub min_slow_path_interval: Duration,
    /// 上次状态快照.
    pub last_state: Option<RealtimeState>,
}

impl PathSelector {
    /// 创建默认路径选择器.
    pub fn new() -> Self {
        Self {
            price_change_threshold: 0.1,
            soc_change_threshold: 5.0,
            load_change_threshold: 20.0,
            last_slow_path_ms: None,
            min_slow_path_interval: Duration::from_secs(300),
            last_state: None,
        }
    }

    /// 选择路径.
    ///
    /// 1. 首次运行 → 走慢路径
    /// 2. 距上次慢路径超过 min_slow_path_interval → 走慢路径
    /// 3. 电价变化 > price_change_threshold → 走慢路径
    /// 4. SOC 变化 > soc_change_threshold/100.0 → 走慢路径
    /// 5. 默认走快路径
    pub fn select(&mut self, state: &RealtimeState, now_ms: u64) -> PathType {
        // 1. 首次运行走慢路径
        match self.last_slow_path_ms {
            None => {
                self.last_slow_path_ms = Some(now_ms);
                self.last_state = Some(state.clone());
                return PathType::SlowPath;
            }
            Some(last) => {
                // 2. 间隔超时走慢路径 (now_ms - last >= interval_ms)
                let interval_ms = self.min_slow_path_interval.as_millis() as u64;
                if now_ms.saturating_sub(last) >= interval_ms {
                    self.last_slow_path_ms = Some(now_ms);
                    self.last_state = Some(state.clone());
                    return PathType::SlowPath;
                }
            }
        }

        // 3. 检查状态变化
        if let Some(last_state) = &self.last_state {
            let price_delta = (state.current_price - last_state.current_price).abs();
            if price_delta > self.price_change_threshold {
                self.last_slow_path_ms = Some(now_ms);
                self.last_state = Some(state.clone());
                return PathType::SlowPath;
            }

            let soc_delta_pct = ((state.system.soc_pct - last_state.system.soc_pct).abs()) * 100.0;
            if soc_delta_pct > self.soc_change_threshold {
                self.last_slow_path_ms = Some(now_ms);
                self.last_state = Some(state.clone());
                return PathType::SlowPath;
            }
        }

        // 5. 默认走快路径
        self.last_state = Some(state.clone());
        PathType::FastPath
    }
}

impl Default for PathSelector {
    fn default() -> Self {
        Self::new()
    }
}
