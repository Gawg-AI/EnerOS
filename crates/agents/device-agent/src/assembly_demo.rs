//! 装配检查点演示（蓝图/24 §7.4 + §6.7，`#[cfg(test)]`）：HMAC 密钥 /
//! lease_id / 重启计数的注入路径——密钥只走装配层注入（环境变量，不落明文
//! 配置），lease_id 来自 `configs/control-write.toml`（预置 hex），重启计数
//! 经装配参数传入 `EnvelopeIngress::new`（fencing epoch 种子）。
//!
//! 同时输出拒绝指标样例（§7.4 验收：拒绝指标样例输出）。

#![cfg(test)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;

use eneros_control_gateway::envelope::{ActorKind, EnvelopeBuilder};
use eneros_control_gateway::lease::LeaseScope;
use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
use eneros_telemetry_model::unit::PhysicalUnit;

use crate::device_type::DeviceType;
use crate::ingress::{
    verify_handshake_response, DenyClass, EnvelopeIngress, IngressOutcome, METRIC_DENIED_HMAC,
    METRIC_DENIED_LEGACY, METRIC_IDEMPOTENT_HIT, METRIC_WRITE_ALLOWED,
};
use crate::registry::MockPointDevice;
use crate::{DeliveryFrame, DeviceAgent, MockCommandSource};

/// 从 configs/control-write.toml 读取预置 lease_id（16 字节 hex）。
fn read_lease_id_from_config() -> [u8; 16] {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../configs/control-write.toml"
    );
    let text = std::fs::read_to_string(path).expect("control-write.toml 必须存在");
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("lease_id"))
        .expect("control_write.lease_id 键必须存在");
    let hex = line
        .split('=')
        .nth(1)
        .expect("键值格式")
        .trim()
        .trim_matches('"');
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("合法 hex");
    }
    out
}

/// 装配检查点演示：密钥经装配层注入（环境变量，不落明文）→ 完整信封链路。
#[test]
#[allow(clippy::disallowed_macros)] // 装配演示允许断言
fn assembly_injection_path_and_metric_samples() {
    // ---- 装配层注入（§7.4）----
    // 1. HMAC 密钥：只经装配层（环境变量/密钥服务）注入；configs 中无明文。
    let key: Vec<u8> = std::env::var("ENEROS_WRITE_KEY")
        .unwrap_or_else(|_| String::from("assembly-demo-key-000000000000000000"))
        .into_bytes();
    // 2. lease_id：配置预置（两侧同源）。
    let lease_id = read_lease_id_from_config();
    // 3. 重启计数：平台监督提供（supervisor restart_count）。
    let restart_count: u64 = 7;

    // ---- 装配 EnvelopeIngress（与签发侧同源参数）----
    let device_id: u16 = 1;
    let point_id: u32 = 1;
    let policy = Policy::new(3).with_rule(PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![device_id as u32],
        points: vec![point_id],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    });
    let safety = vec![(
        device_id as u32,
        point_id,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id,
                device_id,
                unit: PhysicalUnit::Kilowatts,
                safe_min: -500.0,
                safe_max: 500.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    )];
    let mut ingress = EnvelopeIngress::new(
        &key,
        lease_id,
        LeaseScope {
            devices: vec![device_id as u32],
            points: vec![point_id],
            max_class: 1,
        },
        3_600,
        restart_count,
        policy,
        60,
        &safety,
        1_700_000_000_000,
    );
    assert!(ingress.lease_ok(), "本地租约必须以重启计数为种子签发成功");

    // ---- 装配设备（模拟 PCS；生产为真实协议适配器）----
    // FIX-24.R3：CTL-010 真实反馈需要全部遥测点（无遥测 → 复验 fail-safe deny）。
    let mut dev = MockPointDevice::new(device_id)
        .with_point("power_setpoint", 0.0)
        .with_point("grid_up", 1.0)
        .with_point("soc_pct", 50.0)
        .with_point("dc_bus_v", 600.0)
        .with_point("overcurrent", 0.0)
        .with_point("overtemp", 0.0);
    dev.touch(1_700_000_000_000);
    let mut pm = BTreeMap::new();
    for n in [
        "power_setpoint",
        "grid_up",
        "soc_pct",
        "dc_bus_v",
        "overcurrent",
        "overtemp",
    ] {
        if let Some(id) = dev.point_id_of(n) {
            pm.insert(String::from(n), id);
        }
    }
    // ---- 启动握手（签发方经 EIPC 通道取 fencing_token）----
    let handshake = ingress.handshake_response().expect("handshake");
    let (got_lease, token) = verify_handshake_response(&key, &handshake).expect("hmac verify");
    assert_eq!(got_lease, lease_id);
    assert_eq!(token, restart_count + 1, "token 以重启计数为种子铸造");

    // ---- 装配 Agent（§7.4：收口点经装配层注入；随后握手结果供信封签发）----
    let mut agent = DeviceAgent::new_with_ingress(
        "demo",
        Box::new(MockCommandSource::new()),
        0,
        ingress, // 密钥/租约/epoch 随收口点注入
    );
    agent
        .registry_mut()
        .register("pcs", DeviceType::Pcs, device_id, Box::new(dev), pm);

    // ---- 有效信封 → 落写 ----
    let now_secs = 1_700_000_000u64;
    let env = EnvelopeBuilder::new(
        [1u8; 16],
        device_id as u32,
        point_id,
        ControlCommand::Single(SingleCommand::On),
        now_secs,
        now_secs + 30,
        [0xCD; 16],
    )
    .lease(lease_id, token)
    .policy_version(3)
    .sequence(1)
    .actor_kind(ActorKind::Ai)
    .idempotency_key([0x55; 16])
    .build(&key)
    .encode();
    let payload = DeliveryFrame {
        envelope: env,
        power_kw: 25.0,
    }
    .encode();
    let out = agent.process_envelope(&payload, 1_700_000_000_000);
    assert!(
        matches!(out, IngressOutcome::Applied { .. }),
        "装配链路有效信封必须落写: {out:?}"
    );

    // ---- 篡改信封 → 拒绝（HMAC）+ 拒绝指标样例输出 ----
    let mut tampered = env;
    tampered[45] ^= 0xFF;
    let payload2 = DeliveryFrame {
        envelope: tampered,
        power_kw: 25.0,
    }
    .encode();
    let out2 = agent.process_envelope(&payload2, 1_700_000_000_000);
    assert!(
        matches!(
            out2,
            IngressOutcome::Denied {
                class: DenyClass::Hmac,
                ..
            }
        ),
        "篡改信封必须拒绝: {out2:?}"
    );

    // ---- 拒绝指标样例输出（§7.4 验收证据；telemetry 导出见 WP-24.5）----
    let now = 1_700_000_000_000u64;
    println!("=== 拒绝指标样例（eneros_write_*_total，蓝图/24 §6.6）===");
    println!(
        "eneros_write_allowed_total        = {}",
        agent.ingress.metric_total(METRIC_WRITE_ALLOWED, now)
    );
    println!(
        "eneros_write_denied_hmac_total    = {}",
        agent.ingress.metric_total(METRIC_DENIED_HMAC, now)
    );
    println!(
        "eneros_write_idempotent_hit_total = {}",
        agent.ingress.metric_total(METRIC_IDEMPOTENT_HIT, now)
    );
    println!(
        "eneros_write_denied_legacy_total  = {}",
        agent.ingress.metric_total(METRIC_DENIED_LEGACY, now)
    );
    println!("=== 装配检查点（§7.4）：密钥经装配层注入（env），未落明文 ===");
    println!(
        "lease_id 来自 configs/control-write.toml：{:02x?}",
        lease_id
    );
    println!("restart_count（epoch 种子）注入：{restart_count}");
    println!("本地租约签发：token = {token}（= restart_count + 1）");

    // 断言样例数值（防回归：允许 1 条、拒绝 1 条）。
    assert_eq!(agent.ingress.metric_total(METRIC_WRITE_ALLOWED, now), 1);
    assert_eq!(agent.ingress.metric_total(METRIC_DENIED_HMAC, now), 1);
}
