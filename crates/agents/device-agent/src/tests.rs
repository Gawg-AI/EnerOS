//! 集成测试 — DeviceAgent T1~T24（ARCH-P02：PointAccess 统一 seam）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;

use eneros_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};
use eneros_agent::{AgentState, AgentType};
use eneros_protocol_abstract::access::PointAccess;
use eneros_upa_model::PointValue;

use super::*;

/// 构造 MockPointDevice + point_map 并注册到注册表。
fn register_mock(
    reg: &mut DeviceRegistry,
    name: &str,
    device_type: DeviceType,
    points: &[(&str, f64)],
) {
    let mut dev = MockPointDevice::new(1);
    for (n, v) in points {
        dev = dev.with_point(n, *v);
    }
    let mut pm = BTreeMap::new();
    for (n, _) in points {
        if let Some(id) = dev.point_id_of(n) {
            pm.insert(String::from(*n), id);
        }
    }
    reg.register(name, device_type, 1, Box::new(dev), pm)
        .expect("静态合法登记");
}

// ===== T1: DeviceType 变体构造 =====
#[test]
fn t1_device_type_variants() {
    assert_eq!(DeviceType::Pcs, DeviceType::Pcs);
    assert_eq!(DeviceType::Battery, DeviceType::Battery);
    assert_eq!(DeviceType::Bms, DeviceType::Bms);
    assert_eq!(DeviceType::Meter, DeviceType::Meter);
    assert_eq!(DeviceType::Temperature, DeviceType::Temperature);
    assert_ne!(DeviceType::Pcs, DeviceType::Battery);
    assert_ne!(DeviceType::Battery, DeviceType::Bms);
}

// ===== T2: DeviceState::default 全零 =====
#[test]
fn t2_device_state_default() {
    let s = DeviceState::default();
    assert_eq!(s.soc, 0.0);
    assert_eq!(s.voltage, 0.0);
    assert_eq!(s.current, 0.0);
    assert_eq!(s.temperature, 0.0);
    assert_eq!(s.power, 0.0);
    assert!(!s.online);
    assert_eq!(s.last_update_ms, 0);
}

// ===== T3: DeviceSnapshot new/set/get =====
#[test]
fn t3_device_snapshot_new_set_get() {
    let mut snap = DeviceSnapshot::new();
    assert!(snap.is_empty());
    assert_eq!(snap.len(), 0);
    let state = DeviceState {
        soc: 0.5,
        online: true,
        ..DeviceState::default()
    };
    snap.set("pcs", state);
    assert!(!snap.is_empty());
    assert_eq!(snap.len(), 1);
    let got = snap.get("pcs").expect("pcs should exist");
    assert!((got.soc - 0.5).abs() < 1e-9);
    assert!(got.online);
    assert!(snap.get("unknown").is_none());
}

// ===== T4: MockPointDevice::new 空 + read_point(PointId) 未找到 =====
#[test]
fn t4_mock_point_device_new_empty() {
    let mut dev = MockPointDevice::new(1);
    assert_eq!(
        dev.read_point(99),
        Err(eneros_protocol_abstract::error::ProtocolError::PointNotFound)
    );
    assert!(dev.point_ids().is_empty());
}

// ===== T5: MockPointDevice::with_point + point_id_of 映射 =====
#[test]
fn t5_mock_point_device_with_point() {
    let dev = MockPointDevice::new(1).with_point("soc", 0.65);
    let id = dev.point_id_of("soc").expect("soc point should exist");
    assert_eq!(id, 1);
    assert!(dev.point_id_of("missing").is_none());
}

// ===== T6: MockPointDevice read_point 返回带质量位与时间戳的 DataPoint =====
#[test]
fn t6_mock_point_device_read_datapoint() {
    let mut dev = MockPointDevice::new(7).with_point("voltage", 400.0);
    let id = dev.point_id_of("voltage").unwrap();
    let dp = dev.read_point(id).expect("read should succeed");
    assert_eq!(dp.point_id, id);
    assert_eq!(dp.device_id, 7);
    assert_eq!(dp.value, PointValue::Float(400.0));
    assert!(dp.quality.valid, "读点必须带质量位");
    assert_eq!(dp.timestamp_ms, 0, "读点必须带时间戳");
}

// ===== T7: MockPointDevice 离线 → 读点 quality invalid =====
#[test]
fn t7_mock_point_device_offline_quality() {
    let mut dev = MockPointDevice::new(1).with_point("soc", 0.65);
    dev.set_offline(true);
    let id = dev.point_id_of("soc").unwrap();
    let dp = dev.read_point(id).expect("read should succeed");
    assert!(!dp.quality.valid, "离线时质量位必须 invalid");
}

// ===== T8: DeviceRegistry::new 空 + register + len =====
#[test]
fn t8_device_registry_register() {
    let mut reg = DeviceRegistry::new();
    assert!(reg.is_empty());
    register_mock(&mut reg, "pcs", DeviceType::Pcs, &[("soc", 0.5)]);
    assert!(!reg.is_empty());
    assert_eq!(reg.len(), 1);
}

// ===== T9: DeviceRegistry get_mut 成功 =====
#[test]
fn t9_device_registry_get_mut() {
    let mut reg = DeviceRegistry::new();
    register_mock(&mut reg, "pcs", DeviceType::Pcs, &[("soc", 0.5)]);
    let info = reg.get_mut("pcs");
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.device_type, DeviceType::Pcs);
    assert_eq!(info.point_map.get("soc"), Some(&1u32));
}

// ===== T10: DeviceRegistry get_mut 未找到返回 None =====
#[test]
fn t10_device_registry_get_mut_not_found() {
    let mut reg = DeviceRegistry::new();
    register_mock(&mut reg, "pcs", DeviceType::Pcs, &[("soc", 0.5)]);
    let info = reg.get_mut("unknown");
    assert!(info.is_none());
}

// ===== T11: DeviceCommand 构造 =====
#[test]
fn t11_device_command_construction() {
    let cmd = DeviceCommand {
        target_device: String::from("pcs"),
        power_kw: 50.0,
        ttl_ms: 5000,
        timestamp_ms: 1000,
    };
    assert_eq!(cmd.target_device, "pcs");
    assert!((cmd.power_kw - 50.0).abs() < 1e-9);
    assert_eq!(cmd.ttl_ms, 5000);
    assert_eq!(cmd.timestamp_ms, 1000);
}

// ===== T12: MockCommandSource::new 空 + try_read None =====
#[test]
fn t12_mock_command_source_new_empty() {
    let mut src = MockCommandSource::new();
    assert!(src.try_read().is_none());
}

// ===== T13: MockCommandSource::with_commands + try_read 成功 =====
#[test]
fn t13_mock_command_source_with_commands() {
    let cmd = DeviceCommand {
        target_device: String::from("pcs"),
        power_kw: 50.0,
        ttl_ms: 5000,
        timestamp_ms: 1000,
    };
    let mut src = MockCommandSource::with_commands(vec![cmd]);
    let result = src.try_read();
    assert!(result.is_some());
    let cmd = result.unwrap();
    assert_eq!(cmd.target_device, "pcs");
    // second read -> None
    assert!(src.try_read().is_none());
}

// ===== T14: MockCommandSource push + try_read =====
#[test]
fn t14_mock_command_source_push() {
    let mut src = MockCommandSource::new();
    let cmd = DeviceCommand {
        target_device: String::from("battery"),
        power_kw: 25.0,
        ttl_ms: 3000,
        timestamp_ms: 2000,
    };
    src.push(cmd);
    let result = src.try_read();
    assert!(result.is_some());
    assert_eq!(result.unwrap().target_device, "battery");
}

// ===== T15: DeviceError 变体构造 =====
#[test]
fn t15_device_error_variants() {
    let _ = DeviceError::DeviceNotFound(String::from("pcs"));
    let _ = DeviceError::PointNotFound(String::from("soc"));
    let _ = DeviceError::DeviceOffline(String::from("battery"));
    let _ = DeviceError::WriteFailed(String::from("write"));
    let _ = DeviceError::ReadFailed(String::from("read"));
}

// ===== T16: From<DeviceError> for AgentRuntimeError 转换 =====
#[test]
fn t16_from_device_error_for_agent_runtime_error() {
    let e: AgentRuntimeError = DeviceError::PointNotFound(String::from("x")).into();
    assert!(matches!(e, AgentRuntimeError::DeviceError(_)));
}

// ===== T17: DeviceAgent::new_default 构造 + 预注册 3 设备 =====
#[test]
fn t17_device_agent_new_default() {
    let agent = DeviceAgent::new_default(1000);
    assert_eq!(agent.descriptor.agent_type, AgentType::Device);
    assert_eq!(agent.state, AgentState::Created);
    assert_eq!(agent.devices.len(), 3);
    assert_eq!(agent.tick_count, 0);
    assert!(agent.last_snapshot().is_empty());
}

// ===== T18: DeviceAgent::on_start 状态转 Running =====
#[test]
fn t18_device_agent_on_start() {
    let mut agent = DeviceAgent::new_default(0);
    agent.on_start(1000).unwrap();
    assert_eq!(agent.state, AgentState::Running);
    assert_eq!(agent.descriptor().agent_type, AgentType::Device);
}

// ===== T19: DeviceAgent::on_tick 经 PointAccess 采集（soc=0.65，质量位有效）=====
#[test]
fn t19_device_agent_on_tick_polls_devices() {
    let mut agent = DeviceAgent::new("test", Box::new(MockCommandSource::new()), 0);
    register_mock(
        agent.registry_mut(),
        "battery",
        DeviceType::Battery,
        &[
            ("soc", 0.65),
            ("voltage", 48.0),
            ("current", 50.0),
            ("temperature", 28.0),
            ("power", 25.0),
        ],
    );
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    let snap = agent.last_snapshot();
    let state = snap.get("battery").expect("battery should be in snapshot");
    assert!((state.soc - 0.65).abs() < 1e-9);
    assert!((state.voltage - 48.0).abs() < 1e-9);
    assert!(state.online);
    assert_eq!(agent.tick_count, 1);
}

// ===== T20: DeviceAgent::on_tick 旧匿名命令双轨——收到不执行、留痕（蓝图/24 §6.2）=====
#[test]
#[allow(clippy::disallowed_macros)] // 测试断言允许 panic
fn t20_device_agent_on_tick_denies_legacy_command() {
    let mut source = MockCommandSource::new();
    source.push(DeviceCommand {
        target_device: String::from("pcs"),
        power_kw: 50.0,
        ttl_ms: 5000,
        timestamp_ms: 1000,
    });
    let mut agent = DeviceAgent::new("test", Box::new(source), 0);
    // pcs 设备含 power_setpoint 点位（ReadWrite）
    let dev = MockPointDevice::new(1).with_point("power_setpoint", 0.0);
    let mut pm = BTreeMap::new();
    if let Some(id) = dev.point_id_of("power_setpoint") {
        pm.insert(String::from("power_setpoint"), id);
    }
    agent
        .registry_mut()
        .register("pcs", DeviceType::Pcs, 1, Box::new(dev), pm)
        .expect("静态合法登记");
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    // 旧行为（匿名命令直接落写）已判定为错误：必须收到不执行。
    // 防回归：power_setpoint 保持初始值 0.0（未被写入）。
    let info = agent
        .registry_mut()
        .get_mut("pcs")
        .expect("pcs should exist");
    let id = info
        .point_map
        .get("power_setpoint")
        .expect("point_map 应含 power_setpoint");
    let dp = info.adapter.read_point(*id).expect("read should succeed");
    match dp.value {
        PointValue::Float(v) => assert!(
            (v - 0.0).abs() < 1e-9,
            "旧匿名命令不得落写：power_setpoint 必须保持 0.0，got {v}"
        ),
        other => panic!("power_setpoint 应为 Float，got {other:?}"),
    }
    // 留痕：拒绝计数 +1（WP-24.3 接入 write_denied_legacy 指标 + 审计）。
    assert_eq!(agent.legacy_denied, 1, "旧匿名命令必须计数留痕");
    // §6.6：write_denied_legacy 指标同步留痕（D12 双轨）。
    assert_eq!(
        agent
            .ingress
            .metric_total(crate::ingress::METRIC_DENIED_LEGACY, 2000),
        1,
        "旧匿名命令必须产生 write_denied_legacy 指标"
    );
}

// ===== T21: DeviceAgent::on_tick 设备离线（质量位 invalid → online:false）=====
#[test]
fn t21_device_agent_on_tick_offline_device() {
    let mut agent = DeviceAgent::new("test", Box::new(MockCommandSource::new()), 0);
    let mut dev = MockPointDevice::new(1).with_point("soc", 0.65);
    dev.set_offline(true);
    let mut pm = BTreeMap::new();
    if let Some(id) = dev.point_id_of("soc") {
        pm.insert(String::from("soc"), id);
    }
    agent
        .registry_mut()
        .register("battery", DeviceType::Battery, 1, Box::new(dev), pm)
        .expect("静态合法登记");
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    let snap = agent.last_snapshot();
    let state = snap.get("battery").expect("battery should be in snapshot");
    assert!(!state.online, "质量位 invalid 时必须标记离线");
}

// ===== T22: DeviceAgent::on_tick 命令目标设备不存在跳过 =====
#[test]
fn t22_device_agent_on_tick_unknown_device_skipped() {
    let mut source = MockCommandSource::new();
    source.push(DeviceCommand {
        target_device: String::from("unknown"),
        power_kw: 50.0,
        ttl_ms: 5000,
        timestamp_ms: 1000,
    });
    let mut agent = DeviceAgent::new("test", Box::new(source), 0);
    agent.on_start(1000).unwrap();
    let result = agent.on_tick(2000);
    assert!(result.is_ok());
}

// ===== T23: DeviceAgent::on_stop 状态转 Dead =====
#[test]
fn t23_device_agent_on_stop() {
    let mut agent = DeviceAgent::new_default(0);
    agent.on_start(1000).unwrap();
    agent.on_stop(2000).unwrap();
    assert_eq!(agent.state, AgentState::Dead);
}

// ===== T24: DeviceAgent::on_heartbeat Running → Alive / Dead → Dead =====
#[test]
fn t24_device_agent_heartbeat() {
    let mut agent = DeviceAgent::new_default(0);
    // Created → Dead
    assert_eq!(agent.on_heartbeat(1000), HeartbeatStatus::Dead);
    // Running → Alive
    agent.on_start(1000).unwrap();
    assert_eq!(agent.on_heartbeat(2000), HeartbeatStatus::Alive);
    // Dead → Dead
    agent.on_stop(3000).unwrap();
    assert_eq!(agent.on_heartbeat(4000), HeartbeatStatus::Dead);
}

// ===== WP-A.3（SPEC-SERIES-2026-08-27/A）：能力摘要收编 =====

#[test]
fn wp_a3_register_digest_is_real_and_deterministic() {
    let mut reg = DeviceRegistry::new();
    register_mock(&mut reg, "pcs", DeviceType::Pcs, &[("soc", 0.5)]);
    let d1 = reg
        .get_mut("pcs")
        .expect("已注册")
        .registration
        .capability_digest;
    assert_ne!(d1, [0u8; 32], "登记摘要不得为全零占位（WP-A.3 C2）");

    // 确定性：同输入重建 → 同摘要。
    let mut reg2 = DeviceRegistry::new();
    register_mock(&mut reg2, "pcs", DeviceType::Pcs, &[("soc", 0.5)]);
    let d2 = reg2.get_mut("pcs").unwrap().registration.capability_digest;
    assert_eq!(d1, d2, "同输入同输出");
}

#[test]
fn wp_a3_replay_heals_zero_placeholder_and_guards_empty_schema() {
    // 回放自愈：旧快照/WAL 时代持久化的全零占位 → register_with_registration
    // 按四元组重算，自动换真值（行格式不变）。
    let mut reg = DeviceRegistry::new();
    let legacy = DeviceRegistration {
        device_id: 7,
        adapter_kind: AdapterKind::Meter,
        endpoint: EndpointRef(String::from("meter-legacy")),
        point_schema: PointSchemaVersion(1),
        allowed_ops: OperationSet::read_write_readback(),
        capability_digest: [0u8; 32],
        config_generation: 0,
    };
    let dev = MockPointDevice::new(1).with_point("voltage", 220.0);
    let mut pm = BTreeMap::new();
    if let Some(id) = dev.point_id_of("voltage") {
        pm.insert(String::from("voltage"), id);
    }
    reg.register_with_registration("meter-legacy", DeviceType::Meter, legacy, Box::new(dev), pm)
        .expect("全零占位经重算自愈");
    let healed = reg
        .get_mut("meter-legacy")
        .unwrap()
        .registration
        .capability_digest;
    assert_ne!(healed, [0u8; 32], "回放后摘要必须为真值");

    // 非法空 schema（空点映射）→ 拒注册留证（WP-A.3 C2）。
    let mut reg2 = DeviceRegistry::new();
    let res = reg2.register(
        "empty-device",
        DeviceType::Battery,
        9,
        Box::new(MockPointDevice::new(9)),
        BTreeMap::new(),
    );
    assert!(
        matches!(res, Err(RegistryError::EmptyPointMap)),
        "空点映射必须被守卫拒绝"
    );
    assert!(!reg2.contains("empty-device"), "被拒登记不得入库");
}
