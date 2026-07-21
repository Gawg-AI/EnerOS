//! 数据镜像模型 — TwinModel / DeviceTwin / MarketMirror / TwinSnapshot（v0.89.0）.
//!
//! - **D2**：`BTreeMap<u64, DeviceTwin>` 替代蓝图 `HashMap`（no_std 标准选择，有序遍历便于测试）。
//! - **D6**：复用 `eneros-device-agent::DeviceState`，不重复定义设备状态。
//! - **D7**：复用 `eneros-grid-agent::GridState`，不重复定义电网状态。
//! - **D9**：[`TwinSnapshot::summary_json`] 输出摘要 JSON（非全量模型序列化）。
//! - **D10**：[`MarketMirror`] 极简 2 字段（timestamp/current_price）替代 `MarketData`。

use alloc::collections::BTreeMap;
use alloc::string::String;

use eneros_device_agent::DeviceState;
use eneros_grid_agent::GridState;
use serde::Serialize;

/// 市场镜像（D10：极简 2 字段，仅保留最新价）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MarketMirror {
    /// 时间戳（ms，外部提供）.
    pub timestamp: u64,
    /// 当前电价（元/kWh）.
    pub current_price: f32,
}

/// 设备孪生（D3：u64 ID；D4：状态字段去重，全量含于 `DeviceState`）.
#[derive(Debug, Clone, Default)]
pub struct DeviceTwin {
    /// 设备 ID（由 topic 末段 `/power/state/battery/{id}` 解析）.
    pub device_id: u64,
    /// 设备状态（D6：复用 device-agent `DeviceState`）.
    pub state: DeviceState,
}

impl PartialEq for DeviceTwin {
    /// 逐字段相等（`DeviceState` 未派生 `PartialEq`，此处手动实现）.
    fn eq(&self, other: &Self) -> bool {
        self.device_id == other.device_id
            && self.state.soc == other.state.soc
            && self.state.voltage == other.state.voltage
            && self.state.current == other.state.current
            && self.state.temperature == other.state.temperature
            && self.state.power == other.state.power
            && self.state.online == other.state.online
            && self.state.last_update_ms == other.state.last_update_ms
    }
}

/// 孪生模型（设备表 + 电网状态 + 市场镜像 + 最后更新时间）.
#[derive(Debug, Clone, Default)]
pub struct TwinModel {
    /// 设备孪生表（D2：BTreeMap，按 device_id 有序遍历）.
    pub devices: BTreeMap<u64, DeviceTwin>,
    /// 电网状态（D7：复用 grid-agent `GridState`）.
    pub grid: GridState,
    /// 市场镜像（无市场数据时为 None）.
    pub market: Option<MarketMirror>,
    /// 最后一次成功应用更新的时间戳（ms，外部提供）.
    pub last_update: u64,
}

impl TwinModel {
    /// 设备数量.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }
}

/// 孪生快照（某一时刻模型的一致性 clone）.
#[derive(Debug, Clone)]
pub struct TwinSnapshot {
    /// 快照时间戳（== 模型 last_update）.
    pub timestamp: u64,
    /// 模型副本（与后续更新隔离）.
    pub model: TwinModel,
}

/// 摘要 DTO（D9：发布/查询用，serde 序列化）.
#[derive(Serialize)]
struct SummaryDto {
    timestamp: u64,
    last_update: u64,
    device_count: usize,
    grid_timestamp: u64,
    market_timestamp: Option<u64>,
}

impl TwinSnapshot {
    /// 生成摘要 JSON（D9）.
    ///
    /// serde_json 对纯数据 DTO 序列化不会失败；失败时兜底返回 `"{}"`。
    pub fn summary_json(&self) -> String {
        let dto = SummaryDto {
            timestamp: self.timestamp,
            last_update: self.model.last_update,
            device_count: self.model.device_count(),
            grid_timestamp: self.model.grid.timestamp,
            market_timestamp: self.model.market.as_ref().map(|m| m.timestamp),
        };
        match serde_json::to_string(&dto) {
            Ok(s) => s,
            Err(_) => String::from("{}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== T1: MarketMirror::default() 全零 + Copy 语义 =====
    #[test]
    fn t1_market_mirror_default_and_copy() {
        let a = MarketMirror::default();
        assert_eq!(a.timestamp, 0);
        assert!(a.current_price.abs() < 1e-9);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    // ===== T2: DeviceTwin::default() 全零 =====
    #[test]
    fn t2_device_twin_default() {
        let d = DeviceTwin::default();
        assert_eq!(d.device_id, 0);
        assert!(d.state.soc.abs() < 1e-12);
        assert!(!d.state.online);
        assert_eq!(d.state.last_update_ms, 0);
    }

    // ===== T3: TwinModel::default() 空模型 =====
    #[test]
    fn t3_twin_model_default() {
        let m = TwinModel::default();
        assert!(m.devices.is_empty());
        assert!(m.market.is_none());
        assert_eq!(m.last_update, 0);
        assert_eq!(m.device_count(), 0);
        assert!(m.grid.frequency.abs() < 1e-9);
    }

    // ===== T4: BTreeMap 有序（乱序插入 30/10/20 → 有序遍历 10/20/30）=====
    #[test]
    fn t4_devices_btreemap_ordered() {
        let mut m = TwinModel::default();
        for id in [30u64, 10, 20] {
            m.devices.insert(
                id,
                DeviceTwin {
                    device_id: id,
                    ..DeviceTwin::default()
                },
            );
        }
        let keys: alloc::vec::Vec<u64> = m.devices.keys().copied().collect();
        assert_eq!(keys, [10, 20, 30]);
    }

    // ===== T5: TwinSnapshot 构造 + clone 相等 =====
    #[test]
    fn t5_snapshot_clone_equal() {
        let mut model = TwinModel::default();
        model.devices.insert(1, DeviceTwin::default());
        model.last_update = 42;
        let snap = TwinSnapshot {
            timestamp: 42,
            model,
        };
        let snap2 = snap.clone();
        assert_eq!(snap.timestamp, snap2.timestamp);
        assert_eq!(snap.model.devices.len(), snap2.model.devices.len());
    }

    // ===== T6: summary_json 可解析 + device_count/last_update 正确 + market None 为 null =====
    #[test]
    fn t6_summary_json_parseable() {
        let mut model = TwinModel::default();
        model.devices.insert(1, DeviceTwin::default());
        model.devices.insert(2, DeviceTwin::default());
        model.last_update = 1234;
        let snap = TwinSnapshot {
            timestamp: 1234,
            model,
        };
        let s = snap.summary_json();
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["device_count"], 2);
        assert_eq!(v["last_update"], 1234);
        assert!(v["market_timestamp"].is_null());
    }
}
