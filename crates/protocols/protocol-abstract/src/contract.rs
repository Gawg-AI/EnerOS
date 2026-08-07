//! seam 层共享契约测试套件（07 蓝图 §4.1 / ADR-0026 §5）。
//!
//! 定义 [`ProtocolAdapter`] 的统一契约测试：**mock 适配器与每个真实适配器
//! 跑同一套件**。适配器实现 crate（如 modbus-rtu、iec104-master）在 `#[cfg(test)]`
//! 中调用 [`run_adapter_contract`]，以证明其满足 seam 层契约。
//!
//! 契约只断言外部可观察行为（点读写结果、状态迁移、错误），不依赖适配器内部
//! 实现；不使用"方法被调用计数"替代结果断言（MASTER §0 规则 3/4）。

use alloc::vec;

use eneros_upa_model::{DeviceId, PointId, PointValue};

use crate::access::PointAccessExt;
use crate::adapter::{AdapterState, ProtocolAdapter};
use crate::config::{AdapterConfig, ProtocolType};
use crate::error::ProtocolError;

/// 运行协议适配器 seam 层契约测试。
///
/// # 参数
/// - `adapter`：被测适配器（mock 或真实）。
/// - `cfg`：适配器配置；`device_configs` 必须包含 `device_id` 对应的设备，
///   且该设备下可访问 `point_ids` 中的点。
/// - `expected_type`：适配器应返回的协议类型。
/// - `device_id`：`read_device_points` 断言使用的设备 ID。
/// - `point_ids`：适配器点表中必须存在的点 ID 列表（至少 2 个，以便批量断言）。
///
/// # Panics
/// 任一契约断言失败即 panic，测试直接失败。
#[allow(clippy::disallowed_macros)] // 测试辅助代码允许 panic 断言
pub fn run_adapter_contract<A: ProtocolAdapter + PointAccessExt>(
    adapter: &mut A,
    cfg: &AdapterConfig,
    expected_type: ProtocolType,
    device_id: DeviceId,
    point_ids: &[PointId],
) {
    assert!(
        point_ids.len() >= 2,
        "契约测试需要至少 2 个点 ID（批量断言用）"
    );

    // 1. 协议类型标识。
    assert_eq!(
        adapter.protocol_type(),
        expected_type,
        "契约：protocol_type() 必须返回预期类型"
    );

    // 2. 初始状态。
    assert_eq!(
        adapter.state(),
        AdapterState::Uninitialized,
        "契约：new 后必须为 Uninitialized"
    );

    // 3. init 成功并进入 Initialized。
    adapter.init(cfg).expect("契约：init(有效配置) 必须成功");
    assert_eq!(
        adapter.state(),
        AdapterState::Initialized,
        "契约：init 成功后必须为 Initialized"
    );

    // 4. 单点读取：每个注册点返回 DataPoint 且 point_id 匹配。
    for &id in point_ids {
        let dp = adapter
            .read_point(id)
            .unwrap_or_else(|e| panic!("契约：read_point({id}) 必须成功，got {e:?}"));
        assert_eq!(dp.point_id, id, "契约：返回的 point_id 必须与请求一致");
    }

    // 5. 批量读取：长度一致且全部成功。
    let batch = adapter.read_points(point_ids);
    assert_eq!(
        batch.len(),
        point_ids.len(),
        "契约：read_points 返回数量必须等于请求数量"
    );
    for (i, r) in batch.iter().enumerate() {
        assert!(r.is_ok(), "契约：read_points[{i}] 必须成功，got {r:?}");
    }

    // 6. 写点成功。
    adapter
        .write_point(point_ids[0], PointValue::Float(1.5))
        .expect("契约：write_point(可写点) 必须成功");

    // 7. 按设备读取：返回结果非空且包含注册点。
    let dev_points = adapter
        .read_device_points(device_id)
        .expect("契约：read_device_points(已注册设备) 必须成功");
    assert!(!dev_points.is_empty(), "契约：read_device_points 不得为空");
    assert!(
        dev_points.iter().any(|dp| dp.point_id == point_ids[0]),
        "契约：read_device_points 结果必须包含注册点 {}",
        point_ids[0]
    );

    // 8. 生命周期：start → Running、poll → Ok、stop → Stopped。
    adapter.start().expect("契约：start 必须成功");
    assert_eq!(
        adapter.state(),
        AdapterState::Running,
        "契约：start 后必须为 Running"
    );
    adapter
        .poll(1_700_000_000_000)
        .expect("契约：poll 必须成功");
    adapter.stop().expect("契约：stop 必须成功");
    assert_eq!(
        adapter.state(),
        AdapterState::Stopped,
        "契约：stop 后必须为 Stopped"
    );

    // 9. 负向：未注册点必须报错（fail-closed）。
    let missing: PointId = point_ids
        .iter()
        .copied()
        .max()
        .map(|m| m.wrapping_add(1))
        .unwrap_or(0);
    let result = adapter.read_point(missing);
    assert!(
        result.is_err(),
        "契约：read_point(未注册点) 必须返回 Err，got {result:?}"
    );
    assert_ne!(
        result.unwrap_err(),
        ProtocolError::AdapterNotFound,
        "契约：单适配器内未注册点不得误报 AdapterNotFound"
    );
}

/// 便捷包装：从配置构造适配器配置（供各适配器测试复用）。
pub fn simple_adapter_config(protocol_type: ProtocolType, device_id: DeviceId) -> AdapterConfig {
    AdapterConfig {
        name: alloc::string::String::from("contract-device"),
        protocol_type,
        device_configs: vec![crate::config::DeviceConfig {
            device_id,
            name: alloc::string::String::from("d0"),
            address: crate::address::ProtocolAddress::Modbus {
                slave_addr: 1,
                reg_addr: 0,
                func_code: 3,
            },
        }],
    }
}
