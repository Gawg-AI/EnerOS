//! EnerOS SOE 事件顺序记录引擎（Sequence of Events，v0.53.0）.
//!
//! 提供 ms 级时标事件记录、按时间戳排序（不乱序）、持久化（trait 抽象）、
//! 查询（按时间/设备/最新）、上传（trait 抽象）与过期清理能力，为电力系统
//! 故障分析提供精确的事件回放序列。
//!
//! # 核心类型
//! - [`event::SoeEvent`] — SOE 事件（event_id/timestamp_ms/point_id/event_type/...）
//! - [`event::SoeEventType`] — 事件类型（11 变体：DigitalChange/AnalogOverLimit/...）
//! - [`event::EventPriority`] — 事件优先级（Critical/High/Medium/Low）
//! - [`engine::SoeEngine`] — 事件引擎（队列/记录/持久化/上传/清理）
//! - [`storage::SoeStorage`] — 持久化存储 trait + [`storage::InMemorySoeStorage`] mock
//! - [`upload::UploadChannel`] — 上传通道 trait + [`upload::MockUploadChannel`] mock
//! - [`trigger::EventTrigger`] — 触发器 trait + [`trigger::DigitalChangeTrigger`] + [`trigger::OverLimitTrigger`]
//!
//! # 偏差声明（D1~D10）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 时间戳用 `u64` 毫秒参数注入（蓝图 `MonotonicTime`/`SystemTime` 在 no_std 不存在；与 v0.50.0~v0.52.0 D1 一致） |
//! | **D2** | crate 放入 `crates/protocols/soe-engine/`（P1-G 四遥与 SOE，与 upa-model/telemetry-model 同级） |
//! | **D3** | 仅依赖 `eneros-upa-model` + `eneros-telemetry-model`（复用 PointId/DeviceId/PointValue/QualityFlag） |
//! | **D4** | 持久化抽象为 `SoeStorage` trait + `InMemorySoeStorage` mock 实现（不直接依赖 v0.25.0 TSDB） |
//! | **D5** | 上传抽象为 `UploadChannel` trait + `MockUploadChannel` mock 实现（不直接依赖网络栈） |
//! | **D6** | 优先队列使用 `alloc::collections::BinaryHeap`（no_std 友好；蓝图 `PriorityQueue` 无标准实现） |
//! | **D7** | 不要求 `Send + Sync`（no_std 单线程；与 v0.51.0 D2 一致） |
//! | **D8** | 不使用 `AtomicU64`（no_std 单线程，`next_event_id: u64` 自增；蓝图 `AtomicU64` 改为 `u64`） |
//! | **D9** | `SystemTime::now()` 改为 `now_ms: u64` 参数注入（与 D1 一致） |
//! | **D10** | 不实现 `EventTrigger: Send + Sync`（与 D7 一致） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，依赖 `eneros-upa-model` + `eneros-telemetry-model`（纯数据模型）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod engine;
pub mod error;
pub mod event;
pub mod storage;
pub mod trigger;
pub mod upload;

pub use config::{SoeConfig, SoeStats};
pub use engine::SoeEngine;
pub use error::SoeError;
pub use event::{EventPriority, SoeEvent, SoeEventType};
pub use storage::{InMemorySoeStorage, SoeStorage};
pub use trigger::{DigitalChangeTrigger, EventTrigger, OverLimitTrigger};
pub use upload::{MockUploadChannel, UploadChannel};

#[cfg(test)]
mod tests {
    //! 集成测试 — 覆盖 SOE 引擎全链路（T1~T20）。

    use alloc::boxed::Box;
    use alloc::string::String;

    use eneros_telemetry_model::QualityFlag;
    use eneros_upa_model::{
        DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
    };

    use super::*;

    /// 构造遥信点.
    fn make_digital_point(
        point_id: PointId,
        device_id: DeviceId,
        value: bool,
        ts: u64,
    ) -> DataPoint {
        DataPoint {
            point_id,
            device_id,
            name: String::from("breaker"),
            description: None,
            point_type: PointType::Digital,
            value: PointValue::Bool(value),
            quality: PointQuality::good(),
            timestamp_ms: ts,
            source: DataSource::Internal,
            unit: None,
        }
    }

    /// 构造遥测点.
    fn make_analog_point(point_id: PointId, device_id: DeviceId, value: f64, ts: u64) -> DataPoint {
        DataPoint {
            point_id,
            device_id,
            name: String::from("voltage"),
            description: None,
            point_type: PointType::Analog,
            value: PointValue::Float(value),
            quality: PointQuality::good(),
            timestamp_ms: ts,
            source: DataSource::Internal,
            unit: Some(String::from("V")),
        }
    }

    /// 构造示例事件.
    fn make_event(priority: EventPriority, now_ms: u64) -> SoeEvent {
        SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Bool(false),
            PointValue::Bool(true),
            QualityFlag::Good,
            priority,
            "test",
            now_ms,
        )
    }

    // ===== T1：SoeEvent 构造与 is_critical =====
    #[test]
    fn test_t1_soeevent_construction_and_is_critical() {
        let critical = make_event(EventPriority::Critical, 1000);
        assert!(critical.is_critical());
        assert_eq!(critical.event_id, 0);
        assert_eq!(critical.timestamp_ms, 1000);
        assert_eq!(critical.system_time_ms, 1000);
        assert_eq!(critical.point_id, 1);
        assert_eq!(critical.device_id, 10);
        assert_eq!(critical.description, "test");

        let normal = make_event(EventPriority::Medium, 2000);
        assert!(!normal.is_critical());
    }

    // ===== T2：SoeEventType 11 变体覆盖 =====
    #[test]
    fn test_t2_soeevent_type_variants() {
        let types = [
            SoeEventType::DigitalChange,
            SoeEventType::AnalogOverLimit,
            SoeEventType::AnalogRecovery,
            SoeEventType::QualityChange,
            SoeEventType::ControlExecute,
            SoeEventType::ControlDone,
            SoeEventType::ControlFailed,
            SoeEventType::ManualSet,
            SoeEventType::CommLost,
            SoeEventType::CommRestore,
            SoeEventType::Custom(42),
        ];
        assert_eq!(types.len(), 11);
        assert_eq!(types[0], SoeEventType::DigitalChange);
        assert_eq!(types[10], SoeEventType::Custom(42));
    }

    // ===== T3：EventPriority 排序（Critical < High < Medium < Low）=====
    #[test]
    fn test_t3_event_priority_ordering() {
        assert!(EventPriority::Critical < EventPriority::High);
        assert!(EventPriority::High < EventPriority::Medium);
        assert!(EventPriority::Medium < EventPriority::Low);
        let mut sorted = [
            EventPriority::Low,
            EventPriority::Critical,
            EventPriority::Medium,
            EventPriority::High,
        ];
        sorted.sort();
        assert_eq!(sorted[0], EventPriority::Critical);
        assert_eq!(sorted[1], EventPriority::High);
        assert_eq!(sorted[2], EventPriority::Medium);
        assert_eq!(sorted[3], EventPriority::Low);
    }

    // ===== T4：SoeConfig 默认值 =====
    #[test]
    fn test_t4_soec_config_default() {
        let cfg = SoeConfig::default();
        assert_eq!(cfg.max_queue_size, 10_000);
        assert!(cfg.persist_enabled);
        assert_eq!(cfg.persist_batch_size, 100);
        assert_eq!(cfg.upload_interval_ms, 5_000);
        assert_eq!(cfg.retention_days, 90);
    }

    // ===== T5：InMemorySoeStorage append + query_by_time =====
    #[test]
    fn test_t5_storage_append_and_query_by_time() {
        let mut s = InMemorySoeStorage::new();
        let e1 = SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Bool(false),
            PointValue::Bool(true),
            QualityFlag::Good,
            EventPriority::Medium,
            "a",
            100,
        );
        let e2 = SoeEvent::new(
            2,
            10,
            SoeEventType::AnalogOverLimit,
            PointValue::Float(10.0),
            PointValue::Float(15.0),
            QualityFlag::Good,
            EventPriority::High,
            "b",
            200,
        );
        let e3 = SoeEvent::new(
            3,
            10,
            SoeEventType::QualityChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Invalid,
            EventPriority::Low,
            "c",
            300,
        );
        assert!(s.append(&[e1, e2, e3]).is_ok());
        assert_eq!(s.len(), 3);
        let r = s.query_by_time(150, 250);
        assert!(matches!(r, Ok(ref v) if v.len() == 1));
        let r = s.query_by_time(0, u64::MAX);
        assert!(matches!(r, Ok(ref v) if v.len() == 3));
        // 非法区间
        assert!(matches!(
            s.query_by_time(300, 100),
            Err(SoeError::InvalidArgument)
        ));
    }

    // ===== T6：InMemorySoeStorage query_by_device =====
    #[test]
    fn test_t6_storage_query_by_device() {
        let mut s = InMemorySoeStorage::new();
        let e1 = SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "a",
            100,
        );
        let e2 = SoeEvent::new(
            2,
            20,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "b",
            200,
        );
        let e3 = SoeEvent::new(
            3,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "c",
            300,
        );
        assert!(s.append(&[e1, e2, e3]).is_ok());
        let r = s.query_by_device(10, 100);
        assert!(matches!(r, Ok(ref v) if v.len() == 2));
        let r = s.query_by_device(20, 100);
        assert!(matches!(r, Ok(ref v) if v.len() == 1));
        let r = s.query_by_device(99, 100);
        assert!(matches!(r, Ok(ref v) if v.is_empty()));
        let r = s.query_by_device(10, 1);
        assert!(matches!(r, Ok(ref v) if v.len() == 1));
    }

    // ===== T7：InMemorySoeStorage get_latest =====
    #[test]
    fn test_t7_storage_get_latest() {
        let mut s = InMemorySoeStorage::new();
        let events: Vec<SoeEvent> = (0..5u64)
            .map(|i| {
                SoeEvent::new(
                    i as u32,
                    10,
                    SoeEventType::DigitalChange,
                    PointValue::Null,
                    PointValue::Null,
                    QualityFlag::Good,
                    EventPriority::Medium,
                    "x",
                    100 + i * 10,
                )
            })
            .collect();
        assert!(s.append(&events).is_ok());
        let latest = s.get_latest(3);
        assert_eq!(latest.len(), 3);
        // 时间戳为 100,110,120,130,140 → 最新 3 条为 120,130,140
        assert_eq!(latest[0].timestamp_ms, 120);
        assert_eq!(latest[1].timestamp_ms, 130);
        assert_eq!(latest[2].timestamp_ms, 140);
        assert_eq!(s.get_latest(0).len(), 0);
        assert_eq!(s.get_latest(100).len(), 5);
    }

    // ===== T8：InMemorySoeStorage mark_uploaded + get_unuploaded =====
    #[test]
    fn test_t8_storage_mark_uploaded_and_get_unuploaded() {
        let mut s = InMemorySoeStorage::new();
        let mut e1 = SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "a",
            100,
        );
        e1.event_id = 0;
        let mut e2 = SoeEvent::new(
            2,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "b",
            200,
        );
        e2.event_id = 1;
        let mut e3 = SoeEvent::new(
            3,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "c",
            300,
        );
        e3.event_id = 2;
        assert!(s.append(&[e1, e2, e3]).is_ok());
        let un = s.get_unuploaded(100);
        assert!(matches!(un, Ok(ref v) if v.len() == 3));
        // 标记 event_id 0 和 1 已上传
        assert!(s.mark_uploaded(&[0, 1]).is_ok());
        let un = s.get_unuploaded(100);
        assert!(matches!(un, Ok(ref v) if v.len() == 1));
    }

    // ===== T9：InMemorySoeStorage delete_before =====
    #[test]
    fn test_t9_storage_delete_before() {
        let mut s = InMemorySoeStorage::new();
        let e1 = SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "a",
            100,
        );
        let e2 = SoeEvent::new(
            2,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "b",
            200,
        );
        let e3 = SoeEvent::new(
            3,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "c",
            300,
        );
        assert!(s.append(&[e1, e2, e3]).is_ok());
        let deleted = s.delete_before(200);
        assert!(matches!(deleted, Ok(1)));
        assert_eq!(s.len(), 2);
        let deleted = s.delete_before(0);
        assert!(matches!(deleted, Ok(0)));
        assert_eq!(s.len(), 2);
    }

    // ===== T10：MockUploadChannel 上传统计 =====
    #[test]
    fn test_t10_mock_upload_channel_stats() {
        let mut mock = MockUploadChannel::new();
        assert!(!mock.is_connected());
        let e1 = SoeEvent::new(
            1,
            10,
            SoeEventType::DigitalChange,
            PointValue::Null,
            PointValue::Null,
            QualityFlag::Good,
            EventPriority::Medium,
            "a",
            100,
        );
        // 未连接时上传失败
        assert!(matches!(
            mock.upload(core::slice::from_ref(&e1)),
            Err(SoeError::UploadError)
        ));
        assert_eq!(mock.upload_count(), 0);
        // 连接后上传成功
        mock.set_connected(true);
        assert!(mock.is_connected());
        assert!(mock.upload(&[e1]).is_ok());
        assert_eq!(mock.upload_count(), 1);
        assert_eq!(mock.uploaded_events().len(), 1);
    }

    // ===== T11：DigitalChangeTrigger 检测变位 =====
    #[test]
    fn test_t11_digital_change_trigger_detects_change() {
        let trigger = DigitalChangeTrigger::new();
        let old = make_digital_point(1, 10, false, 100);
        let new = make_digital_point(1, 10, true, 200);
        let event = trigger.check(&old, &new, 200);
        assert!(event.is_some());
        let e = event.unwrap();
        assert_eq!(e.event_type, SoeEventType::DigitalChange);
        assert_eq!(e.priority, EventPriority::Medium);
        assert_eq!(e.timestamp_ms, 200);
        assert_eq!(e.point_id, 1);
        assert_eq!(e.device_id, 10);
    }

    // ===== T12：DigitalChangeTrigger 同值不触发 =====
    #[test]
    fn test_t12_digital_change_trigger_no_change() {
        let trigger = DigitalChangeTrigger::new();
        let old = make_digital_point(1, 10, true, 100);
        let new = make_digital_point(1, 10, true, 200);
        assert!(trigger.check(&old, &new, 200).is_none());
        // 非 Digital 类型不触发
        let old_analog = make_analog_point(1, 10, 10.0, 100);
        let new_analog = make_analog_point(1, 10, 20.0, 200);
        assert!(trigger.check(&old_analog, &new_analog, 200).is_none());
    }

    // ===== T13：OverLimitTrigger 越上限触发 =====
    #[test]
    fn test_t13_over_limit_trigger_high() {
        let mut trigger = OverLimitTrigger::new();
        trigger.add_limit(1, 12.0, 8.0);
        let old = make_analog_point(1, 10, 10.0, 100);
        let new = make_analog_point(1, 10, 15.0, 200);
        let event = trigger.check(&old, &new, 200);
        assert!(event.is_some());
        let e = event.unwrap();
        assert_eq!(e.event_type, SoeEventType::AnalogOverLimit);
        assert_eq!(e.priority, EventPriority::High);
    }

    // ===== T14：OverLimitTrigger 越下限触发 =====
    #[test]
    fn test_t14_over_limit_trigger_low() {
        let mut trigger = OverLimitTrigger::new();
        trigger.add_limit(1, 12.0, 8.0);
        let old = make_analog_point(1, 10, 10.0, 100);
        let new = make_analog_point(1, 10, 5.0, 200);
        let event = trigger.check(&old, &new, 200);
        assert!(event.is_some());
        let e = event.unwrap();
        assert_eq!(e.event_type, SoeEventType::AnalogOverLimit);
        assert_eq!(e.priority, EventPriority::High);
    }

    // ===== T15：OverLimitTrigger 恢复事件触发 =====
    #[test]
    fn test_t15_over_limit_trigger_recovery() {
        let mut trigger = OverLimitTrigger::new();
        trigger.add_limit(1, 12.0, 8.0);
        // 越限 → 正常
        let old = make_analog_point(1, 10, 15.0, 100);
        let new = make_analog_point(1, 10, 10.0, 200);
        let event = trigger.check(&old, &new, 200);
        assert!(event.is_some());
        let e = event.unwrap();
        assert_eq!(e.event_type, SoeEventType::AnalogRecovery);
        assert_eq!(e.priority, EventPriority::Medium);
        // 正常 → 正常 不触发
        let old2 = make_analog_point(1, 10, 10.0, 100);
        let new2 = make_analog_point(1, 10, 11.0, 200);
        assert!(trigger.check(&old2, &new2, 200).is_none());
        // 首次读取（Null）→ 越限 触发（Null 视为在限内）
        let old_null = make_analog_point(1, 10, 0.0, 100);
        let old_null = DataPoint {
            value: PointValue::Null,
            ..old_null
        };
        let new_over = make_analog_point(1, 10, 15.0, 200);
        let event = trigger.check(&old_null, &new_over, 200);
        assert!(event.is_some());
        assert_eq!(event.unwrap().event_type, SoeEventType::AnalogOverLimit);
    }

    // ===== T16：SoeEngine record_event 分配 event_id 递增 =====
    #[test]
    fn test_t16_engine_record_event_id_increment() {
        let config = SoeConfig::default();
        let storage: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine = SoeEngine::new(config, storage);
        let id0 = engine.record_event(make_event(EventPriority::Medium, 100));
        let id1 = engine.record_event(make_event(EventPriority::Medium, 200));
        let id2 = engine.record_event(make_event(EventPriority::Medium, 300));
        assert!(matches!(id0, Ok(0)));
        assert!(matches!(id1, Ok(1)));
        assert!(matches!(id2, Ok(2)));
        assert_eq!(engine.stats().total_events, 3);
    }

    // ===== T17：SoeEngine 事件不乱序（乱序入队，按时间戳出队）=====
    #[test]
    fn test_t17_engine_events_ordered_by_timestamp() {
        let config = SoeConfig {
            persist_batch_size: 3,
            ..SoeConfig::default()
        };
        let storage: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine = SoeEngine::new(config, storage);
        // 乱序入队：t=300, t=100, t=200
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 300))
            .is_ok());
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 100))
            .is_ok());
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 200))
            .is_ok());
        // 第 3 条触发自动持久化（batch_size=3）
        let r = engine.query_by_time(0, u64::MAX);
        assert!(matches!(r, Ok(ref v) if v.len() == 3));
        let events = r.unwrap();
        assert_eq!(events[0].timestamp_ms, 100);
        assert_eq!(events[1].timestamp_ms, 200);
        assert_eq!(events[2].timestamp_ms, 300);
        assert_eq!(engine.stats().persisted_events, 3);
    }

    // ===== T18：SoeEngine process_point_change 触发多事件 =====
    #[test]
    fn test_t18_engine_process_point_change_multiple_triggers() {
        let config = SoeConfig::default();
        let storage: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine = SoeEngine::new(config, storage);
        // 两个变位触发器，同一次变位产生两条事件
        engine.add_trigger(Box::new(DigitalChangeTrigger::new()));
        engine.add_trigger(Box::new(DigitalChangeTrigger::new()));
        let old = make_digital_point(1, 10, false, 100);
        let new = make_digital_point(1, 10, true, 200);
        let ids = engine.process_point_change(&old, &new, 200);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], 0);
        assert_eq!(ids[1], 1);
        assert_eq!(engine.stats().total_events, 2);
    }

    // ===== T19：SoeEngine upload_events 流程 =====
    #[test]
    fn test_t19_engine_upload_events() {
        let config = SoeConfig {
            persist_batch_size: 2,
            ..SoeConfig::default()
        };
        let storage: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine = SoeEngine::new(config, storage);
        let mut mock = MockUploadChannel::new();
        mock.set_connected(true);
        engine.set_upload_channel(Box::new(mock));
        // 记录 2 条，触发自动持久化
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 100))
            .is_ok());
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 200))
            .is_ok());
        assert_eq!(engine.stats().persisted_events, 2);
        // 上传
        let uploaded = engine.upload_events();
        assert!(matches!(uploaded, Ok(2)));
        assert_eq!(engine.stats().uploaded_events, 2);
        // 再次上传应无未上传事件
        let uploaded = engine.upload_events();
        assert!(matches!(uploaded, Ok(0)));
        // 未连接时返回 0
        let config2 = SoeConfig {
            persist_batch_size: 2,
            ..SoeConfig::default()
        };
        let storage2: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine2 = SoeEngine::new(config2, storage2);
        engine2.set_upload_channel(Box::new(MockUploadChannel::new()));
        assert!(matches!(engine2.upload_events(), Ok(0)));
    }

    // ===== T20：SoeEngine cleanup_expired 清理过期事件 =====
    #[test]
    fn test_t20_engine_cleanup_expired() {
        let config = SoeConfig {
            persist_batch_size: 2,
            retention_days: 1, // 1 天 = 86_400_000 ms
            ..SoeConfig::default()
        };
        let storage: Box<dyn SoeStorage> = Box::new(InMemorySoeStorage::new());
        let mut engine = SoeEngine::new(config, storage);
        // 记录 2 条旧事件（t=1000, t=2000），自动持久化
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 1000))
            .is_ok());
        assert!(engine
            .record_event(make_event(EventPriority::Medium, 2000))
            .is_ok());
        assert_eq!(engine.stats().persisted_events, 2);
        // now = 1 天 + 5000ms → cutoff = 5000，t=1000/2000 均过期
        let now_ms = 86_400_000 + 5_000;
        let deleted = engine.cleanup_expired(now_ms);
        assert!(matches!(deleted, Ok(2)));
        // 清理后查询应为空
        let r = engine.query_by_time(0, u64::MAX);
        assert!(matches!(r, Ok(ref v) if v.is_empty()));
    }
}
