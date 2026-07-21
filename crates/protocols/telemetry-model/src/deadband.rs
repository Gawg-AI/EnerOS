//! 死区过滤器（DeadbandFilter）.
//!
//! [`DeadbandFilter`] 为多点提供统一的死区过滤，基于 `BTreeMap` 维护每点状态，
//! 支持配置/过滤/强制上报/统计/移除。对应遥测上报的批量死区过滤场景（D6）。

use alloc::collections::BTreeMap;

use eneros_upa_model::PointId;

/// 单点死区状态（crate 内部）。
#[derive(Debug, Clone)]
pub(crate) struct PointDeadband {
    /// 死区值。
    deadband: f64,
    /// 上次上报值。
    last_reported: Option<f64>,
    /// 上报计数。
    report_count: u64,
    /// 跳过计数。
    skip_count: u64,
}

impl PointDeadband {
    fn new(deadband: f64) -> Self {
        Self {
            deadband,
            last_reported: None,
            report_count: 0,
            skip_count: 0,
        }
    }
}

/// 死区过滤器（批量多点的死区过滤）。
///
/// 使用 `BTreeMap` 存储每点配置（D6：no_std 无 HashMap，BTreeMap 友好）。
#[derive(Debug, Clone)]
pub struct DeadbandFilter {
    filters: BTreeMap<PointId, PointDeadband>,
}

impl DeadbandFilter {
    /// 创建空过滤器。
    pub fn new() -> Self {
        Self {
            filters: BTreeMap::new(),
        }
    }

    /// 配置某点的死区值（插入或更新）。
    pub fn configure(&mut self, point_id: PointId, deadband: f64) {
        if let Some(pd) = self.filters.get_mut(&point_id) {
            pd.deadband = deadband;
        } else {
            self.filters.insert(point_id, PointDeadband::new(deadband));
        }
    }

    /// 判断该点是否应上报当前值。
    ///
    /// - 点未配置：返回 `true`（不过滤）。
    /// - 首次上报（`last_reported` 为 `None`）：记录、`report_count += 1`，返回 `true`。
    /// - 变化超过死区：记录、`report_count += 1`，返回 `true`。
    /// - 否则：`skip_count += 1`，返回 `false`。
    pub fn should_report(&mut self, point_id: PointId, value: f64) -> bool {
        let pd = match self.filters.get_mut(&point_id) {
            Some(pd) => pd,
            None => return true,
        };
        match pd.last_reported {
            None => {
                pd.last_reported = Some(value);
                pd.report_count += 1;
                true
            }
            Some(last) => {
                if (value - last).abs() > pd.deadband {
                    pd.last_reported = Some(value);
                    pd.report_count += 1;
                    true
                } else {
                    pd.skip_count += 1;
                    false
                }
            }
        }
    }

    /// 强制上报：将 `last_reported` 设为当前值，`report_count += 1`（仅当点已配置）。
    pub fn force_report(&mut self, point_id: PointId, value: f64) {
        if let Some(pd) = self.filters.get_mut(&point_id) {
            pd.last_reported = Some(value);
            pd.report_count += 1;
        }
    }

    /// 返回该点的统计 `(report_count, skip_count)`（未配置返回 `None`）。
    pub fn get_stats(&self, point_id: PointId) -> Option<(u64, u64)> {
        self.filters
            .get(&point_id)
            .map(|pd| (pd.report_count, pd.skip_count))
    }

    /// 返回已配置点数。
    pub fn point_count(&self) -> usize {
        self.filters.len()
    }

    /// 移除某点配置，返回是否曾存在。
    pub fn remove(&mut self, point_id: PointId) -> bool {
        self.filters.remove(&point_id).is_some()
    }
}

impl Default for DeadbandFilter {
    fn default() -> Self {
        Self::new()
    }
}
