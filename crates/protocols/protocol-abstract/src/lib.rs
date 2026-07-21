//! EnerOS 协议抽象层（Protocol Abstraction Layer，v0.51.0）.
//!
//! 定义协议无关的统一访问接口，将 Modbus/IEC 104/CAN 等协议栈抽象为
//! [`PointAccess`] / [`ProtocolAdapter`] trait，并由 [`ProtocolManager`]
//! 统一管理多协议适配器与点路由。
//!
//! # 核心类型
//! - [`error::ProtocolError`] — 协议抽象层错误（9 变体）
//! - [`address::ProtocolAddress`] — 三协议统一地址（Modbus/Iec104/Can）
//! - [`mapping::ProtocolPointMapping`] — 点映射 + 工程量变换（scale/offset）
//! - [`config::ProtocolType`] — 协议类型（可作 `BTreeMap` key）
//! - [`config::AdapterConfig`] / [`config::DeviceConfig`] — 适配器/设备配置
//! - [`access::PointAccess`] — 统一点读写访问 trait
//! - [`adapter::ProtocolAdapter`] — 协议适配器 trait（继承 PointAccess + 生命周期）
//! - [`adapter::AdapterState`] — 适配器状态机
//! - [`manager::ProtocolManager`] — 多协议管理器 + 点路由
//!
//! # 与 v0.50.0 的关系
//!
//! 复用 `eneros-upa-model` 的 `DataPoint`/`PointId`/`DeviceId`/`PointValue`/
//! `PointType`/`PointQuality`/`DataSource` 类型（path 依赖）。
//!
//! # 偏差声明（D1~D7）
//!
//! | 偏差 | 说明 |
//! |------|------|
//! | **D1** | 不直接依赖 `eneros-modbus-*`/`eneros-iec104-*`/`eneros-can`（适配器实现为后续版本任务；本版本仅定义 trait + mock 适配器 + ProtocolManager）—— Simplicity First |
//! | **D2** | 不实现 `Send + Sync` 约束（蓝图 `PointAccess: Send + Sync` 为 std 约束；no_std 单线程无需） |
//! | **D3** | 不实现 `subscribe`/`unsubscribe` 回调（蓝图含此项，但 `Box<dyn Fn>` 在 no_std 无 `std::sync` 时复杂；变更上报改为 `poll()` 主动查询，订阅机制后置）—— Simplicity First |
//! | **D4** | 不使用 `Arc<RwLock<PointDatabase>>`（蓝图含此项；no_std 无 Arc/RwLock；ProtocolManager 持有 `BTreeMap<ProtocolType, Box<dyn ProtocolAdapter>>`） |
//! | **D5** | 时间戳用 `u64` 毫秒参数注入（与 v0.50.0 D1 一致） |
//! | **D6** | crate 放入 `crates/protocols/protocol-abstract/`（P1-F 协议栈最上一层） |
//! | **D7** | 不实现 `DeviceDriver` trait（协议抽象层非设备驱动，与 v0.48.0~v0.50.0 一致） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，外部依赖仅 `eneros-upa-model`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod access;
pub mod adapter;
pub mod address;
pub mod config;
pub mod error;
pub mod manager;
pub mod mapping;

#[cfg(test)]
pub mod mock;

// 重导出公共 API
pub use access::PointAccess;
pub use adapter::{AdapterState, ProtocolAdapter};
pub use address::ProtocolAddress;
pub use config::{AdapterConfig, DeviceConfig, ProtocolType};
pub use error::ProtocolError;
pub use manager::ProtocolManager;
pub use mapping::ProtocolPointMapping;

#[cfg(test)]
mod tests {
    //! 跨模块集成测试 — 覆盖 MockAdapter / ProtocolManager / 地址 / 映射全链路（T1~T12）.

    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use eneros_upa_model::{
        DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
    };

    use crate::access::PointAccess;
    use crate::adapter::{AdapterState, ProtocolAdapter};
    use crate::address::ProtocolAddress;
    use crate::config::{AdapterConfig, DeviceConfig, ProtocolType};
    use crate::error::ProtocolError;
    use crate::manager::ProtocolManager;
    use crate::mapping::ProtocolPointMapping;
    use crate::mock::MockAdapter;

    // ===== 测试辅助函数 =====

    /// 构造测试用 DataPoint（device_id/name/point_type/value 可定制）.
    fn make_point(point_id: PointId, device_id: DeviceId, value: PointValue) -> DataPoint {
        DataPoint {
            point_id,
            device_id,
            name: String::from("test_point"),
            description: None,
            point_type: PointType::Analog,
            value,
            quality: PointQuality::good(),
            timestamp_ms: 1000,
            source: DataSource::Internal,
            unit: None,
        }
    }

    /// 浮点近似比较（f64 不能直接 assert_eq）.
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ===== T1：MockAdapter read_point 正常 =====
    #[test]
    fn test_t1_mock_read_point_normal() {
        let mut adapter = MockAdapter::new(ProtocolType::ModbusRtu);
        adapter.set_point(1, make_point(1, 10, PointValue::Float(42.5)));

        let point = adapter.read_point(1).expect("read ok");
        assert_eq!(point.point_id, 1);
        assert_eq!(point.device_id, 10);
        assert_eq!(point.value, PointValue::Float(42.5));
    }

    // ===== T2：MockAdapter read_point 点不存在返回 PointNotFound =====
    #[test]
    fn test_t2_mock_read_point_not_found() {
        let mut adapter = MockAdapter::new(ProtocolType::ModbusTcp);
        // 不设置任何点
        let result = adapter.read_point(999);
        assert!(matches!(result, Err(ProtocolError::PointNotFound)));
    }

    // ===== T3：MockAdapter write_point 更新点值 =====
    #[test]
    fn test_t3_mock_write_point_updates_value() {
        let mut adapter = MockAdapter::new(ProtocolType::Iec104);
        adapter.set_point(5, make_point(5, 1, PointValue::Float(10.0)));

        // 写入新值
        adapter
            .write_point(5, PointValue::Float(99.0))
            .expect("write ok");

        // 读回验证
        let point = adapter.read_point(5).expect("read ok");
        assert_eq!(point.value, PointValue::Float(99.0));

        // 写入不存在的点返回错误
        let result = adapter.write_point(888, PointValue::Bool(true));
        assert_eq!(result, Err(ProtocolError::PointNotFound));
    }

    // ===== T4：MockAdapter read_points 批量读取 =====
    #[test]
    fn test_t4_mock_read_points_batch() {
        let mut adapter = MockAdapter::new(ProtocolType::Can);
        adapter.set_point(1, make_point(1, 1, PointValue::Float(1.0)));
        adapter.set_point(2, make_point(2, 1, PointValue::Float(2.0)));
        // point 3 不存在

        let results = adapter.read_points(&[1, 2, 3]);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(matches!(results[2], Err(ProtocolError::PointNotFound)));
        assert_eq!(results[0].as_ref().unwrap().value, PointValue::Float(1.0));
    }

    // ===== T5：MockAdapter read_device_points 按设备读取 =====
    #[test]
    fn test_t5_mock_read_device_points() {
        let mut adapter = MockAdapter::new(ProtocolType::ModbusRtu);
        adapter.set_point(1, make_point(1, 10, PointValue::Float(1.0)));
        adapter.set_point(2, make_point(2, 10, PointValue::Float(2.0)));
        adapter.set_point(3, make_point(3, 20, PointValue::Float(3.0)));

        // 设备 10 有 2 个点
        let pts = adapter.read_device_points(10).expect("read ok");
        assert_eq!(pts.len(), 2);

        // 设备 20 有 1 个点
        let pts = adapter.read_device_points(20).expect("read ok");
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].point_id, 3);

        // 设备 99 无点
        let pts = adapter.read_device_points(99).expect("read ok");
        assert!(pts.is_empty());
    }

    // ===== T6：ProtocolManager 注册适配器 + 路由 =====
    #[test]
    fn test_t6_manager_register_adapter_and_route() {
        let mut manager = ProtocolManager::new();
        assert_eq!(manager.adapter_count(), 0);

        let adapter = Box::new(MockAdapter::new(ProtocolType::ModbusRtu));
        manager.register_adapter(adapter);
        assert_eq!(manager.adapter_count(), 1);

        // 注册路由
        manager.register_route(100, ProtocolType::ModbusRtu);

        // 未注册协议类型的适配器状态为 None
        assert_eq!(manager.adapter_state(ProtocolType::Iec104), None);
        // 已注册的适配器状态可查（Uninitialized）
        assert_eq!(
            manager.adapter_state(ProtocolType::ModbusRtu),
            Some(AdapterState::Uninitialized)
        );
    }

    // ===== T7：ProtocolManager read_point 按路由转发 =====
    #[test]
    fn test_t7_manager_read_point_routes() {
        let mut manager = ProtocolManager::new();

        let mut adapter = MockAdapter::new(ProtocolType::ModbusTcp);
        adapter.set_point(42, make_point(42, 5, PointValue::Int(123)));
        manager.register_adapter(Box::new(adapter));
        manager.register_route(42, ProtocolType::ModbusTcp);

        // 路由命中 → 适配器内点存在
        let point = manager.read_point(42).expect("read ok");
        assert_eq!(point.point_id, 42);
        assert_eq!(point.value, PointValue::Int(123));

        // 路由未命中 → AdapterNotFound
        let result = manager.read_point(999);
        assert!(matches!(result, Err(ProtocolError::AdapterNotFound)));

        // 路由命中但适配器内点不存在 → PointNotFound
        manager.register_route(500, ProtocolType::ModbusTcp);
        let result = manager.read_point(500);
        assert!(matches!(result, Err(ProtocolError::PointNotFound)));
    }

    // ===== T8：ProtocolManager poll_all 轮询所有适配器 =====
    #[test]
    fn test_t8_manager_poll_all() {
        let mut manager = ProtocolManager::new();

        // 注册两个适配器（不同协议类型）
        let adapter_a = Box::new(MockAdapter::new(ProtocolType::ModbusRtu));
        let adapter_b = Box::new(MockAdapter::new(ProtocolType::Iec104));
        manager.register_adapter(adapter_a);
        manager.register_adapter(adapter_b);
        assert_eq!(manager.adapter_count(), 2);

        // poll_all 一次
        manager.poll_all(5000);

        // 两个适配器仍存在（poll 不改变状态，仅推进内部计数）
        assert_eq!(
            manager.adapter_state(ProtocolType::ModbusRtu),
            Some(AdapterState::Uninitialized)
        );
        assert_eq!(
            manager.adapter_state(ProtocolType::Iec104),
            Some(AdapterState::Uninitialized)
        );
    }

    // ===== T9：ProtocolAdapter 生命周期（init→start→poll→stop）=====
    #[test]
    fn test_t9_adapter_lifecycle() {
        let mut adapter = MockAdapter::new(ProtocolType::ModbusRtu);

        // 初始 Uninitialized
        assert_eq!(adapter.state(), AdapterState::Uninitialized);

        // 构造配置
        let config = AdapterConfig {
            name: String::from("modbus_rtu_0"),
            protocol_type: ProtocolType::ModbusRtu,
            device_configs: vec![DeviceConfig {
                device_id: 1,
                name: String::from("dev1"),
                address: ProtocolAddress::Modbus {
                    slave_addr: 1,
                    reg_addr: 0,
                    func_code: 3,
                },
            }],
        };

        // init → Initialized
        adapter.init(&config).expect("init ok");
        assert_eq!(adapter.state(), AdapterState::Initialized);

        // start → Running
        adapter.start().expect("start ok");
        assert_eq!(adapter.state(), AdapterState::Running);

        // poll 多次 → poll_count 递增，状态保持 Running
        assert_eq!(adapter.poll_count(), 0);
        adapter.poll(1000).expect("poll ok");
        adapter.poll(2000).expect("poll ok");
        adapter.poll(3000).expect("poll ok");
        assert_eq!(adapter.poll_count(), 3);
        assert_eq!(adapter.state(), AdapterState::Running);

        // stop → Stopped
        adapter.stop().expect("stop ok");
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    // ===== T10：ProtocolAddress 枚举构造与匹配 =====
    #[test]
    fn test_t10_protocol_address_construction() {
        let modbus_addr = ProtocolAddress::Modbus {
            slave_addr: 1,
            reg_addr: 40001,
            func_code: 3,
        };
        let iec104_addr = ProtocolAddress::Iec104 {
            common_addr: 1,
            ioa: 100,
            type_id: 13,
        };
        let can_addr = ProtocolAddress::Can {
            can_id: 0x123,
            start_byte: 0,
            length: 8,
        };

        // 匹配验证 + 字段断言（用 matches! 避免 panic!，符合 no_std 规范）
        assert!(matches!(
            modbus_addr,
            ProtocolAddress::Modbus {
                slave_addr: 1,
                reg_addr: 40001,
                func_code: 3,
            }
        ));
        assert!(matches!(
            iec104_addr,
            ProtocolAddress::Iec104 {
                common_addr: 1,
                ioa: 100,
                type_id: 13,
            }
        ));
        assert!(matches!(
            can_addr,
            ProtocolAddress::Can {
                can_id: 0x123,
                start_byte: 0,
                length: 8,
            }
        ));

        // 相等性
        let modbus_addr2 = ProtocolAddress::Modbus {
            slave_addr: 1,
            reg_addr: 40001,
            func_code: 3,
        };
        assert_eq!(modbus_addr, modbus_addr2);
        assert_ne!(modbus_addr, iec104_addr);
    }

    // ===== T11：ProtocolPointMapping 工程量转换（raw→engineering→raw）=====
    #[test]
    fn test_t11_point_mapping_conversion() {
        let mapping = ProtocolPointMapping {
            point_id: 1,
            device_id: 10,
            protocol_addr: ProtocolAddress::Modbus {
                slave_addr: 1,
                reg_addr: 0,
                func_code: 3,
            },
            data_type: PointType::Analog,
            scale: 0.1,
            offset: 10.0,
        };

        // raw=100 → engineering = 100 * 0.1 + 10.0 = 20.0
        let eng = mapping.to_engineering(100);
        assert!(approx_eq(eng, 20.0), "expected 20.0, got {}", eng);

        // engineering=20.0 → raw = (20.0 - 10.0) / 0.1 = 100
        let raw = mapping.from_engineering(20.0);
        assert_eq!(raw, 100);

        // 往返一致性
        let raw2 = mapping.from_engineering(mapping.to_engineering(250));
        assert_eq!(raw2, 250);

        // 负偏移场景
        let mapping2 = ProtocolPointMapping {
            point_id: 2,
            device_id: 10,
            protocol_addr: ProtocolAddress::Iec104 {
                common_addr: 1,
                ioa: 0,
                type_id: 13,
            },
            data_type: PointType::Analog,
            scale: 1.0,
            offset: -50.0,
        };
        // raw=100 → engineering = 100 * 1.0 + (-50.0) = 50.0
        assert!(approx_eq(mapping2.to_engineering(100), 50.0));
        // engineering=50.0 → raw = (50.0 - (-50.0)) / 1.0 = 100
        assert_eq!(mapping2.from_engineering(50.0), 100);
    }

    // ===== T12：多协议共存（两个 MockAdapter 不同 protocol_type）=====
    #[test]
    fn test_t12_multi_protocol_coexistence() {
        let mut manager = ProtocolManager::new();

        // ModbusRtu 适配器
        let mut modbus_adapter = MockAdapter::new(ProtocolType::ModbusRtu);
        modbus_adapter.set_point(1, make_point(1, 1, PointValue::Float(1.1)));

        // Iec104 适配器
        let mut iec104_adapter = MockAdapter::new(ProtocolType::Iec104);
        iec104_adapter.set_point(2, make_point(2, 2, PointValue::Float(2.2)));

        manager.register_adapter(Box::new(modbus_adapter));
        manager.register_adapter(Box::new(iec104_adapter));
        assert_eq!(manager.adapter_count(), 2);

        // 注册路由
        manager.register_route(1, ProtocolType::ModbusRtu);
        manager.register_route(2, ProtocolType::Iec104);

        // 分别通过路由读取两个协议的点
        let p1 = manager.read_point(1).expect("read modbus ok");
        assert_eq!(p1.value, PointValue::Float(1.1));

        let p2 = manager.read_point(2).expect("read iec104 ok");
        assert_eq!(p2.value, PointValue::Float(2.2));

        // 批量读取跨协议
        let results = manager.read_points(&[1, 2]);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());

        // 写入跨协议
        manager
            .write_point(1, PointValue::Float(9.9))
            .expect("write modbus ok");
        manager
            .write_point(2, PointValue::Float(8.8))
            .expect("write iec104 ok");

        // 验证写入生效
        let p1 = manager.read_point(1).expect("read ok");
        assert_eq!(p1.value, PointValue::Float(9.9));
        let p2 = manager.read_point(2).expect("read ok");
        assert_eq!(p2.value, PointValue::Float(8.8));

        // poll_all 轮询两个适配器
        manager.poll_all(10000);
    }

    // ===== 附加测试：write_points 批量写入 =====
    #[test]
    fn test_write_points_batch() {
        let mut adapter = MockAdapter::new(ProtocolType::ModbusRtu);
        adapter.set_point(1, make_point(1, 1, PointValue::Float(0.0)));
        adapter.set_point(2, make_point(2, 1, PointValue::Float(0.0)));

        let cmds: Vec<(PointId, PointValue)> = vec![
            (1, PointValue::Float(1.1)),
            (2, PointValue::Float(2.2)),
            (3, PointValue::Float(3.3)), // 不存在
        ];
        let results = adapter.write_points(&cmds);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert_eq!(results[2], Err(ProtocolError::PointNotFound));

        // 验证已写入
        assert_eq!(adapter.read_point(1).unwrap().value, PointValue::Float(1.1));
        assert_eq!(adapter.read_point(2).unwrap().value, PointValue::Float(2.2));
    }

    // ===== 附加测试：ProtocolManager Default =====
    #[test]
    fn test_manager_default() {
        let manager = ProtocolManager::default();
        assert_eq!(manager.adapter_count(), 0);
    }

    // ===== 附加测试：protocol_type() 返回正确类型 =====
    #[test]
    fn test_protocol_type_accessor() {
        let adapter = MockAdapter::new(ProtocolType::Can);
        assert_eq!(adapter.protocol_type(), ProtocolType::Can);
    }
}
