//! SOE 事件触发器（D7/D10：不要求 Send+Sync）.

use alloc::collections::BTreeMap;
use alloc::format;

use eneros_telemetry_model::QualityFlag;
use eneros_upa_model::{DataPoint, PointId, PointQuality, PointType, PointValue};

use crate::event::{EventPriority, SoeEvent, SoeEventType};

/// 事件触发器 trait.
///
/// 不要求 `Send + Sync`（D7/D10：no_std 单线程）。
pub trait EventTrigger {
    /// 检查数据点变化是否触发事件.
    fn check(&self, old: &DataPoint, new: &DataPoint, now_ms: u64) -> Option<SoeEvent>;
}

/// 遥信变位触发器.
#[derive(Debug, Default, Clone)]
pub struct DigitalChangeTrigger;

impl DigitalChangeTrigger {
    /// 构造.
    pub fn new() -> Self {
        Self
    }
}

impl EventTrigger for DigitalChangeTrigger {
    fn check(&self, old: &DataPoint, new: &DataPoint, now_ms: u64) -> Option<SoeEvent> {
        if old.point_type != PointType::Digital {
            return None;
        }
        if old.value == new.value {
            return None;
        }
        Some(SoeEvent::new(
            new.point_id,
            new.device_id,
            SoeEventType::DigitalChange,
            old.value.clone(),
            new.value.clone(),
            point_quality_to_flag(new.quality),
            EventPriority::Medium,
            &format!("{}: {:?} -> {:?}", new.name, old.value, new.value),
            now_ms,
        ))
    }
}

/// 遥测越限触发器.
#[derive(Debug, Default, Clone)]
pub struct OverLimitTrigger {
    /// 越限配置：point_id -> (high_limit, low_limit).
    limits: BTreeMap<PointId, (f64, f64)>,
}

impl OverLimitTrigger {
    /// 构造.
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加越限配置.
    pub fn add_limit(&mut self, point_id: PointId, high: f64, low: f64) {
        self.limits.insert(point_id, (high, low));
    }
}

impl EventTrigger for OverLimitTrigger {
    fn check(&self, old: &DataPoint, new: &DataPoint, now_ms: u64) -> Option<SoeEvent> {
        if new.point_type != PointType::Analog {
            return None;
        }
        let value = match &new.value {
            PointValue::Float(v) => *v,
            _ => return None,
        };
        let (high, low) = self.limits.get(&new.point_id)?;
        // 旧值：Float 取值；Null（首次读取）视为"在限内"。
        let was_over = match &old.value {
            PointValue::Float(v) => *v > *high || *v < *low,
            _ => false,
        };
        let is_over = value > *high || value < *low;
        if !was_over && is_over {
            return Some(SoeEvent::new(
                new.point_id,
                new.device_id,
                SoeEventType::AnalogOverLimit,
                old.value.clone(),
                new.value.clone(),
                point_quality_to_flag(new.quality),
                EventPriority::High,
                &format!("{} over limit: {:.2}", new.name, value),
                now_ms,
            ));
        }
        if was_over && !is_over {
            return Some(SoeEvent::new(
                new.point_id,
                new.device_id,
                SoeEventType::AnalogRecovery,
                old.value.clone(),
                new.value.clone(),
                point_quality_to_flag(new.quality),
                EventPriority::Medium,
                &format!("{} recovery: {:.2}", new.name, value),
                now_ms,
            ));
        }
        None
    }
}

/// 将 `PointQuality`（标志位组合）映射为主导 `QualityFlag`.
fn point_quality_to_flag(q: PointQuality) -> QualityFlag {
    if q.invalid {
        QualityFlag::Invalid
    } else if q.questionable {
        QualityFlag::Questionable
    } else if q.substituted {
        QualityFlag::Substituted
    } else if q.blocked {
        QualityFlag::Blocked
    } else if q.overflow {
        QualityFlag::Overflow
    } else if q.outdated {
        QualityFlag::Outdated
    } else {
        QualityFlag::Good
    }
}
