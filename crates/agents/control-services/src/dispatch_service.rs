//! 蓝图 40 WP-40.6：svc-control-dispatch 服务逻辑。
//!
//! 服务请求 wire：`DISP_SUBMIT` = DispatchRequest（proposal wire + telemetry
//! wire + actor + deadline）→ 唯一 ControlDispatch 全链 → 回执 wire
//! （final_state + note + latency）。
//!
//! 服务持有 [`DispatchEngine`]（validate→authorize→persist→deliver→ACK→
//! readback→audit）；投递经 [`DeliveryChannel`]（生产：Sel4EipcChannel →
//! svc-device-agent）。

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use eneros_control_dispatch::dispatch::{AuthenticatedActor, DispatchReceipt, DispatchState};
use eneros_control_dispatch::ControlDispatch;
#[cfg(test)]
use eneros_control_dispatch::DeliveryChannel;

use crate::ai_worker::decode_proposal;
use crate::telemetry::decode_trusted;

/// 请求码。
pub const DISP_SUBMIT: u8 = 1;
/// 47（WP-47.1 协议入站写）：协议写意图提交（intent wire + actor）。
pub const DISP_SUBMIT_INTENT: u8 = 2;

/// 回执状态。
pub const DISP_OK: u8 = 0;
pub const DISP_ERR: u8 = 1;

/// 回执 wire：status 1 + final_state 1 + committed 8 + latency 8 + note ≤64。
pub const RECEIPT_WIRE_LEN: usize = 1 + 1 + 8 + 8 + 64;

/// 编码回执（note 截断到 64B）。
pub fn encode_receipt(r: &DispatchReceipt) -> Vec<u8> {
    let mut out = Vec::with_capacity(RECEIPT_WIRE_LEN);
    out.push(DISP_OK);
    out.push(state_code(r.final_state));
    out.extend_from_slice(&r.committed_at_ms.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&r.latency_ms.to_le_bytes());
    let note = r.note.clone().unwrap_or_default();
    let bytes = note.as_bytes();
    let n = core::cmp::min(64, bytes.len());
    out.extend_from_slice(&bytes[..n]);
    out.resize(RECEIPT_WIRE_LEN, 0);
    out
}

/// 解码回执（长度校验）。
pub fn decode_receipt(buf: &[u8]) -> Option<(u8, DispatchState, u64, u64, String)> {
    if buf.len() != RECEIPT_WIRE_LEN {
        return None;
    }
    let status = buf[0];
    let state = state_from_code(buf[1])?;
    let committed = u64::from_le_bytes(buf[2..10].try_into().ok()?);
    let latency = u64::from_le_bytes(buf[10..18].try_into().ok()?);
    let note_end = buf[18..]
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    let note = String::from_utf8_lossy(&buf[18..18 + note_end]).into_owned();
    Some((status, state, committed, latency, note))
}

fn state_code(s: DispatchState) -> u8 {
    match s {
        DispatchState::Proposed => 0,
        DispatchState::Validated => 1,
        DispatchState::Authorized => 2,
        DispatchState::Reserved => 3,
        DispatchState::Persisted => 4,
        DispatchState::Delivered => 5,
        DispatchState::TransportAcked => 6,
        DispatchState::ReadbackVerified => 7,
        DispatchState::Committed => 8,
        DispatchState::Rejected => 9,
        DispatchState::Failed => 10,
        DispatchState::Indeterminate => 11,
    }
}

fn state_from_code(c: u8) -> Option<DispatchState> {
    match c {
        0 => Some(DispatchState::Proposed),
        1 => Some(DispatchState::Validated),
        2 => Some(DispatchState::Authorized),
        3 => Some(DispatchState::Reserved),
        4 => Some(DispatchState::Persisted),
        5 => Some(DispatchState::Delivered),
        6 => Some(DispatchState::TransportAcked),
        7 => Some(DispatchState::ReadbackVerified),
        8 => Some(DispatchState::Committed),
        9 => Some(DispatchState::Rejected),
        10 => Some(DispatchState::Failed),
        11 => Some(DispatchState::Indeterminate),
        _ => None,
    }
}

/// 服务逻辑：请求（proposal wire + telemetry wire + actor）→ 回执。
pub fn handle_submit(engine: &mut dyn ControlDispatch, req: &[u8]) -> Vec<u8> {
    let Some(&code) = req.first() else {
        return err_receipt("empty request");
    };
    if code != DISP_SUBMIT {
        return err_receipt("unknown opcode");
    }
    let body = &req[1..];
    // 布局：proposal wire（69B）+ telemetry wire（TRUSTED_WIRE_LEN）+ actor 1B。
    let prop_len = crate::ai_worker::PROPOSAL_WIRE_LEN;
    if body.len() < prop_len + crate::telemetry::TRUSTED_WIRE_LEN + 1 {
        return err_receipt("short request");
    }
    let Some(plan) = decode_proposal(&body[..prop_len]) else {
        return err_receipt("bad proposal");
    };
    let Some(telemetry) =
        decode_trusted(&body[prop_len..prop_len + crate::telemetry::TRUSTED_WIRE_LEN])
    else {
        return err_receipt("bad telemetry");
    };
    let actor_kind = body[prop_len + crate::telemetry::TRUSTED_WIRE_LEN];
    if actor_kind != 2 {
        // 2 = ActorKind::Ai（信封字节码）。
        return err_receipt("actor must be Ai");
    }
    let actor = AuthenticatedActor {
        kind: eneros_control_gateway::envelope::ActorKind::Ai,
        service: [0u8; 16],
        session_epoch: 0,
    };
    let deadline_ms = 5000;
    match engine.submit(&plan, &telemetry, &actor, deadline_ms) {
        Ok(receipt) => encode_receipt(&receipt),
        Err(e) => err_receipt(&format!("{e:?}")),
    }
}

fn err_receipt(reason: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(RECEIPT_WIRE_LEN);
    out.push(DISP_ERR);
    out.push(state_code(DispatchState::Rejected));
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    let bytes = reason.as_bytes();
    let n = core::cmp::min(64, bytes.len());
    out.extend_from_slice(&bytes[..n]);
    out.resize(RECEIPT_WIRE_LEN, 0);
    out
}

/// 47（WP-47.1）：协议写意图请求处理（`DISP_SUBMIT_INTENT` + 意图线码
/// 49B + actor 1B）。遥测由调用方（svc-control-dispatch 经 svc-telemetry
/// 快照）提供——不可得 → fail-closed 拒绝（不猜成功）。
///
/// 请求布局：`[opcode=2][intent wire 49B][actor kind 1B]`。
pub fn handle_submit_intent(
    engine: &mut dyn ControlDispatch,
    req: &[u8],
    telemetry: Option<&eneros_safety_validator::TrustedTelemetry>,
) -> Vec<u8> {
    let Some(&code) = req.first() else {
        return err_receipt("empty request");
    };
    if code != DISP_SUBMIT_INTENT {
        return err_receipt("unknown opcode");
    }
    let body = &req[1..];
    let intent_len = eneros_protocol_abstract::write::WRITE_INTENT_WIRE_LEN;
    if body.len() != intent_len + 1 {
        return err_receipt("intent request length");
    }
    let Some(intent) = eneros_protocol_abstract::write::decode_intent(&body[..intent_len]) else {
        return err_receipt("bad intent wire");
    };
    let actor_kind = body[intent_len];
    if actor_kind != 2 {
        // 2 = ActorKind::Ai（信封字节码；协议写沿用 AI 决策链唯一签发面）。
        return err_receipt("actor must be Ai");
    }
    let Some(tel) = telemetry else {
        return err_receipt("no trusted telemetry (fail-closed)");
    };
    let actor = AuthenticatedActor {
        kind: eneros_control_gateway::envelope::ActorKind::Ai,
        service: [0u8; 16],
        session_epoch: 0,
    };
    let deadline_ms = 5000;
    match engine.submit_intent(&intent, tel, &actor, deadline_ms) {
        Ok(receipt) => encode_receipt(&receipt),
        Err(e) => err_receipt(&format!("{e:?}")),
    }
}

/// proposal 重建后补充 plan_id（wire 定长去零）。
pub fn plan_id_of(plan: &eneros_dual_brain::ProposedControlPlan) -> String {
    plan.plan_id.clone()
}

// ============================================================================
// 产品装配（40 收尾）：svc-control-dispatch 与 svc-device-agent 的同源配置。
// 生产配置注入（安全 key handle/RuntimeConfigBundle）属 41；本装配为
// qemu-prod profile 的确定配置——两端 key/lease/policy/safety 必须同源
// （不同源互相拒绝，QEMU 实弹验证）。
// ============================================================================

/// 产品装配 HMAC key（32B；两端同源）。
pub const PROD_KEY: &[u8; 32] = b"w40-prod-key-0000000000000000000";
/// 产品租约 ID（两侧同源预置，§6.7）。
pub const PROD_LEASE_ID: [u8; 16] = [0x40; 16];
/// fencing epoch 种子（两侧同源；首签 token = seed + 1）。
pub const PROD_EPOCH_SEED: u64 = 7;
/// 目标设备/点（虚拟 PCS 有功功率点）。
pub const PROD_DEVICE_ID: u32 = 1;
pub const PROD_POINT_ID: u32 = 10;
/// 租约 TTL（秒）。
pub const PROD_LEASE_TTL_SECS: u64 = 3_600;
/// 策略版本。
pub const PROD_POLICY_VERSION: u64 = 3;

/// 产品策略：Ai 角色 → 设备 1 点 10 Switch。
///
/// 注意：`replace_rules` 会把 generation +1（policy.rs 语义）——
/// 初始代必须为 `PROD_POLICY_VERSION - 1`，使生效代恰为
/// `PROD_POLICY_VERSION`（信封 `policy_version` 与链 generation 同值，
/// QEMU 实弹 StalePolicyVersion 暴露）。
pub fn prod_policy() -> eneros_control_gateway::policy::Policy {
    use eneros_control_gateway::envelope::ActorKind;
    use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
    let mut policy = Policy::new(PROD_POLICY_VERSION - 1);
    let _ = policy.replace_rules(vec![PolicyRule {
        actor_kind: Some(ActorKind::Ai),
        devices: vec![PROD_DEVICE_ID],
        points: vec![PROD_POINT_ID],
        kinds: vec![CommandClass::Switch],
        time_window: (0, 0),
        obligations: vec![],
    }]);
    policy
}

/// 产品安全点（±500kW 离散遥控）。
pub fn prod_safety_points() -> Vec<(u32, u32, eneros_control_gateway::safety::SafetyPoint)> {
    use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
    use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
    use eneros_telemetry_model::unit::PhysicalUnit;
    vec![(
        PROD_DEVICE_ID,
        PROD_POINT_ID,
        SafetyPoint {
            role: SafetyRole::PccBreaker,
            spec: PointSpec {
                point_id: PROD_POINT_ID,
                device_id: PROD_DEVICE_ID as u16,
                unit: PhysicalUnit::Kilowatts,
                safe_min: -500.0,
                safe_max: 500.0,
                max_ramp_per_sec: None,
                direction: ControlDirection::Discrete,
                remote_controllable: true,
            },
        },
    )]
}

/// dispatch 侧产品链（完整装配：policy 规则 + lease 签发 + safety 注册）。
pub fn prod_chain(now_ms: u64) -> eneros_control_gateway::chain::ControlChain {
    use eneros_control_gateway::chain::ControlChain;
    use eneros_control_gateway::lease::LeaseScope;
    let mut chain = ControlChain::new(PROD_KEY, 5, 64, prod_policy(), 60, 300, 64, 30);
    chain.lease_service_mut().seed_epoch(PROD_EPOCH_SEED);
    let _ = chain.lease_service_mut().issue(
        PROD_LEASE_ID,
        LeaseScope {
            devices: vec![PROD_DEVICE_ID],
            points: vec![PROD_POINT_ID],
            max_class: 1,
        },
        PROD_LEASE_TTL_SECS,
        now_ms / 1000,
    );
    for (device, point, spec) in prod_safety_points() {
        chain.safety_mut().register(device, point, spec);
    }
    chain
}

#[cfg(test)]
mod tests {
    use eneros_control_dispatch::{DispatchConfig, DispatchEngine, InMemoryIntentStore};

    use super::*;
    use crate::ai_worker::{AiWorkerService, AI_PROPOSE};

    // 复用 ai_worker 测试的 mock 后端不可见——此处定义本地后端。
    pub struct LocalBackend;
    impl crate::ai_worker::DecisionBackend for LocalBackend {
        fn decide(
            &mut self,
            _t: &eneros_safety_validator::TrustedTelemetry,
            now_ms: u64,
        ) -> Result<Vec<u8>, crate::ai_worker::DecisionReject> {
            let plan = eneros_dual_brain::ProposedControlPlan {
                plan_id: String::from("req-svc-1"),
                path: eneros_dual_brain::DecisionPath::Fast,
                schedule: eneros_energy_lp_model::result::ScheduleResult {
                    schedule: vec![eneros_energy_lp_model::result::ScheduleEntry {
                        period: 0,
                        charge_power_kw: 0.0,
                        discharge_power_kw: 5.0,
                        net_power_kw: 5.0,
                        soc_pct: 0.5,
                        revenue_yuan: 0.0,
                    }],
                    total_revenue_yuan: 0.0,
                    objective_value: 0.0,
                    solve_status: eneros_solver_core::result::SolveStatus::Optimal,
                },
                power_kw: 5.0,
                target_device: String::from("pcs"),
                ttl_secs: 60,
                issued_at_ms: now_ms,
                input_digest: None,
                model_digest: None,
                latency: Default::default(),
                safety_violations: 0,
                safety_clamped: false,
                feedback: None,
            };
            Ok(crate::ai_worker::encode_proposal(&plan).to_vec())
        }
    }

    /// 内存通道（回执 2B 或 receipt wire）。
    struct MemoryChannel {
        outcome: Vec<u8>,
    }
    impl DeliveryChannel for MemoryChannel {
        fn deliver(
            &mut self,
            _target: &str,
            _payload: &[u8],
            _timeout_ms: u64,
        ) -> Result<Vec<u8>, eneros_control_dispatch::ChannelError> {
            Ok(self.outcome.clone())
        }
    }

    fn engine() -> DispatchEngine {
        // 简化装配：budget 宽松；delivery 回 Applied（code 0）。
        let outcome = eneros_device_agent::OutcomeFrame {
            code: eneros_device_agent::CODE_APPLIED,
            class: 0,
        }
        .encode()
        .to_vec();
        let chain = build_chain();
        DispatchEngine::new(DispatchConfig {
            chain,
            key: b"w40-svc-key-000000000000000000000000".to_vec(),
            lease_id: [0x40; 16],
            fencing_token: 8,
            issuer_service: [0x41; 16],
            issuer_epoch: 0,
            policy_version: 4,
            device_id: 1,
            point_id: 10,
            target_endpoint: String::from("svc-device-agent"),
            max_power_kw: 500.0,
            gate: eneros_dual_brain::gate::DecisionGate::new(100, 1000.0, 3_600_000),
            audit: None,
            intent_log: Box::new(InMemoryIntentStore::new(64)),
            delivery: Box::new(MemoryChannel { outcome }),
            feedback_mapping: eneros_control_dispatch::FeedbackPointMapping {
                grid_up: 5,
                soc: 4,
                dc_bus_v: 1,
                overcurrent: 6,
                overtemp: 6,
            },
            now_fn: || 1_700_000_000_000,
        })
    }

    fn build_chain() -> eneros_control_gateway::chain::ControlChain {
        use eneros_control_gateway::chain::ControlChain;
        use eneros_control_gateway::envelope::ActorKind;
        use eneros_control_gateway::lease::LeaseScope;
        use eneros_control_gateway::policy::{CommandClass, Policy, PolicyRule};
        use eneros_control_gateway::safety::{SafetyPoint, SafetyRole};
        use eneros_telemetry_model::point_spec::{ControlDirection, PointSpec};
        use eneros_telemetry_model::unit::PhysicalUnit;
        let mut policy = Policy::new(3);
        let _ = policy.replace_rules(vec![PolicyRule {
            actor_kind: Some(ActorKind::Ai),
            devices: vec![1],
            points: vec![10],
            kinds: vec![CommandClass::Switch],
            time_window: (0, 0),
            obligations: vec![],
        }]);
        let mut chain = ControlChain::new(
            b"w40-svc-key-000000000000000000000000",
            5,
            64,
            policy,
            60,
            300,
            64,
            30,
        );
        chain.lease_service_mut().seed_epoch(7);
        let _ = chain.lease_service_mut().issue(
            [0x40; 16],
            LeaseScope {
                devices: vec![1],
                points: vec![10],
                max_class: 1,
            },
            600,
            1_700_000_000,
        );
        chain.safety_mut().register(
            1,
            10,
            SafetyPoint {
                role: SafetyRole::PccBreaker,
                spec: PointSpec {
                    point_id: 10,
                    device_id: 1,
                    unit: PhysicalUnit::Kilowatts,
                    safe_min: -500.0,
                    safe_max: 500.0,
                    max_ramp_per_sec: None,
                    direction: ControlDirection::Discrete,
                    remote_controllable: true,
                },
            },
        );
        chain
    }

    fn request_from_worker() -> Vec<u8> {
        // worker 产出 proposal wire（AI_OK + proposal）。
        let mut worker = AiWorkerService::new(LocalBackend, [0xAA; 16]);
        let mut req = vec![AI_PROPOSE];
        // 需要可信遥测 wire——构造一个最小可信遥测（引擎时钟一致）。
        let t = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        req.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        let resp = worker.propose(&req, 1_700_000_000_000);
        assert_eq!(resp[0], crate::ai_worker::AI_OK);
        // 组装 dispatch 请求：DISP_SUBMIT + proposal + telemetry + actor。
        let tel = &req[1..];
        let mut out = Vec::with_capacity(1 + resp.len() - 1 + tel.len() + 1);
        out.push(DISP_SUBMIT);
        out.extend_from_slice(&resp[1..]);
        out.extend_from_slice(tel);
        out.push(2);
        out
    }

    #[test]
    fn receipt_wire_roundtrip() {
        let r = eneros_control_dispatch::DispatchReceipt {
            command_id: [1; 16],
            correlation_id: [2; 16],
            final_state: DispatchState::Committed,
            state_path: vec![DispatchState::Committed],
            committed_at_ms: Some(42),
            note: Some(String::from("ok")),
            latency_ms: 7,
        };
        let buf = encode_receipt(&r);
        let (status, state, committed, latency, note) = decode_receipt(&buf).expect("decode");
        assert_eq!(status, DISP_OK);
        assert_eq!(state, DispatchState::Committed);
        assert_eq!(committed, 42);
        assert_eq!(latency, 7);
        assert_eq!(note, "ok");
    }

    #[test]
    fn submit_routes_to_engine() {
        let req = request_from_worker();
        let mut engine2 = engine();
        // 诊断：直接解码请求中的遥测。
        let prop_len = crate::ai_worker::PROPOSAL_WIRE_LEN;
        let t = crate::telemetry::decode_trusted(
            &req[1 + prop_len..1 + prop_len + crate::telemetry::TRUSTED_WIRE_LEN],
        )
        .expect("tel");
        assert!(t.is_valid_at(1_700_000_000_000), "telemetry 必须新鲜");
        let resp = handle_submit(&mut engine2, &req);
        let (status, state, _c, _l, note) = decode_receipt(&resp).expect("receipt");
        assert_eq!(status, DISP_OK, "note={note}");
        assert_eq!(state, DispatchState::Committed, "note={note}");
    }

    #[test]
    fn malformed_submit_rejected() {
        let mut engine = engine();
        let resp = handle_submit(&mut engine, &[]);
        assert_eq!(resp[0], DISP_ERR);
        let resp = handle_submit(&mut engine, &[99]);
        assert_eq!(resp[0], DISP_ERR);
    }

    // ============ 47（WP-47.1）协议写意图 ingress ============

    /// 意图请求（opcode 2 + intent wire + actor）。
    fn intent_request(intent: &eneros_protocol_abstract::write::PointWriteIntent) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 49 + 1);
        out.push(DISP_SUBMIT_INTENT);
        out.extend_from_slice(&eneros_protocol_abstract::write::encode_intent(intent));
        out.push(2); // ActorKind::Ai
        out
    }

    #[test]
    fn submit_intent_routes_to_engine_committed() {
        let mut engine = engine();
        let tel = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
            "mqtt",
            PROD_DEVICE_ID as u16,
            PROD_POINT_ID,
            eneros_upa_model::PointValue::Float(5.0),
            1,
            1_700_000_000_000,
        );
        let resp = handle_submit_intent(&mut engine, &intent_request(&intent), Some(&tel));
        let (status, state, _c, _l, note) = decode_receipt(&resp).expect("receipt");
        assert_eq!(status, DISP_OK, "note={note}");
        assert_eq!(state, DispatchState::Committed, "note={note}");
    }

    #[test]
    fn submit_intent_fails_closed_without_telemetry() {
        let mut engine = engine();
        let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
            "mqtt",
            PROD_DEVICE_ID as u16,
            PROD_POINT_ID,
            eneros_upa_model::PointValue::Float(5.0),
            1,
            1_700_000_000_000,
        );
        let resp = handle_submit_intent(&mut engine, &intent_request(&intent), None);
        assert_eq!(resp[0], DISP_ERR);
        assert!(decode_receipt(&resp)
            .expect("receipt")
            .4
            .contains("no trusted telemetry"));
    }

    #[test]
    fn submit_intent_rejects_bad_wire_and_actor() {
        let mut engine = engine();
        let tel = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        // 长度不符。
        let resp = handle_submit_intent(&mut engine, &[DISP_SUBMIT_INTENT, 1, 2, 3], Some(&tel));
        assert_eq!(resp[0], DISP_ERR);
        // 未知 opcode。
        let resp = handle_submit_intent(&mut engine, &[99], Some(&tel));
        assert_eq!(resp[0], DISP_ERR);
        // actor 非 Ai。
        let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
            "mqtt",
            PROD_DEVICE_ID as u16,
            PROD_POINT_ID,
            eneros_upa_model::PointValue::Float(5.0),
            1,
            1_700_000_000_000,
        );
        let mut req = intent_request(&intent);
        *req.last_mut().expect("actor byte") = 1; // Agent
        let resp = handle_submit_intent(&mut engine, &req, Some(&tel));
        assert_eq!(resp[0], DISP_ERR);
    }
}
