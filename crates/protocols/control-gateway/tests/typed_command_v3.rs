//! 蓝图 61 v0.314.25 红测：命令语义、单位与安全目录（信封 V3 typed value）。
//!
//! 唯一不变量：Gateway 验证和签发的 typed value，必须与 writer 最终发送
//! 给设备的 typed value 在资产、点、命令种类、数值、单位、有效期和
//! digest 上完全一致。
//!
//! T1 类别错配拒绝 / T2 V2 legacy 仅 Switch / T5 typed 往返 + digest 绑定 /
//! T6 未知 value_tag fail-closed / T7 长度门。
//!
//! 实现前必须为红（编译失败或断言失败），实现后全绿。

use eneros_control_gateway::chain::{ChainOutcome, ControlChain, FeedbackBundle};
use eneros_control_gateway::envelope::{
    CommandEnvelope, EnvelopeBuilder, EnvelopeError, ENVELOPE_LEN_V3, HEADER_LEN_V3,
};
use eneros_control_gateway::profile::ControlProfile;
use eneros_control_gateway::wire::ControlEnvelope;
use eneros_telemetry_model::command::{CommandValue, ControlCommand, SingleCommand};

const KEY: &[u8] = b"b61-t25-typed-command-red-test-key-0000000000";
const LEASE_ID: [u8; 16] = [0x40; 16];
const EPOCH_SEED: u64 = 7;

fn now() -> u64 {
    1_700_000_000
}

/// Switch 档案链（ai_pcc_single：Switch/PccBreaker/±500/Discrete——现快照）。
fn switch_chain() -> ControlChain {
    let profile = ControlProfile::ai_pcc_single(1, 10, 4, LEASE_ID, EPOCH_SEED);
    let mut chain = ControlChain::from_profile(KEY, &profile);
    let _ = profile.issue_lease(&mut chain, now());
    chain
}

fn builder(value: CommandValue) -> EnvelopeBuilder {
    EnvelopeBuilder::new(
        [2u8; 16],
        1,
        10,
        ControlCommand::Single(SingleCommand::On),
        now(),
        now() + 30,
        [0xAB; 16],
    )
    .actor_kind(eneros_control_gateway::envelope::ActorKind::Ai)
    .lease(LEASE_ID, 1 + EPOCH_SEED)
    .policy_version(4)
    .value(value)
}

/// T5-a：V3 信封携带 ActivePowerKw(50.0) 往返一致（HMAC 覆盖值域）。
#[test]
fn t5_typed_value_roundtrip() {
    let env = builder(CommandValue::ActivePowerKw(50.0)).build(KEY);
    let buf = env.encode();
    assert_eq!(buf.len(), ENVELOPE_LEN_V3);
    let back = CommandEnvelope::decode(&buf).unwrap();
    assert!(back.verify_hmac(KEY), "V3 HMAC 必须覆盖 typed value");
    assert_eq!(back.value, CommandValue::ActivePowerKw(50.0));
}

/// T5-b：command_digest 确定性 + 值绑定——同体同 digest；值 50→60 变 digest。
#[test]
fn t5_command_digest_deterministic_and_binding() {
    let env50 = builder(CommandValue::ActivePowerKw(50.0)).build(KEY);
    let env60 = builder(CommandValue::ActivePowerKw(60.0)).build(KEY);
    let d1 = env50.command_digest();
    assert_eq!(d1, env50.command_digest(), "digest 必须确定性");
    assert_ne!(
        env60.command_digest(),
        d1,
        "值 50→60 必须改变 digest（授权 50 投递 60 可检出）"
    );
    // Switch 位置与模拟量同 digest 域必须区分。
    let sw = builder(CommandValue::SwitchPosition(1)).build(KEY);
    assert_ne!(sw.command_digest(), d1);
}

/// T1：gateway 拒绝类别错配——ActivePowerKw 打在 Switch 档案上（策略 PDP）。
#[test]
fn t1_switch_profile_rejects_active_power() {
    let mut chain = switch_chain();
    chain.sbo_select(10, now());
    let env = builder(CommandValue::ActivePowerKw(50.0)).build(KEY);
    let outcome = chain.execute(
        &env.encode(),
        now(),
        &FeedbackBundle::fresh(true, 50.0, now()),
    );
    assert!(
        matches!(outcome, ChainOutcome::Denied(_)),
        "ActivePowerKw 打在 Switch 类别策略上必须拒绝：{outcome:?}"
    );
    // 对照：SwitchPosition(1) 同链放行（Executed）。
    let mut chain2 = switch_chain();
    chain2.sbo_select(10, now());
    let env2 = builder(CommandValue::SwitchPosition(1)).build(KEY);
    assert!(matches!(
        chain2.execute(
            &env2.encode(),
            now(),
            &FeedbackBundle::fresh(true, 50.0, now())
        ),
        ChainOutcome::Executed { .. }
    ));
}

/// T2：V2 legacy 信封（172B）只产出 SwitchPosition——不得解释为连续值。
#[test]
fn t2_legacy_v2_envelope_decodes_to_switch_only() {
    let v3 = builder(CommandValue::SwitchPosition(1)).build(KEY);
    let legacy = v3.encode_legacy_v2().expect("Switch 可降级 V2");
    assert_eq!(legacy.len(), 172);
    let back = CommandEnvelope::decode(&legacy).expect("V2 legacy 可解码");
    assert_eq!(back.value, CommandValue::SwitchPosition(1));
    // 连续值不可降级（legacy V2 无模拟量语义）。
    let analog = builder(CommandValue::ActivePowerKw(50.0)).build(KEY);
    assert!(
        analog.encode_legacy_v2().is_none(),
        "ActivePower 不得降级 V2"
    );
}

/// T6：V3 信封未知 value_tag fail-closed（BadCommand）。
#[test]
fn t6_unknown_value_tag_rejected() {
    let env = builder(CommandValue::ActivePowerKw(1.0)).build(KEY);
    let mut buf = env.encode();
    buf[51] = 0x7F; // value_tag（V3 头部偏移 51）
    assert!(matches!(
        CommandEnvelope::decode(&buf),
        Err(EnvelopeError::BadCommand { .. })
    ));
}

/// T7：长度门——非 172/181 都拒绝；V3 头长 = 140 + 1 + 8。
#[test]
fn t7_v3_length_gate() {
    assert!(matches!(
        CommandEnvelope::decode(&[0u8; 180]),
        Err(EnvelopeError::BadLength { got: 180 })
    ));
    assert_eq!(HEADER_LEN_V3, 149);
    assert_eq!(ENVELOPE_LEN_V3, 181);
    // V3 digest 长度域分离：ControlEnvelope V3 不再携带 power。
    let env = builder(CommandValue::ActivePowerKw(50.0)).build(KEY);
    let ce = ControlEnvelope::new_v3([7u8; 16], [0x41; 16], 0, now(), now() + 30, &env.encode());
    let bytes = ce.encode();
    let back = ControlEnvelope::decode(&bytes).expect("V3 ControlEnvelope 往返");
    assert!(!back.has_power_field(), "V3 不得携带第二执行真相 power");
    assert!(
        back.verify_payload_digest(),
        "payload digest = SM3(envelope)"
    );
}
