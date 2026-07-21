//! 集成测试 — DeviceAgent T1~T24.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;

use eneros_agent::{AgentState, AgentType};
use eneros_energy_market_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};

use super::*;

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

// ===== T4: MockDevice::new 空 + read_point 未找到 =====
#[test]
fn t4_mock_device_new_empty() {
    let mut dev = MockDevice::new(DeviceType::Battery);
    let result = dev.read_point("soc");
    assert!(matches!(result, Err(DeviceError::PointNotFound(_))));
}

// ===== T5: MockDevice::with_point 链式 + read_point 成功 =====
#[test]
fn t5_mock_device_with_point() {
    let mut dev = MockDevice::new(DeviceType::Battery).with_point("soc", 0.65);
    let result = dev.read_point("soc");
    assert!(result.is_ok());
    assert!((result.unwrap() - 0.65).abs() < 1e-9);
}

// ===== T6: MockDevice set_point + write_point 成功 =====
#[test]
fn t6_mock_device_set_point_write_point() {
    let mut dev = MockDevice::new(DeviceType::Pcs);
    dev.set_point("voltage", 400.0);
    assert!((dev.read_point("voltage").unwrap() - 400.0).abs() < 1e-9);
    let result = dev.write_point("power", 50.0);
    assert!(result.is_ok());
    assert!((dev.read_point("power").unwrap() - 50.0).abs() < 1e-9);
}

// ===== T7: MockDevice 离线 + read_point 返回 DeviceOffline =====
#[test]
fn t7_mock_device_offline() {
    let mut dev = MockDevice::new(DeviceType::Battery).with_point("soc", 0.65);
    dev.set_online(false);
    let result = dev.read_point("soc");
    assert!(matches!(result, Err(DeviceError::DeviceOffline(_))));
    assert!(!dev.is_online());
}

// ===== T8: DeviceRegistry::new 空 + register + len =====
#[test]
fn t8_device_registry_register() {
    let mut reg = DeviceRegistry::new();
    assert!(reg.is_empty());
    reg.register(
        "pcs",
        DeviceType::Pcs,
        Box::new(MockDevice::new(DeviceType::Pcs)),
    );
    assert!(!reg.is_empty());
    assert_eq!(reg.len(), 1);
}

// ===== T9: DeviceRegistry get_mut 成功 =====
#[test]
fn t9_device_registry_get_mut() {
    let mut reg = DeviceRegistry::new();
    reg.register(
        "pcs",
        DeviceType::Pcs,
        Box::new(MockDevice::new(DeviceType::Pcs)),
    );
    let info = reg.get_mut("pcs");
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.device_type, DeviceType::Pcs);
}

// ===== T10: DeviceRegistry get_mut 未找到返回 None =====
#[test]
fn t10_device_registry_get_mut_not_found() {
    let mut reg = DeviceRegistry::new();
    reg.register(
        "pcs",
        DeviceType::Pcs,
        Box::new(MockDevice::new(DeviceType::Pcs)),
    );
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

// ===== T19: DeviceAgent::on_tick 采集设备状态（soc=0.65）=====
#[test]
fn t19_device_agent_on_tick_polls_devices() {
    let mut agent = DeviceAgent::new("test", Box::new(MockCommandSource::new()), 0);
    agent.registry_mut().register(
        "battery",
        DeviceType::Battery,
        Box::new(MockDevice::new(DeviceType::Battery).with_point("soc", 0.65)),
    );
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    let snap = agent.last_snapshot();
    let state = snap.get("battery").expect("battery should be in snapshot");
    assert!((state.soc - 0.65).abs() < 1e-9);
    assert!(state.online);
    assert_eq!(agent.tick_count, 1);
}

// ===== T20: DeviceAgent::on_tick 执行命令（power_kw=50.0）=====
#[test]
fn t20_device_agent_on_tick_executes_commands() {
    let mut source = MockCommandSource::new();
    source.push(DeviceCommand {
        target_device: String::from("pcs"),
        power_kw: 50.0,
        ttl_ms: 5000,
        timestamp_ms: 1000,
    });
    let mut agent = DeviceAgent::new("test", Box::new(source), 0);
    agent.registry_mut().register(
        "pcs",
        DeviceType::Pcs,
        Box::new(MockDevice::new(DeviceType::Pcs)),
    );
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    // Verify power_setpoint was written
    let info = agent
        .registry_mut()
        .get_mut("pcs")
        .expect("pcs should exist");
    let power_setpoint = info.adapter.read_point("power_setpoint").unwrap();
    assert!((power_setpoint - 50.0).abs() < 1e-9);
}

// ===== T21: DeviceAgent::on_tick 设备离线标记 =====
#[test]
fn t21_device_agent_on_tick_offline_device() {
    let mut agent = DeviceAgent::new("test", Box::new(MockCommandSource::new()), 0);
    let mut device = MockDevice::new(DeviceType::Battery).with_point("soc", 0.65);
    device.set_online(false);
    agent
        .registry_mut()
        .register("battery", DeviceType::Battery, Box::new(device));
    agent.on_start(1000).unwrap();
    agent.on_tick(2000).unwrap();
    let snap = agent.last_snapshot();
    let state = snap.get("battery").expect("battery should be in snapshot");
    assert!(!state.online);
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
