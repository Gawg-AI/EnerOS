//! EnerOS 统一点表模型 UPA（Unified Point Abstraction，v0.50.0）.
//!
//! 定义统一数据点 [`DataPoint`] 与点表数据库 [`PointDatabase`]，将 Modbus/IEC 104/CAN
//! 等协议数据归一化为统一表示，为 v0.51.0 协议抽象层与 v0.52.0 四遥模型提供基础。
//!
//! # 核心类型
//! - [`point::DataPoint`] — 统一数据点（point_id/device_id/name/point_type/value/quality/timestamp_ms/source/unit）
//! - [`point::PointType`] — 点类型（Analog/Digital/Control/Setpoint/Counter）
//! - [`point::PointValue`] — 点值（Float/Int/Bool/Enum/String/Null）
//! - [`point::PointQuality`] — 品质标志（valid/invalid/questionable/substituted/overflow/blocked/outdated）
//! - [`point::DataSource`] — 数据来源（ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual）
//! - [`database::PointDatabase`] — 点表数据库（多索引：device/type/name）
//!
//! # 偏差声明（D1~D9）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 时间戳用 `u64` 毫秒参数注入（无 `MonotonicTime` 类型，与 v0.48.0 D3 一致；蓝图 `timestamp: MonotonicTime` 改为 `timestamp_ms: u64`） |
//! | **D2** | `PointDatabase` 不内置 `RwLock`（no_std 单线程使用；多线程场景由调用方用 `spin::RwLock` 包装）—— Simplicity First |
//! | **D3** | `next_id` 用普通 `u32` 自增字段（非 `AtomicU32`；所有方法 `&mut self`，无并发需求）—— Simplicity First |
//! | **D4** | 使用 `alloc::collections::BTreeMap` 替代 `std::collections::HashMap`（有序、no_std 友好、key 可推导） |
//! | **D5** | crate 放入 `crates/protocols/upa-model/`（P1-F 协议栈第八层，与 modbus/iec104 同级） |
//! | **D6** | 零外部依赖（纯数据模型，不耦合 eneros-modbus-*/eneros-iec104-*/eneros-can；协议适配在 v0.51.0 实现） |
//! | **D7** | `PointValue::Float` 用 `f64`（蓝图原样；`PointValue` 仅派生 `PartialEq` 不派生 `Eq`，因 f64 不实现 Eq） |
//! | **D8** | 不实现 `DeviceDriver` trait（数据模型非设备驱动，与 v0.48.0/v0.49.0 一致） |
//! | **D9** | `update()` 接受 `now_ms: u64` 参数用于设置时间戳（D1 时间注入） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，**零外部依赖**（D6）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod database;
pub mod point;

pub use database::PointDatabase;
pub use point::{DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue};

#[cfg(test)]
mod tests {
    //! 跨模块集成测试 — 覆盖 point + database 全链路（T1~T12）。

    use alloc::string::String;

    use crate::database::PointDatabase;
    use crate::point::{DataPoint, DataSource, PointQuality, PointType, PointValue};

    // ===== T1：注册点 → 验证 ID + 初始值 Null + 品质 invalid =====
    #[test]
    fn test_register_initial_state() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "temp", PointType::Analog, 1000);
        assert_eq!(id, 0);
        let p = db.get_by_id(id).expect("point exists");
        assert_eq!(p.point_id, 0);
        assert_eq!(p.device_id, 1);
        assert_eq!(p.name, "temp");
        assert_eq!(p.point_type, PointType::Analog);
        assert_eq!(p.value, PointValue::Null);
        assert_eq!(p.quality, PointQuality::invalid());
        assert_eq!(p.timestamp_ms, 1000);
        assert_eq!(p.source, DataSource::Internal);
        assert!(p.description.is_none());
        assert!(p.unit.is_none());
    }

    // ===== T2：更新点值 → 验证 value/quality/timestamp 更新 =====
    #[test]
    fn test_update_value() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "v", PointType::Analog, 100);
        let ok = db.update(id, PointValue::Float(42.5), PointQuality::good(), 2000);
        assert!(ok);
        let p = db.get_by_id(id).expect("exists");
        assert_eq!(p.value, PointValue::Float(42.5));
        assert_eq!(p.quality, PointQuality::good());
        assert_eq!(p.timestamp_ms, 2000);
        // immutable fields unchanged
        assert_eq!(p.point_id, id);
        assert_eq!(p.device_id, 1);
        assert_eq!(p.name, "v");
        assert_eq!(p.point_type, PointType::Analog);
    }

    // ===== T3：按 ID 查询存在/不存在 =====
    #[test]
    fn test_get_by_id() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "a", PointType::Digital, 0);
        assert!(db.get_by_id(id).is_some());
        assert!(db.get_by_id(999).is_none());
    }

    // ===== T4：按设备查询 → 多点返回 =====
    #[test]
    fn test_get_by_device() {
        let mut db = PointDatabase::new();
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        db.register(2, "c", PointType::Analog, 0);
        let pts = db.get_by_device(1);
        assert_eq!(pts.len(), 2);
        let pts2 = db.get_by_device(2);
        assert_eq!(pts2.len(), 1);
        let pts3 = db.get_by_device(999);
        assert!(pts3.is_empty());
    }

    // ===== T5：按类型查询 → 过滤正确 =====
    #[test]
    fn test_get_by_type() {
        let mut db = PointDatabase::new();
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        db.register(1, "c", PointType::Analog, 0);
        let analog = db.get_by_type(PointType::Analog);
        assert_eq!(analog.len(), 2);
        let digital = db.get_by_type(PointType::Digital);
        assert_eq!(digital.len(), 1);
        let control = db.get_by_type(PointType::Control);
        assert!(control.is_empty());
    }

    // ===== T6：按名称查询 → 精确匹配 =====
    #[test]
    fn test_get_by_name() {
        let mut db = PointDatabase::new();
        db.register(1, "temperature", PointType::Analog, 0);
        let p = db.get_by_name("temperature");
        assert!(p.is_some());
        assert_eq!(p.unwrap().name, "temperature");
        assert!(db.get_by_name("nonexistent").is_none());
    }

    // ===== T7：删除点 → 主存储 + 索引清理 =====
    #[test]
    fn test_remove() {
        let mut db = PointDatabase::new();
        let id = db.register(1, "x", PointType::Analog, 0);
        db.update(id, PointValue::Float(1.0), PointQuality::good(), 10);
        assert_eq!(db.count(), 1);
        assert!(db.remove(id));
        assert_eq!(db.count(), 0);
        assert!(db.get_by_id(id).is_none());
        // indices cleaned
        assert!(db.get_by_device(1).is_empty());
        assert!(db.get_by_type(PointType::Analog).is_empty());
        assert!(db.get_by_name("x").is_none());
        // remove non-existent
        assert!(!db.remove(999));
    }

    // ===== T8：PointValue 六种类型构造与比较 =====
    #[test]
    fn test_point_value_variants() {
        let f = PointValue::Float(1.25);
        let i = PointValue::Int(-5);
        let b = PointValue::Bool(true);
        let e = PointValue::Enum(2);
        let s = PointValue::String(String::from("hello"));
        let n = PointValue::Null;
        assert_eq!(f, PointValue::Float(1.25));
        assert_eq!(i, PointValue::Int(-5));
        assert_eq!(b, PointValue::Bool(true));
        assert_eq!(e, PointValue::Enum(2));
        assert_eq!(s, PointValue::String(String::from("hello")));
        assert_eq!(n, PointValue::Null);
        assert_ne!(f, n);
    }

    // ===== T9：PointQuality::good() / invalid() 构造 =====
    #[test]
    fn test_point_quality_constructors() {
        let g = PointQuality::good();
        assert!(g.valid);
        assert!(!g.invalid);
        assert!(!g.questionable);
        assert!(!g.substituted);
        assert!(!g.overflow);
        assert!(!g.blocked);
        assert!(!g.outdated);

        let inv = PointQuality::invalid();
        assert!(!inv.valid);
        assert!(inv.invalid);
        assert!(!inv.questionable);
    }

    // ===== T10：点 ID 自增（0/1/2）=====
    #[test]
    fn test_id_auto_increment() {
        let mut db = PointDatabase::new();
        let id0 = db.register(1, "a", PointType::Analog, 0);
        let id1 = db.register(1, "b", PointType::Analog, 0);
        let id2 = db.register(1, "c", PointType::Analog, 0);
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    // ===== T11：重复名称注册（name_index 覆盖）=====
    #[test]
    fn test_duplicate_name_register() {
        let mut db = PointDatabase::new();
        let id0 = db.register(1, "dup", PointType::Analog, 0);
        let id1 = db.register(2, "dup", PointType::Digital, 0);
        // name_index overwritten to point to id1 (latest)
        assert_eq!(db.get_by_name("dup").map(|p| p.point_id), Some(id1));
        // both points still exist in main storage
        assert!(db.get_by_id(id0).is_some());
        assert!(db.get_by_id(id1).is_some());
        assert_eq!(db.count(), 2);
    }

    // ===== T12：count() / list_all() =====
    #[test]
    fn test_count_and_list_all() {
        let mut db = PointDatabase::new();
        assert_eq!(db.count(), 0);
        assert!(db.list_all().is_empty());
        db.register(1, "a", PointType::Analog, 0);
        db.register(1, "b", PointType::Digital, 0);
        db.register(2, "c", PointType::Control, 0);
        assert_eq!(db.count(), 3);
        let all: Vec<&DataPoint> = db.list_all();
        assert_eq!(all.len(), 3);
        // sorted by PointId ascending (BTreeMap guarantees)
        assert_eq!(all[0].point_id, 0);
        assert_eq!(all[1].point_id, 1);
        assert_eq!(all[2].point_id, 2);
    }
}
