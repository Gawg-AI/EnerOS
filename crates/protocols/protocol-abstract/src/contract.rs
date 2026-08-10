//! seam 层共享契约测试套件（07 蓝图 §4.1 / ADR-0026 §5 / 蓝图/24 §8.2 第一层）。
//!
//! 定义 [`ProtocolAdapter`] 的统一契约测试：**mock 适配器与每个真实适配器
//! 跑同一套件**。适配器实现 crate（如 modbus-rtu、iec104-master）在 `#[cfg(test)]`
//! 中调用 [`run_adapter_contract`]，以证明其满足 seam 层契约。
//!
//! 蓝图/24 D11 第一层：契约套件增加 [`AuthorizedWriter`] 约束与四类写用例
//! （授权落写 / 不匹配拒绝 / 篡改值拒绝 / 读基线防回归）。
//!
//! 契约只断言外部可观察行为（点读写结果、状态迁移、错误），不依赖适配器内部
//! 实现；不使用"方法被调用计数"替代结果断言（MASTER §0 规则 3/4）。

#[cfg(any(test, feature = "mock"))]
use alloc::vec;

// 29 GV-02：契约套件整体为测试设施（消费方全部在 cfg(test) 内），生产构建
// 不编译本模块体——导入随门控一并收紧，避免生产构建 unused 警告。
#[cfg(any(test, feature = "mock"))]
use eneros_telemetry_model::command::AuthorizedCommand;
#[cfg(any(test, feature = "mock"))]
use eneros_upa_model::{DeviceId, PointId, PointValue};

#[cfg(any(test, feature = "mock"))]
use crate::access::{AuthorizedWriter, PointAccessExt, WriteError};
#[cfg(any(test, feature = "mock"))]
use crate::adapter::{AdapterState, ProtocolAdapter};
#[cfg(any(test, feature = "mock"))]
use crate::config::{AdapterConfig, ProtocolType};
#[cfg(any(test, feature = "mock"))]
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
/// # 写用例说明（蓝图/24 §8.2 第一层）
///
/// 契约内的授权写使用 [`authorized_command`] 构造的 `AuthorizedCommand`
/// （device_id = `device_id` 参数、point_id = 目标点、command = Single(On)）。
/// "篡改值拒绝"用例的适配器层判据：**载荷值类型与点类型矛盾**（如对
/// Analog 点写 Bool）——类型级不一致是适配器可检测的篡改形态；语义级
/// 矛盾（值域/开关语义）归 ControlChain 与候选 5，不在 seam 契约范围。
///
/// # Panics
/// 任一契约断言失败即 panic，测试直接失败。
#[allow(clippy::disallowed_macros)] // 测试辅助代码允许 panic 断言
/// 29 GV-02：契约套件全部消费方在 `#[cfg(test)]` 模块内（grep 核实 modbus-tcp/
/// rtu/iec104-master adapter.rs + 本 crate lib.rs tests），生产构建无需编译
/// ——随 authorized_command 一并 mock/test 门控（S2 范式）。
#[cfg(any(test, feature = "mock"))]
pub fn run_adapter_contract<A: ProtocolAdapter + PointAccessExt + AuthorizedWriter>(
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

    // 4. 单点读取：每个注册点返回 DataPoint 且 point_id 匹配（读基线，防回归）。
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

    // 6. 授权落写：匹配的 AuthorizedCommand + 匹配点 → 成功；回读确认
    //    （echoed 若存在必须与写入值一致——§6.1 "可空"语义，echoed 即 seam
    //    层回读通道；真实适配器以协议应答确认时返回 None）。
    let written_value = PointValue::Float(1.5);
    let auth = authorized_command(device_id, point_ids[0]);
    let outcome = adapter
        .write_authorized(&auth, written_value.clone())
        .unwrap_or_else(|e| panic!("契约：write_authorized(授权命令+匹配点) 必须成功，got {e:?}"));
    if let Some(echo) = &outcome.echoed {
        assert_eq!(
            echo, &written_value,
            "契约：回读确认（若提供）必须与写入值一致"
        );
    }
    // 落写后回读基线（真实适配器工程量换算可能有损，"状态不变"以基线为准，
    // 断言精确相等，不设容差）。
    let baseline = adapter
        .read_point(point_ids[0])
        .unwrap_or_else(|e| panic!("契约：授权落写后 read_point 必须成功，got {e:?}"))
        .value;

    // 7. 不匹配拒绝：设备不匹配 → CommandMismatch，且设备状态不变
    //    （回读值仍为基线）。
    let wrong_device = authorized_command(device_id.wrapping_add(1), point_ids[0]);
    let r = adapter.write_authorized(&wrong_device, written_value.clone());
    assert_eq!(
        r,
        Err(WriteError::CommandMismatch),
        "契约：设备不匹配必须返回 CommandMismatch"
    );
    let dp = adapter
        .read_point(point_ids[0])
        .expect("契约：不匹配拒绝后 read_point 必须成功");
    assert_eq!(
        dp.value, baseline,
        "契约：不匹配拒绝不得改变设备状态（回读必须仍为基线）"
    );

    // 8. 篡改值拒绝：授权命令 + 与点类型矛盾的载荷（Analog 点写 Bool）→
    //    拒绝（类型级篡改防御），设备状态不变。
    let r = adapter.write_authorized(&auth, PointValue::Bool(true));
    assert!(
        r.is_err(),
        "契约：篡改值（与点类型矛盾）必须拒绝，got {r:?}"
    );
    let dp = adapter
        .read_point(point_ids[0])
        .expect("契约：篡改值拒绝后 read_point 必须成功");
    assert_eq!(dp.value, baseline, "契约：篡改值拒绝不得改变设备状态");

    // 9. 按设备读取：返回结果非空且包含注册点。
    let dev_points = adapter
        .read_device_points(device_id)
        .expect("契约：read_device_points(已注册设备) 必须成功");
    assert!(!dev_points.is_empty(), "契约：read_device_points 不得为空");
    assert!(
        dev_points.iter().any(|dp| dp.point_id == point_ids[0]),
        "契约：read_device_points 结果必须包含注册点 {}",
        point_ids[0]
    );

    // 10. 生命周期：start → Running、poll → Ok、stop → Stopped。
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

    // 11. 负向：未注册点必须报错（fail-closed）。
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

/// 构造一条与 (device_id, point_id) 匹配的授权命令（契约写用例共用）。
///
/// 其余字段取固定合法值；本 helper 仅用于契约测试——生产路径的
/// `AuthorizedCommand` 只能由控制网关产出（编译期事实，§8.2 第一层）。
/// 29 GV-02：authorized-construction 门禁转全行扫描后，此 helper 位于
/// 生产文件且无 cfg 保护会被拦——补 mock/test 门控（S2 mock 门控范式）。
#[cfg(any(test, feature = "mock"))]
pub fn authorized_command(device_id: DeviceId, point_id: PointId) -> AuthorizedCommand {
    use eneros_telemetry_model::command::{ActorKind, ControlCommand, SingleCommand};
    // WP-26.4：字段私有后经 from_parts 构造（契约套件唯一 fixture 入口）。
    AuthorizedCommand::from_parts(
        device_id,
        point_id,
        ControlCommand::Single(SingleCommand::On),
        ActorKind::Operator,
        [0xAB; 16],
        [0xCD; 16],
        1,
        [0xE1; 16],
        7,
        3,
        [0x01; 16],
    )
}

/// 便捷包装：从配置构造适配器配置（供各适配器测试复用）。
#[cfg(any(test, feature = "mock"))]
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
