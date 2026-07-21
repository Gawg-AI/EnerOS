//! GridSampler trait + MockGridSampler + 异常检测辅助函数.

use alloc::vec::Vec;

use crate::state::{DataQuality, GridState};
use crate::GridError;

/// 电网状态采样器接口.
///
/// 抽象电网数据采集源（RTU/IED/PMU 等），由具体实现提供 `now_ms` 时戳的采样结果。
pub trait GridSampler {
    /// 执行一次采样，返回 `now_ms` 时戳的电网状态.
    fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError>;
}

/// Mock 电网采样器（测试用）.
#[derive(Debug, Clone)]
pub struct MockGridSampler {
    /// 下一次采样返回的状态
    pub next_state: GridState,
    /// 是否模拟采样失败
    pub fail: bool,
}

impl MockGridSampler {
    /// 创建成功路径采样器，返回给定状态.
    pub fn new(state: GridState) -> Self {
        MockGridSampler {
            next_state: state,
            fail: false,
        }
    }

    /// 创建失败路径采样器（`sample` 恒返回 `Err(SampleFailed)`）.
    pub fn new_failing() -> Self {
        MockGridSampler {
            next_state: GridState::default(),
            fail: true,
        }
    }

    /// Builder：替换下一次采样返回的状态.
    pub fn with_state(mut self, state: GridState) -> Self {
        self.next_state = state;
        self
    }
}

impl GridSampler for MockGridSampler {
    fn sample(&mut self, now_ms: u64) -> Result<GridState, GridError> {
        if self.fail {
            return Err(GridError::SampleFailed);
        }
        let mut s = self.next_state;
        s.timestamp = now_ms;
        Ok(s)
    }
}

/// 校验电网频率/电压是否在正常范围.
///
/// 频率 ∈ [49.5, 50.5] Hz，电压 ∈ [200.0, 240.0] V。
pub fn is_valid_grid(freq: f32, voltage: f32) -> bool {
    (49.5..=50.5).contains(&freq) && (200.0..=240.0).contains(&voltage)
}

fn frequency_out_of_range(s: &GridState) -> bool {
    s.frequency < 49.5 || s.frequency > 50.5
}

fn voltage_out_of_range(s: &GridState) -> bool {
    s.voltage_a < 200.0 || s.voltage_a > 240.0
}

fn quality_invalid(s: &GridState) -> bool {
    s.quality == DataQuality::Invalid
}

/// 默认异常检测器集合（3 个：频率越限/电压越限/数据无效）.
pub fn default_anomaly_detectors() -> Vec<fn(&GridState) -> bool> {
    alloc::vec![
        frequency_out_of_range,
        voltage_out_of_range,
        quality_invalid
    ]
}
