//! 孪生镜像器 — TwinMirror / TwinError（v0.89.0）.
//!
//! 旁路订阅 `/power/state/*` 状态主题，将样本合并进 [`TwinModel`]
//! （蓝图 §4.4：订阅消息过期 → 不更新；模型字段缺失 → 保留旧值），
//! 并周期性向 `/power/twin/update` 发布摘要快照。
//!
//! - **D1**：sync `on_tick(now_ms)` 替代蓝图 `async run()`，外部调度驱动。
//! - **D5**：`Box<dyn DdsNode>` 持有节点，多 topic 对应多 reader。
//! - **D8**：`publish_interval_ms` + 外部 `now_ms` 驱动发布节拍。
//! - **D9**：发布摘要 JSON（[`PublishDto`]），非全量模型序列化。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_bus_dds::{DdsError, DdsNode, ParticipantId, QosPolicy, ReaderId, WriterId};
use serde::{Deserialize, Serialize};

use crate::model::{MarketMirror, TwinModel, TwinSnapshot};

/// 孪生错误（DDS 透传）.
#[derive(Debug)]
pub enum TwinError {
    /// DDS 中间件错误.
    Dds(DdsError),
}

impl From<DdsError> for TwinError {
    fn from(e: DdsError) -> Self {
        TwinError::Dds(e)
    }
}

/// 电网状态 payload（全字段可选，缺失保留旧值）.
#[derive(Debug, Deserialize)]
struct GridPayload {
    frequency: Option<f32>,
    voltage_a: Option<f32>,
    voltage_b: Option<f32>,
    voltage_c: Option<f32>,
    current_a: Option<f32>,
    current_b: Option<f32>,
    current_c: Option<f32>,
    active_power: Option<f32>,
    reactive_power: Option<f32>,
    power_factor: Option<f32>,
    timestamp: Option<u64>,
}

/// 设备状态 payload（全字段可选，缺失保留旧值）.
#[derive(Debug, Deserialize)]
struct DevicePayload {
    soc: Option<f64>,
    voltage: Option<f64>,
    current: Option<f64>,
    temperature: Option<f64>,
    power: Option<f64>,
    online: Option<bool>,
    last_update_ms: Option<u64>,
}

/// 市场价格 payload（必填字段，缺失即解析失败）.
#[derive(Debug, Deserialize)]
struct MarketPayload {
    timestamp: u64,
    current_price: f32,
}

/// 发布摘要 DTO（D9）.
#[derive(Serialize)]
struct PublishDto {
    timestamp: u64,
    last_update: u64,
    device_count: usize,
    grid_timestamp: u64,
    market_timestamp: Option<u64>,
    applied_count: u64,
    skipped_count: u64,
    published_count: u64,
}

/// Digital Twin 镜像器.
///
/// 字段全 pub，便于测试与外部观测（与项目既有 Agent 风格一致，无不变量需保护）。
pub struct TwinMirror {
    /// 孪生模型.
    pub model: TwinModel,
    /// DDS 节点（D5：Box<dyn DdsNode>，测试用 MockDdsNode）.
    pub node: Box<dyn DdsNode>,
    /// DDS participant 句柄.
    pub participant: ParticipantId,
    /// 订阅 reader 列表（topic, ReaderId）.
    pub readers: Vec<(String, ReaderId)>,
    /// `/power/twin/update` 发布 writer 句柄.
    pub writer: WriterId,
    /// 发布周期（ms）.
    pub publish_interval_ms: u64,
    /// 上次发布时间（ms）.
    pub last_publish_ms: u64,
    /// 成功应用更新计数.
    pub applied_count: u64,
    /// 跳过更新计数（解析失败/过期/未知 topic）.
    pub skipped_count: u64,
    /// 已发布快照计数.
    pub published_count: u64,
}

impl TwinMirror {
    /// 创建镜像器：participant → 逐 topic reader → `/power/twin/update` writer.
    ///
    /// 任一 DDS 调用失败返回 [`TwinError::Dds`]。计数器与 `last_publish_ms` 初始为 0。
    pub fn new(
        mut node: Box<dyn DdsNode>,
        topics: &[&str],
        publish_interval_ms: u64,
    ) -> Result<Self, TwinError> {
        let participant = node.create_participant()?;
        let mut readers = Vec::with_capacity(topics.len());
        for topic in topics {
            let reader = node.create_reader(participant, topic, QosPolicy::default())?;
            readers.push((String::from(*topic), reader));
        }
        let writer = node.create_writer(participant, "/power/twin/update", QosPolicy::default())?;
        Ok(Self {
            model: TwinModel::default(),
            node,
            participant,
            readers,
            writer,
            publish_interval_ms,
            last_publish_ms: 0,
            applied_count: 0,
            skipped_count: 0,
            published_count: 0,
        })
    }

    /// 跳过辅助：skipped_count + 1，返回 false.
    fn skip(&mut self) -> bool {
        self.skipped_count += 1;
        false
    }

    /// 应用成功辅助：applied_count + 1，last_update = now_ms，返回 true.
    fn applied(&mut self, now_ms: u64) -> bool {
        self.applied_count += 1;
        self.model.last_update = now_ms;
        true
    }

    /// 应用一条状态更新到孪生模型.
    ///
    /// 返回 true 表示已合并；false 表示跳过（解析失败/时间戳过期/未知 topic）。
    /// 合并语义（蓝图 §4.4）：payload 字段缺失保留旧值；payload 时间戳早于当前值丢弃。
    pub fn apply_update(&mut self, topic: &str, payload: &[u8], now_ms: u64) -> bool {
        if topic == "/power/state/grid" {
            let p: GridPayload = match serde_json::from_slice(payload) {
                Ok(p) => p,
                Err(_) => return self.skip(),
            };
            // 过期丢弃：payload 时间戳早于当前电网时间戳.
            if matches!(p.timestamp, Some(ts) if ts < self.model.grid.timestamp) {
                return self.skip();
            }
            if let Some(v) = p.frequency {
                self.model.grid.frequency = v;
            }
            if let Some(v) = p.voltage_a {
                self.model.grid.voltage_a = v;
            }
            if let Some(v) = p.voltage_b {
                self.model.grid.voltage_b = v;
            }
            if let Some(v) = p.voltage_c {
                self.model.grid.voltage_c = v;
            }
            if let Some(v) = p.current_a {
                self.model.grid.current_a = v;
            }
            if let Some(v) = p.current_b {
                self.model.grid.current_b = v;
            }
            if let Some(v) = p.current_c {
                self.model.grid.current_c = v;
            }
            if let Some(v) = p.active_power {
                self.model.grid.active_power = v;
            }
            if let Some(v) = p.reactive_power {
                self.model.grid.reactive_power = v;
            }
            if let Some(v) = p.power_factor {
                self.model.grid.power_factor = v;
            }
            if let Some(ts) = p.timestamp {
                self.model.grid.timestamp = ts;
            }
            self.applied(now_ms)
        } else if let Some(rest) = topic.strip_prefix("/power/state/battery/") {
            let id: u64 = match rest.parse() {
                Ok(id) => id,
                Err(_) => return self.skip(),
            };
            let p: DevicePayload = match serde_json::from_slice(payload) {
                Ok(p) => p,
                Err(_) => return self.skip(),
            };
            let entry = self.model.devices.entry(id).or_default();
            // 过期丢弃：payload 时间戳早于该设备已合并的时间戳.
            if matches!(p.last_update_ms, Some(ts) if ts < entry.state.last_update_ms) {
                return self.skip();
            }
            if let Some(v) = p.soc {
                entry.state.soc = v;
            }
            if let Some(v) = p.voltage {
                entry.state.voltage = v;
            }
            if let Some(v) = p.current {
                entry.state.current = v;
            }
            if let Some(v) = p.temperature {
                entry.state.temperature = v;
            }
            if let Some(v) = p.power {
                entry.state.power = v;
            }
            if let Some(v) = p.online {
                entry.state.online = v;
            }
            entry.state.last_update_ms = now_ms;
            self.applied(now_ms)
        } else if topic == "/power/market/price" {
            let p: MarketPayload = match serde_json::from_slice(payload) {
                Ok(p) => p,
                Err(_) => return self.skip(),
            };
            // 过期丢弃：payload 时间戳早于当前市场时间戳.
            let stale = self
                .model
                .market
                .as_ref()
                .is_some_and(|m| p.timestamp < m.timestamp);
            if stale {
                return self.skip();
            }
            self.model.market = Some(MarketMirror {
                timestamp: p.timestamp,
                current_price: p.current_price,
            });
            self.applied(now_ms)
        } else {
            self.skip()
        }
    }

    /// 生成当前模型的一致性快照（clone 语义，与后续更新隔离）.
    pub fn snapshot(&self) -> TwinSnapshot {
        TwinSnapshot {
            timestamp: self.model.last_update,
            model: self.model.clone(),
        }
    }

    /// 发布摘要快照到 `/power/twin/update`（D9）.
    pub fn publish(&mut self) -> Result<(), TwinError> {
        let snap = self.snapshot();
        let dto = PublishDto {
            timestamp: snap.timestamp,
            last_update: snap.model.last_update,
            device_count: snap.model.device_count(),
            grid_timestamp: snap.model.grid.timestamp,
            market_timestamp: snap.model.market.as_ref().map(|m| m.timestamp),
            applied_count: self.applied_count,
            skipped_count: self.skipped_count,
            published_count: self.published_count,
        };
        // serde_json 对纯数据 DTO 序列化不会失败；失败时跳过本次发布（非 DDS 错误）.
        let bytes = serde_json::to_vec(&dto).unwrap_or_default();
        if bytes.is_empty() {
            return Ok(());
        }
        self.node.write(self.writer, &bytes)?;
        self.published_count += 1;
        Ok(())
    }

    /// 周期节拍：拉取全部订阅样本 → 逐条合并 → 到期发布快照.
    ///
    /// 返回 Ok(true) 表示本拍发布了快照；Ok(false) 表示未到发布周期。
    pub fn on_tick(&mut self, now_ms: u64) -> Result<bool, TwinError> {
        // 先收集样本到 batch（避免 readers 迭代与 node.take 的借用冲突）.
        let mut batch: Vec<(String, Vec<u8>)> = Vec::new();
        for (topic, reader) in self.readers.iter() {
            let samples = self.node.take(*reader, 100)?;
            for s in samples {
                batch.push((topic.clone(), s.payload));
            }
        }
        for (topic, payload) in &batch {
            self.apply_update(topic, payload, now_ms);
        }
        if now_ms.saturating_sub(self.last_publish_ms) >= self.publish_interval_ms {
            self.publish()?;
            self.last_publish_ms = now_ms;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_agent_bus_dds::MockDdsNode;

    use super::*;

    /// 便捷构造：Mock 节点 + 指定 topics + 发布周期.
    fn make_mirror(topics: &[&str], interval_ms: u64) -> TwinMirror {
        TwinMirror::new(Box::new(MockDdsNode::new_default()), topics, interval_ms)
            .expect("mirror new")
    }

    // ===== T7: grid 全字段更新全部生效 =====
    #[test]
    fn t7_grid_full_update() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update(
            "/power/state/grid",
            br#"{"frequency":50.0,"voltage_a":220.0,"voltage_b":220.1,"voltage_c":219.9,"current_a":10.0,"current_b":10.1,"current_c":9.9,"active_power":120.0,"reactive_power":30.0,"power_factor":0.95,"timestamp":100}"#,
            1000,
        );
        assert!(ok);
        let g = &m.model.grid;
        assert!((g.frequency - 50.0).abs() < 1e-6);
        assert!((g.voltage_a - 220.0).abs() < 1e-6);
        assert!((g.voltage_b - 220.1).abs() < 1e-6);
        assert!((g.voltage_c - 219.9).abs() < 1e-6);
        assert!((g.current_a - 10.0).abs() < 1e-6);
        assert!((g.current_b - 10.1).abs() < 1e-6);
        assert!((g.current_c - 9.9).abs() < 1e-6);
        assert!((g.active_power - 120.0).abs() < 1e-6);
        assert!((g.reactive_power - 30.0).abs() < 1e-6);
        assert!((g.power_factor - 0.95).abs() < 1e-6);
        assert_eq!(g.timestamp, 100);
    }

    // ===== T8: grid 部分字段更新，未提及字段保留旧值 =====
    #[test]
    fn t8_grid_partial_update_keeps_old() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update(
            "/power/state/grid",
            br#"{"frequency":50.0,"timestamp":100}"#,
            1000
        ));
        assert!(m.apply_update(
            "/power/state/grid",
            br#"{"active_power":120.0,"timestamp":200}"#,
            2000
        ));
        assert!((m.model.grid.frequency - 50.0).abs() < 1e-6);
        assert!((m.model.grid.active_power - 120.0).abs() < 1e-6);
        assert_eq!(m.model.grid.timestamp, 200);
    }

    // ===== T9: grid 无效 JSON → 跳过 + 模型不变 =====
    #[test]
    fn t9_grid_invalid_json_skipped() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update("/power/state/grid", b"not json", 1000);
        assert!(!ok);
        assert_eq!(m.skipped_count, 1);
        assert!(m.model.grid.frequency.abs() < 1e-9);
        assert_eq!(m.model.last_update, 0);
    }

    // ===== T10: grid 过期时间戳 → 跳过 + 模型不变 =====
    #[test]
    fn t10_grid_stale_timestamp_skipped() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update(
            "/power/state/grid",
            br#"{"frequency":50.0,"timestamp":1000}"#,
            1000
        ));
        let ok = m.apply_update(
            "/power/state/grid",
            br#"{"frequency":49.0,"timestamp":500}"#,
            2000,
        );
        assert!(!ok);
        assert!((m.model.grid.frequency - 50.0).abs() < 1e-6);
        assert_eq!(m.model.grid.timestamp, 1000);
    }

    // ===== T11: grid 相同时间戳接受（非过期）=====
    #[test]
    fn t11_grid_same_timestamp_accepted() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update(
            "/power/state/grid",
            br#"{"frequency":50.0,"timestamp":1000}"#,
            1000
        ));
        let ok = m.apply_update(
            "/power/state/grid",
            br#"{"frequency":49.8,"timestamp":1000}"#,
            2000,
        );
        assert!(ok);
        assert!((m.model.grid.frequency - 49.8).abs() < 1e-6);
    }

    // ===== T12: grid.timestamp 更新为 payload 时间戳 =====
    #[test]
    fn t12_grid_timestamp_updated_from_payload() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/grid", br#"{"timestamp":777}"#, 1000));
        assert_eq!(m.model.grid.timestamp, 777);
    }

    // ===== T13: battery 新设备创建 =====
    #[test]
    fn t13_battery_new_device() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update(
            "/power/state/battery/7",
            br#"{"soc":0.8,"power":1.5}"#,
            2000,
        );
        assert!(ok);
        let d = m.model.devices.get(&7).expect("device 7");
        assert!((d.state.soc - 0.8).abs() < 1e-9);
        assert!((d.state.power - 1.5).abs() < 1e-9);
        assert_eq!(d.state.last_update_ms, 2000);
    }

    // ===== T14: battery 二次部分合并，soc 保留旧值 =====
    #[test]
    fn t14_battery_partial_merge_keeps_old() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/battery/1", br#"{"soc":0.8}"#, 1000));
        assert!(m.apply_update("/power/state/battery/1", br#"{"power":2.0}"#, 2000));
        let d = m.model.devices.get(&1).expect("device 1");
        assert!((d.state.soc - 0.8).abs() < 1e-9);
        assert!((d.state.power - 2.0).abs() < 1e-9);
    }

    // ===== T15: battery 无效 id → 跳过 =====
    #[test]
    fn t15_battery_invalid_id_skipped() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update("/power/state/battery/abc", br#"{"soc":0.8}"#, 1000);
        assert!(!ok);
        assert_eq!(m.skipped_count, 1);
        assert!(m.model.devices.is_empty());
    }

    // ===== T16: battery 过期（payload last_update_ms 早于已合并值）→ 跳过 + soc 保留 =====
    #[test]
    fn t16_battery_stale_skipped() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/battery/1", br#"{"soc":0.8}"#, 2000));
        let ok = m.apply_update(
            "/power/state/battery/1",
            br#"{"soc":0.1,"last_update_ms":1000}"#,
            3000,
        );
        assert!(!ok);
        let d = m.model.devices.get(&1).expect("device 1");
        assert!((d.state.soc - 0.8).abs() < 1e-9);
    }

    // ===== T17: battery online=true 合并 =====
    #[test]
    fn t17_battery_online_merge() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/battery/1", br#"{"online":true}"#, 1000));
        assert!(m.model.devices.get(&1).expect("device 1").state.online);
    }

    // ===== T18: applied/skipped 计数器（3 成功 + 2 失败）=====
    #[test]
    fn t18_applied_skipped_counters() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/grid", br#"{"frequency":50.0}"#, 100));
        assert!(m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100));
        assert!(m.apply_update(
            "/power/market/price",
            br#"{"timestamp":1,"current_price":0.5}"#,
            100
        ));
        assert!(!m.apply_update("/power/state/grid", b"bad", 100));
        assert!(!m.apply_update("/unknown", b"{}", 100));
        assert_eq!(m.applied_count, 3);
        assert_eq!(m.skipped_count, 2);
    }

    // ===== T19: battery 多设备相互独立 =====
    #[test]
    fn t19_battery_multi_device_independent() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 1000));
        assert!(m.apply_update("/power/state/battery/2", br#"{"soc":0.9}"#, 1000));
        assert_eq!(m.model.device_count(), 2);
        assert!((m.model.devices.get(&1).expect("d1").state.soc - 0.5).abs() < 1e-9);
        assert!((m.model.devices.get(&2).expect("d2").state.soc - 0.9).abs() < 1e-9);
    }

    // ===== T20: market 正常更新 =====
    #[test]
    fn t20_market_update() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update(
            "/power/market/price",
            br#"{"timestamp":100,"current_price":0.55}"#,
            1000,
        );
        assert!(ok);
        let mk = m.model.market.expect("market");
        assert_eq!(mk.timestamp, 100);
        assert!((mk.current_price - 0.55).abs() < 1e-6);
    }

    // ===== T21: market 缺必填字段 → 解析失败 + 旧值保留 =====
    #[test]
    fn t21_market_missing_field_keeps_old() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update(
            "/power/market/price",
            br#"{"timestamp":100,"current_price":0.55}"#,
            1000
        ));
        let ok = m.apply_update("/power/market/price", br#"{"timestamp":50}"#, 2000);
        assert!(!ok);
        let mk = m.model.market.expect("market");
        assert_eq!(mk.timestamp, 100);
        assert!((mk.current_price - 0.55).abs() < 1e-6);
    }

    // ===== T22: market 无效 JSON → 跳过 =====
    #[test]
    fn t22_market_invalid_json_skipped() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update("/power/market/price", b"not json", 1000);
        assert!(!ok);
        assert!(m.model.market.is_none());
        assert_eq!(m.skipped_count, 1);
    }

    // ===== T23: market 过期时间戳 → 跳过 + 旧值保留 =====
    #[test]
    fn t23_market_stale_skipped() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update(
            "/power/market/price",
            br#"{"timestamp":100,"current_price":0.55}"#,
            1000
        ));
        let ok = m.apply_update(
            "/power/market/price",
            br#"{"timestamp":50,"current_price":0.9}"#,
            2000,
        );
        assert!(!ok);
        let mk = m.model.market.expect("market");
        assert_eq!(mk.timestamp, 100);
        assert!((mk.current_price - 0.55).abs() < 1e-6);
    }

    // ===== T24: 未知 topic → 跳过 =====
    #[test]
    fn t24_unknown_topic_skipped() {
        let mut m = make_mirror(&[], 1000);
        let ok = m.apply_update("/power/alert/fault", br#"{"level":1}"#, 1000);
        assert!(!ok);
        assert_eq!(m.skipped_count, 1);
    }

    // ===== T25: apply 成功后 model.last_update == now_ms =====
    #[test]
    fn t25_apply_success_sets_last_update() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.apply_update("/power/state/grid", br#"{"frequency":50.0}"#, 4321));
        assert_eq!(m.model.last_update, 4321);
    }

    // ===== T26: applied + skipped == 总调用次数 =====
    #[test]
    fn t26_counters_sum_equals_calls() {
        let mut m = make_mirror(&[], 1000);
        let calls = [
            m.apply_update("/power/state/grid", br#"{"frequency":50.0}"#, 1),
            m.apply_update("/power/state/grid", b"bad", 2),
            m.apply_update("/power/state/battery/3", br#"{"soc":0.3}"#, 3),
            m.apply_update("/x", b"{}", 4),
            m.apply_update(
                "/power/market/price",
                br#"{"timestamp":1,"current_price":0.1}"#,
                5,
            ),
        ];
        assert_eq!(m.applied_count + m.skipped_count, calls.len() as u64);
    }

    // ===== T27: snapshot 字段与模型一致 =====
    #[test]
    fn t27_snapshot_fields_consistent() {
        let mut m = make_mirror(&[], 1000);
        m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100);
        m.apply_update("/power/state/grid", br#"{"frequency":50.0}"#, 100);
        let snap = m.snapshot();
        assert_eq!(snap.model.device_count(), 1);
        assert!((snap.model.grid.frequency - 50.0).abs() < 1e-6);
    }

    // ===== T28: snapshot.timestamp == model.last_update =====
    #[test]
    fn t28_snapshot_timestamp_equals_last_update() {
        let mut m = make_mirror(&[], 1000);
        m.apply_update("/power/state/grid", br#"{"frequency":50.0}"#, 999);
        let snap = m.snapshot();
        assert_eq!(snap.timestamp, m.model.last_update);
        assert_eq!(snap.timestamp, 999);
    }

    // ===== T29: 快照后修改原模型 → 快照不变（clone 独立性）=====
    #[test]
    fn t29_snapshot_independent_of_later_updates() {
        let mut m = make_mirror(&[], 1000);
        m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100);
        let snap = m.snapshot();
        m.apply_update("/power/state/battery/1", br#"{"soc":0.9}"#, 200);
        m.apply_update("/power/state/battery/2", br#"{"soc":0.1}"#, 200);
        assert_eq!(snap.model.device_count(), 1);
        assert!((snap.model.devices.get(&1).expect("d1").state.soc - 0.5).abs() < 1e-9);
    }

    // ===== T30: snapshot.summary_json() 可解析 + device_count 正确 =====
    #[test]
    fn t30_snapshot_summary_json() {
        let mut m = make_mirror(&[], 1000);
        m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100);
        m.apply_update("/power/state/battery/2", br#"{"soc":0.6}"#, 100);
        let snap = m.snapshot();
        let s = snap.summary_json();
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert_eq!(v["device_count"], 2);
    }

    // ===== T31: 端到端 — 外部 writer 经总线写入 grid JSON → on_tick 合并 =====
    #[test]
    fn t31_end_to_end_grid_via_bus() {
        let mut node = MockDdsNode::new_default();
        let ext_p = node.create_participant().expect("ext participant");
        let ext_w = node
            .create_writer(ext_p, "/power/state/grid", QosPolicy::default())
            .expect("ext writer");
        let mut m = TwinMirror::new(Box::new(node), &["/power/state/grid"], 1000).expect("mirror");
        m.node
            .write(
                ext_w,
                br#"{"frequency":50.0,"active_power":120.0,"timestamp":100}"#,
            )
            .expect("write");
        let published = m.on_tick(500).expect("tick");
        assert!(!published);
        assert!((m.model.grid.frequency - 50.0).abs() < 1e-6);
        assert!((m.model.grid.active_power - 120.0).abs() < 1e-6);
        assert_eq!(m.model.grid.timestamp, 100);
    }

    // ===== T32: take 消费语义 — 再次 on_tick 无新样本，applied_count 不增 =====
    #[test]
    fn t32_take_consumes_samples() {
        let mut node = MockDdsNode::new_default();
        let ext_p = node.create_participant().expect("ext participant");
        let ext_w = node
            .create_writer(ext_p, "/power/state/grid", QosPolicy::default())
            .expect("ext writer");
        let mut m = TwinMirror::new(Box::new(node), &["/power/state/grid"], 1000).expect("mirror");
        m.node
            .write(ext_w, br#"{"frequency":50.0}"#)
            .expect("write");
        m.on_tick(500).expect("tick1");
        assert_eq!(m.applied_count, 1);
        m.on_tick(600).expect("tick2");
        assert_eq!(m.applied_count, 1, "take 已消费样本，不应重复合并");
    }

    // ===== T33: 周期到达 → Ok(true) + published_count == 1 =====
    #[test]
    fn t33_on_tick_period_reached_publishes() {
        let mut m = make_mirror(&["/power/state/grid"], 1000);
        let published = m.on_tick(1000).expect("tick");
        assert!(published);
        assert_eq!(m.published_count, 1);
        assert_eq!(m.last_publish_ms, 1000);
    }

    // ===== T34: 周期未到 → Ok(false) + published_count == 0 =====
    #[test]
    fn t34_on_tick_period_not_reached() {
        let mut m = make_mirror(&[], 1000);
        let published = m.on_tick(500).expect("tick");
        assert!(!published);
        assert_eq!(m.published_count, 0);
    }

    // ===== T35: 两次周期 → published_count == 2 =====
    #[test]
    fn t35_two_periods_publish_twice() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.on_tick(1000).expect("tick1"));
        assert!(m.on_tick(2000).expect("tick2"));
        assert_eq!(m.published_count, 2);
    }

    // ===== T36: 多 reader — grid 与 battery 两个 topic 各自合并 =====
    #[test]
    fn t36_multi_reader_both_updated() {
        let mut node = MockDdsNode::new_default();
        let ext_p = node.create_participant().expect("ext participant");
        let wg = node
            .create_writer(ext_p, "/power/state/grid", QosPolicy::default())
            .expect("wg");
        let wb = node
            .create_writer(ext_p, "/power/state/battery/1", QosPolicy::default())
            .expect("wb");
        let mut m = TwinMirror::new(
            Box::new(node),
            &["/power/state/grid", "/power/state/battery/1"],
            1000,
        )
        .expect("mirror");
        m.node
            .write(wg, br#"{"frequency":50.5,"timestamp":10}"#)
            .expect("write grid");
        m.node.write(wb, br#"{"soc":0.66}"#).expect("write battery");
        m.on_tick(500).expect("tick");
        assert!((m.model.grid.frequency - 50.5).abs() < 1e-6);
        assert!((m.model.devices.get(&1).expect("d1").state.soc - 0.66).abs() < 1e-9);
    }

    // ===== T37: 节点已关闭 → new 返回 Err(TwinError::Dds) =====
    #[test]
    fn t37_shutdown_node_new_fails() {
        let mut node = MockDdsNode::new_default();
        node.shutdown().expect("shutdown");
        let r = TwinMirror::new(Box::new(node), &[], 1000);
        assert!(matches!(r, Err(TwinError::Dds(_))));
    }

    // ===== T38: 空 topics → new 成功 + readers 为空；on_tick 仅发布 =====
    #[test]
    fn t38_empty_topics_publish_only() {
        let mut m = make_mirror(&[], 1000);
        assert!(m.readers.is_empty());
        let published = m.on_tick(1000).expect("tick");
        assert!(published);
        assert_eq!(m.published_count, 1);
        assert_eq!(m.applied_count, 0);
    }

    // ===== T39: publish 广播 — 观察 reader 收到可解析摘要 =====
    #[test]
    fn t39_publish_payload_parseable() {
        let mut m = make_mirror(&[], 1000);
        let obs = m
            .node
            .create_reader(m.participant, "/power/twin/update", QosPolicy::default())
            .expect("observer reader");
        m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100);
        m.publish().expect("publish");
        let samples = m.node.take(obs, 10).expect("take");
        assert_eq!(samples.len(), 1);
        let v: serde_json::Value = serde_json::from_slice(&samples[0].payload).expect("valid json");
        assert!(v.get("published_count").is_some());
        assert!(v.get("device_count").is_some());
        assert_eq!(v["device_count"], 1);
        assert_eq!(v["applied_count"], 1);
    }

    // ===== T40: 2 设备 + 1 grid 后 publish → 摘要 device_count/grid_timestamp 正确 =====
    #[test]
    fn t40_publish_summary_counts() {
        let mut m = make_mirror(&[], 1000);
        m.apply_update("/power/state/battery/1", br#"{"soc":0.5}"#, 100);
        m.apply_update("/power/state/battery/2", br#"{"soc":0.6}"#, 100);
        m.apply_update(
            "/power/state/grid",
            br#"{"frequency":50.0,"timestamp":88}"#,
            100,
        );
        let obs = m
            .node
            .create_reader(m.participant, "/power/twin/update", QosPolicy::default())
            .expect("observer reader");
        m.publish().expect("publish");
        let samples = m.node.take(obs, 10).expect("take");
        assert_eq!(samples.len(), 1);
        let v: serde_json::Value = serde_json::from_slice(&samples[0].payload).expect("valid json");
        assert_eq!(v["device_count"], 2);
        assert_eq!(v["grid_timestamp"], 88);
    }
}
