//! 故障触发引擎（7 类触发条件 + 持续帧数语义 + 配置序优先级，D12）。
//!
//! - OverCurrent / OverVoltage / OverFrequency：`v > threshold`
//! - UnderVoltage：`v < threshold`
//! - RateOfChange：相邻帧差分 `|v - v_prev| > threshold`
//! - DigitalEvent：数字量上升沿（false → true）
//! - Manual：不自动触发（由 `FaultRecorder::start_recording()` 显式入口）
//!
//! 每条件按 `duration_ms × sample_rate / 1000`（最小 1 帧）折算所需连续命中帧数；
//! 连续命中达所需帧数即触发并复位计数；同帧多条件命中按配置顺序取首个（最小索引）。

use alloc::string::String;
use alloc::vec::Vec;

/// 触发类型（7 变体，蓝图 §4.4）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TriggerType {
    /// 过电流。
    OverCurrent,
    /// 过电压。
    OverVoltage,
    /// 低电压。
    UnderVoltage,
    /// 过频率。
    OverFrequency,
    /// 变化率（相邻帧差分绝对值）。
    RateOfChange,
    /// 数字量事件（上升沿）。
    DigitalEvent,
    /// 手动触发（不自动评估）。
    Manual,
}

/// 触发条件配置。
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerCondition {
    /// 触发类型。
    pub trigger_type: TriggerType,
    /// 阈值（含义随类型：电流/电压/频率/变化率；DigitalEvent/Manual 忽略）。
    pub threshold: f32,
    /// 持续时间（毫秒），折算为所需连续命中帧数（最小 1 帧）。
    pub duration_ms: u32,
    /// 绑定通道 ID（匹配 `ChannelConfig::channel_id`）。
    pub channel: String,
}

/// 触发引擎（crate 内部实现，D12）。
///
/// - `consec[i]`：条件 i 当前连续命中帧数
/// - `required[i]`：条件 i 触发所需连续命中帧数
pub(crate) struct TriggerEngine {
    conditions: Vec<TriggerCondition>,
    consec: Vec<u32>,
    required: Vec<u32>,
}

impl TriggerEngine {
    /// 构建引擎；按 `duration_ms × sample_rate / 1000`（最小 1 帧）折算所需帧数。
    pub(crate) fn new(conditions: Vec<TriggerCondition>, sample_rate: u32) -> Self {
        let required = conditions
            .iter()
            .map(|c| core::cmp::max(1, c.duration_ms as u64 * sample_rate as u64 / 1000) as u32)
            .collect();
        let consec = alloc::vec![0; conditions.len()];
        Self {
            conditions,
            consec,
            required,
        }
    }

    /// 评估一帧采样，返回本帧触发的条件索引（配置序最小者），无触发返回 `None`。
    ///
    /// `ch_idx`：通道名 → `(is_digital, idx)` 闭包（先查模拟通道命名空间，
    /// 再查数字通道命名空间，由调用方保证与 `analog`/`digital` 切片布局一致）。
    /// 通道未匹配视为不命中。所有条件的计数器每帧都会更新，仅返回最小索引。
    pub(crate) fn evaluate(
        &mut self,
        ch_idx: impl Fn(&str) -> Option<(bool, usize)>,
        analog: &[f32],
        prev_analog: &[f32],
        digital: &[bool],
        prev_digital: &[bool],
    ) -> Option<usize> {
        let mut fired: Option<usize> = None;
        for i in 0..self.conditions.len() {
            let cond = &self.conditions[i];
            let hit = match cond.trigger_type {
                TriggerType::Manual => false,
                TriggerType::DigitalEvent => match ch_idx(&cond.channel) {
                    Some((true, idx)) => idx < digital.len() && !prev_digital[idx] && digital[idx],
                    _ => false,
                },
                _ => match ch_idx(&cond.channel) {
                    Some((false, idx)) if idx < analog.len() => {
                        let v = analog[idx];
                        match cond.trigger_type {
                            TriggerType::OverCurrent
                            | TriggerType::OverVoltage
                            | TriggerType::OverFrequency => v > cond.threshold,
                            TriggerType::UnderVoltage => v < cond.threshold,
                            TriggerType::RateOfChange => {
                                (v - prev_analog[idx]).abs() > cond.threshold
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                },
            };
            if hit {
                self.consec[i] += 1;
            } else {
                self.consec[i] = 0;
            }
            if self.consec[i] >= self.required[i] {
                self.consec[i] = 0;
                if fired.is_none() {
                    fired = Some(i);
                }
            }
        }
        fired
    }

    /// 触发条件列表（供 `check_triggers` 取锁存触发引用）。
    pub(crate) fn conditions(&self) -> &[TriggerCondition] {
        &self.conditions
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::{TriggerCondition, TriggerEngine, TriggerType};

    fn cond(
        trigger_type: TriggerType,
        threshold: f32,
        duration_ms: u32,
        channel: &str,
    ) -> TriggerCondition {
        TriggerCondition {
            trigger_type,
            threshold,
            duration_ms,
            channel: String::from(channel),
        }
    }

    fn ia_idx(name: &str) -> Option<(bool, usize)> {
        if name == "Ia" {
            Some((false, 0))
        } else {
            None
        }
    }

    #[test]
    fn tg7_overcurrent_duration_frames() {
        // 4000Hz × 10ms → 需连续 40 帧
        let mut eng = TriggerEngine::new(
            alloc::vec![cond(TriggerType::OverCurrent, 100.0, 10, "Ia")],
            4000,
        );
        // 连续 39 帧超阈值：不触发
        for _ in 0..39 {
            let r = eng.evaluate(ia_idx, &[200.0], &[200.0], &[], &[]);
            assert_eq!(r, None);
        }
        // 回落：计数清零
        let r = eng.evaluate(ia_idx, &[10.0], &[200.0], &[], &[]);
        assert_eq!(r, None);
        // 再连续 39 帧仍不触发，第 40 帧触发
        for _ in 0..39 {
            let r = eng.evaluate(ia_idx, &[200.0], &[200.0], &[], &[]);
            assert_eq!(r, None);
        }
        let r = eng.evaluate(ia_idx, &[200.0], &[200.0], &[], &[]);
        assert_eq!(r, Some(0));
    }

    #[test]
    fn tg8_under_voltage_triggers_below_threshold() {
        let mut eng = TriggerEngine::new(
            alloc::vec![cond(TriggerType::UnderVoltage, 180.0, 0, "Ia")],
            1000,
        );
        assert_eq!(eng.evaluate(ia_idx, &[220.0], &[220.0], &[], &[]), None);
        assert_eq!(eng.evaluate(ia_idx, &[150.0], &[220.0], &[], &[]), Some(0));
    }

    #[test]
    fn tg9_rate_of_change_uses_adjacent_diff() {
        let mut eng = TriggerEngine::new(
            alloc::vec![cond(TriggerType::RateOfChange, 50.0, 0, "Ia")],
            1000,
        );
        // 相邻帧 10 → 80：|80-10| = 70 > 50，命中
        assert_eq!(eng.evaluate(ia_idx, &[80.0], &[10.0], &[], &[]), Some(0));
        // 平稳帧：|80-80| = 0，不命中
        assert_eq!(eng.evaluate(ia_idx, &[80.0], &[80.0], &[], &[]), None);
    }

    #[test]
    fn tg10_digital_event_rising_edge_only() {
        let idx = |name: &str| {
            if name == "CB" {
                Some((true, 0))
            } else {
                None
            }
        };
        let mut eng = TriggerEngine::new(
            alloc::vec![cond(TriggerType::DigitalEvent, 0.0, 0, "CB")],
            1000,
        );
        // 上升沿 false → true：命中
        assert_eq!(eng.evaluate(idx, &[], &[], &[true], &[false]), Some(0));
        // 持续 true：不再命中
        assert_eq!(eng.evaluate(idx, &[], &[], &[true], &[true]), None);
        // 下降沿：不命中
        assert_eq!(eng.evaluate(idx, &[], &[], &[false], &[true]), None);
    }

    #[test]
    fn tg11_manual_never_auto_triggers() {
        let mut eng =
            TriggerEngine::new(alloc::vec![cond(TriggerType::Manual, 0.0, 0, "Ia")], 1000);
        for _ in 0..100 {
            assert_eq!(eng.evaluate(ia_idx, &[9999.0], &[9999.0], &[], &[]), None);
        }
    }

    #[test]
    fn tg12_same_frame_multi_hit_returns_smallest_index() {
        let idx = |name: &str| match name {
            "Ia" => Some((false, 0)),
            "Ua" => Some((false, 1)),
            _ => None,
        };
        let mut eng = TriggerEngine::new(
            alloc::vec![
                cond(TriggerType::OverCurrent, 100.0, 0, "Ia"),
                cond(TriggerType::OverVoltage, 50.0, 0, "Ua"),
            ],
            1000,
        );
        // 同帧两条件均命中：返回最小索引 0
        assert_eq!(
            eng.evaluate(idx, &[200.0, 80.0], &[0.0, 0.0], &[], &[]),
            Some(0)
        );
    }

    #[test]
    fn tg13_counter_resets_after_trigger_and_can_fire_again() {
        let mut eng = TriggerEngine::new(
            alloc::vec![cond(TriggerType::OverCurrent, 100.0, 1, "Ia")],
            1000,
        );
        // 1000Hz × 1ms → 需 1 帧... duration=1ms → required = max(1, 1) = 1
        assert_eq!(eng.evaluate(ia_idx, &[200.0], &[0.0], &[], &[]), Some(0));
        // 复位后可再次触发
        assert_eq!(eng.evaluate(ia_idx, &[200.0], &[200.0], &[], &[]), Some(0));
    }
}
