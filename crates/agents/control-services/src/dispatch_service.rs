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
use alloc::vec::Vec;

use eneros_control_dispatch::dispatch::{AuthenticatedActor, DispatchReceipt, DispatchState};
use eneros_control_dispatch::ControlDispatch;
#[cfg(test)]
use eneros_control_dispatch::DeliveryChannel;

use crate::telemetry::decode_trusted;

/// 蓝图 61 v0.314.35：proposal → CandidatePlan（resource_catalog_version
/// 由调用方注入——planner 输入与 Registry 同代；[08] §4.8 门）。
pub fn candidate_from_proposal(
    plan: &eneros_dual_brain::decision::ProposedControlPlan,
    catalog_version: u64,
) -> Result<eneros_decision_contract::CandidatePlan, alloc::string::String> {
    eneros_control_dispatch::adapter::proposal_to_candidate(plan, (0, 0, 0, 0), 0, catalog_version)
        .map_err(|e| alloc::format!("{e:?}"))
}

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
///
/// 蓝图 63 WP-63.1.1：final_state 码值经单源
/// [`DispatchState::to_code`]（码表单源 = eneros-dispatch-state），
/// 回执 wire 字节零变化。
pub fn encode_receipt(r: &DispatchReceipt) -> Vec<u8> {
    let mut out = Vec::with_capacity(RECEIPT_WIRE_LEN);
    out.push(DISP_OK);
    out.push(r.final_state.to_code());
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
    // 蓝图 63 WP-63.1.1：decode 走单源 from_code（码表单源）——未知码
    // fail-closed None（wire 字节零变化）。
    let state = DispatchState::from_code(buf[1])?;
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

/// 服务逻辑：请求（proposal wire + telemetry wire + actor）→ 回执。
pub fn handle_submit(engine: &mut dyn ControlDispatch, req: &[u8]) -> Vec<u8> {
    let Some(&code) = req.first() else {
        return err_receipt("empty request");
    };
    if code != DISP_SUBMIT {
        return err_receipt("unknown opcode");
    }
    let body = &req[1..];
    // 蓝图 69.4 v0.329.0：单读——唯一入口 = CandidatePlan V1（`CAND`
    // magic；decision-contract 单源）→ `submit_typed`。旧 70B proposal
    // wire 读臂已删除（v0.314.32 承诺的退役，卷 69.4 WP-69.4.1 落实；
    // 词形 SUBMIT/SUBMIT-ERR 随臂退役）。
    let tel_len = crate::telemetry::TRUSTED_WIRE_LEN;
    if body.len() < tel_len + 1 {
        return err_receipt("short request");
    }
    let plan_len = body.len() - tel_len - 1;
    let Some(telemetry) = decode_trusted(&body[plan_len..plan_len + tel_len]) else {
        return err_receipt("bad telemetry");
    };
    let actor_kind = body[plan_len + tel_len];
    // 蓝图 66 WP-66.3.1：actor 语义源 = ActorKind::Ai（匿名 `!= 2` 收编）。
    if actor_kind != eneros_telemetry_model::command::ActorKind::Ai as u8 {
        return err_receipt("actor must be Ai");
    }
    let actor = AuthenticatedActor {
        kind: eneros_control_gateway::envelope::ActorKind::Ai,
        service: [0u8; 16],
        session_epoch: 0,
    };
    let deadline_ms = 5000;
    // 单读判别：唯一入口 = `CAND` magic（CandidatePlan V1）。
    if plan_len >= 4 && &body[0..4] == b"CAND" {
        let Ok(cand) = eneros_decision_contract::CandidatePlan::decode_v1(&body[..plan_len]) else {
            return err_receipt("bad candidate");
        };
        return match engine.submit_typed(&cand, "pcs", &telemetry, &actor, deadline_ms) {
            Ok(receipt) => {
                if !receipt.final_state.is_committed() {
                    eneros_runtime::println!(
                        "[svc-control-dispatch] TYPED state={:?} note={:?}",
                        receipt.final_state,
                        receipt.note
                    );
                }
                encode_receipt(&receipt)
            }
            Err(e) => {
                eneros_runtime::println!("[svc-control-dispatch] TYPED-ERR {e:?}");
                err_receipt(&format!("{e:?}"))
            }
        };
    }
    // 蓝图 69.4 v0.329.0：旧 70B proposal wire 读臂已删除（v0.314.32
    // 承诺的退役）——非候选形态 fail-closed 显式拒绝（如实回执，不得
    // 静默成功；t21 锁此语义）。
    err_receipt("plan body not CAND (legacy proposal wire retired v0.329.0)")
}

pub fn err_receipt(reason: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(RECEIPT_WIRE_LEN);
    out.push(DISP_ERR);
    out.push(DispatchState::Rejected.to_code());
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
    // 蓝图 66 WP-66.3.1：opcode 单源 = telemetry-model
    // `INTENT_REQUEST_OPCODE`（与下方断言锁的 DISP_SUBMIT_INTENT 恒等，
    // 引用锁测试 `disp_submit_intent_locked_to_intent_request_opcode`）；
    // actor 语义源 = ActorKind::Ai（匿名 `!= 2` 字面已收编）。
    if code != eneros_telemetry_model::command::INTENT_REQUEST_OPCODE {
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
    if actor_kind != eneros_telemetry_model::command::ActorKind::Ai as u8 {
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
        Ok(receipt) => {
            eneros_runtime::println!(
                "[svc-control-dispatch] INTENT state={:?} note={:?}",
                receipt.final_state,
                receipt.note
            );
            encode_receipt(&receipt)
        }
        Err(e) => {
            eneros_runtime::println!("[svc-control-dispatch] INTENT-ERR {e:?}");
            err_receipt(&format!("{e:?}"))
        }
    }
}

// ============================================================================
// 产品装配（40 收尾 + 47 WP-47.2 点表绑定）：svc-control-dispatch 与
// svc-device-agent 的同源配置。key/lease/policy/safety 两端必须同源
// （不同源互相拒绝，QEMU 实弹验证）。
//
// 蓝图 55（WP-55.2）：同源事实收拢为 [`ControlProfile`]——装配值常量
// 仅在此声明，两侧经 [`prod_profile`] 单出口派生策略/租约/安全点；
// `prod_policy`/`prod_safety_points` 手抄块已删（body 即 profile 方法）。
//
// 47（WP-47.2）：写目标（设备/点）不再硬编码——由 45 卷点表
// （config "points" canonical 的 Control 点）绑定，装配方经 svc-config
// 解码后传入；键值脱落源码（旧 PROD 常量已退休，grep 零命中）——本装配只承载
// **qemu-test 测试键**（47.3 起 KMS_DIR 注入，release 无键构建
// fail-closed）。
// ============================================================================

// SPEC-SERIES-2026-08-27/A WP-A.1：装配 HMAC 测试键常量（ASSEMBLY_TEST_KEY）
// 已出仓退休。信封密钥现由构建期注入链接入签名 bundle 的 `security` 节
// （configs/security.toml；键值不落文件），运行期双 bin 经
// cfg_bindings::fetch_control_chain_key 取得，全零/短键/缺节 ⇒
// CHAIN-KEY-MISSING 拒载（fail-closed）。开发档测试键模板：
// configs/kms-dev/control-chain.key（值不经任何 .rs 源文件）。
/// 产品租约 ID（两侧同源预置，§6.7）。
pub const PROD_LEASE_ID: [u8; 16] = [0x40; 16];
/// fencing epoch 种子（两侧同源；首签 token = seed + 1）。
pub const PROD_EPOCH_SEED: u64 = 7;
/// 租约 TTL（秒）。
pub const PROD_LEASE_TTL_SECS: u64 = 3_600;
/// 策略版本（生效 generation 恰等于此值，profile 规范习语保证）。
pub const PROD_POLICY_VERSION: u64 = 3;

/// 产品同源档案单出口：AI → 绑定设备/点的 PccBreaker ±500 kW Switch 链。
/// 写目标（设备/点）由 45 卷点表 Control 点绑定（svc-config 注入）；
/// 不可得时调用方传 0/0 fail-closed（default-deny）。
pub fn prod_profile(device_id: u32, point_id: u32) -> eneros_control_gateway::ControlProfile {
    let mut profile = eneros_control_gateway::ControlProfile::ai_pcs_setpoint(
        device_id,
        point_id,
        PROD_POLICY_VERSION,
        PROD_LEASE_ID,
        PROD_EPOCH_SEED,
    );
    profile.lease_ttl_secs = PROD_LEASE_TTL_SECS;
    profile
}

/// dispatch 侧产品链（完整装配：policy 规则 + lease 签发 + safety 注册）。
/// `key`/`device_id`/`point_id` 由装配方（svc bin）自注入源解析
/// （47.2 点表绑定；47.3 密钥注入）。
pub fn prod_chain(
    key: &[u8; 32],
    device_id: u32,
    point_id: u32,
    now_ms: u64,
) -> eneros_control_gateway::chain::ControlChain {
    use eneros_control_gateway::chain::ControlChain;
    let profile = prod_profile(device_id, point_id);
    let mut chain = ControlChain::from_profile(key, &profile);
    // 与旧实现一致：签发失败不阻断装配（复验侧由 lease_ok/租约未知面兜底
    // default-deny）；蓝图 55 行为保持 D1。
    let _ = profile.issue_lease(&mut chain, now_ms / 1000);
    chain
}

#[cfg(test)]
mod tests {
    use eneros_control_dispatch::{DispatchEngine, InMemoryIntentStore};

    use super::*;

    // 47（WP-47.2）：测试装配绑定（点表绑定语义由 cfg_bindings 承载；
    // 单测固定夹具值）。
    const TEST_DEVICE_ID: u32 = 1;
    const TEST_POINT_ID: u32 = 10;
    use crate::ai_worker::{AiWorkerService, AI_PROPOSE};

    // 复用 ai_worker 测试的 mock 后端不可见——此处定义本地后端。
    pub struct LocalBackend;
    impl crate::ai_worker::DecisionBackend for LocalBackend {
        fn decide(
            &mut self,
            _t: &eneros_safety_validator::TrustedTelemetry,
            now_ms: u64,
        ) -> Result<Vec<u8>, crate::ai_worker::DecisionReject> {
            // 蓝图 55：单时段计划构造器单源。
            let plan = eneros_dual_brain::ProposedControlPlan::single_entry(
                String::from("req-svc-1"),
                eneros_dual_brain::DecisionPath::Fast,
                5.0,
                0.5,
                eneros_solver_core::result::SolveStatus::Optimal,
                60,
                now_ms,
                None,
            );
            Ok(crate::ai_worker::encode_candidate_payload(
                &crate::ai_worker::proposal_to_candidate_unbound(&plan).expect("stub cand"),
            ))
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

    /// host 参考装配 DTO（迁移期：生产装配在 dispatch_assembly 注入
    /// resource-registry PlannerView；[08] §4.6 旧结构降内部 import DTO）。
    #[derive(Clone, Copy)]
    struct HostResourceView {
        device_id: u16,
        point_id: u32,
        unit_code: u16,
        catalog_version: u64,
    }

    impl eneros_decision_contract::ResourceRegistry for HostResourceView {
        fn asset_exists(&self, asset_id: u64) -> bool {
            asset_id == u64::from(self.device_id)
        }

        fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
            if asset_id == u64::from(self.device_id) {
                Some((self.device_id, self.point_id, self.unit_code))
            } else {
                None
            }
        }

        fn catalog_version(&self) -> u64 {
            self.catalog_version
        }
    }

    fn engine() -> DispatchEngine {
        engine_with_catalog_version(0)
    }

    /// WP-69.4.1：catalog_version 可变引擎装配。旧 facade 读臂硬编码
    /// `catalog_version=1`（engine.rs `run`）——t21 用 1 使删除前旧臂可
    /// 真达 Committed（红测语义 =「旧臂是活入口」）；主路夹具恒 0。
    fn engine_with_catalog_version(catalog_version: u64) -> DispatchEngine {
        // 简化装配：budget 宽松；delivery 回 Applied（code 0）。
        // 蓝图 55：档案单源（TTL 600/代次 4 为本测试变体，值语义与旧
        // 手抄 build_chain 一致）。
        let outcome = eneros_device_agent::OutcomeFrame {
            code: eneros_device_agent::CODE_APPLIED,
            class: 0,
        }
        .encode()
        .to_vec();
        let key: &[u8] = b"w40-svc-key-000000000000000000000000";
        let profile = eneros_control_gateway::ControlProfile {
            lease_ttl_secs: 600,
            ..eneros_control_gateway::ControlProfile::ai_pcs_setpoint(
                TEST_DEVICE_ID,
                TEST_POINT_ID,
                4,
                [0x40; 16],
                7,
            )
        };
        let mut chain = eneros_control_gateway::chain::ControlChain::from_profile(key, &profile);
        let _ = profile.issue_lease(&mut chain, 1_700_000_000);
        // 蓝图 61 v0.314.35：host 参考装配视图 = 内部 DTO（生产装配在
        // dispatch_assembly 注入 PlannerView——[08] §4.8 门）。
        // 蓝图 69.4 WP-69.4.4：经 src 侧 ConfigSeed 装配（安全默认单源；
        // 本夹具只声明资源/合同视图与 catalog_version 变体）。
        use eneros_control_dispatch::compiler::{MapContractView, PlanContract};
        let mut contracts = MapContractView::new();
        contracts.insert(
            u64::from(TEST_DEVICE_ID),
            PlanContract {
                class: 2,
                readback_point_id: 1001,
                reversible: 1,
                min: -500.0,
                max: 500.0,
                settle_ms: 0,
                compensable: false,
            },
        );
        DispatchEngine::new(
            eneros_control_dispatch::ConfigSeed {
                chain,
                key: key.to_vec(),
                fencing_token: 8,
                issuer_service: [0x41; 16],
                now_fn: || 1_700_000_000_000,
                intent_log: Box::new(InMemoryIntentStore::new(64)),
                delivery: Box::new(MemoryChannel { outcome }),
                resources: Box::new(HostResourceView {
                    device_id: TEST_DEVICE_ID as u16,
                    point_id: TEST_POINT_ID,
                    unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
                    catalog_version,
                }),
                contracts: Box::new(contracts),
                profile,
            }
            .assemble(),
        )
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
        // 蓝图 66 WP-66.3.1：actor 语义源收编（匿名 `push(2)` 已删）。
        out.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
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
        // 蓝图 61：候选 wire 变长——tail 定位（tel+actor 固定尾段）。
        let tel_len = crate::telemetry::TRUSTED_WIRE_LEN;
        let cand_len = req.len() - 1 - tel_len - 1;
        let t = crate::telemetry::decode_trusted(&req[1 + cand_len..1 + cand_len + tel_len])
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

    // ============ 蓝图 69.4 WP-69.4.1：旧 70B proposal wire 读臂退役 ====

    /// 旧 70B proposal wire 请求（v0.314.32 已停止写入；v0.329.0 读臂退役）。
    /// 本地定长常量自持——不依赖任何仓内常量（后者已随读臂删除）。
    fn legacy_request() -> Vec<u8> {
        const RETIRED_WIRE_LEN: usize = 70;
        let t = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        let mut legacy = vec![0u8; RETIRED_WIRE_LEN];
        let id = b"req-legacy-70b";
        legacy[..id.len()].copy_from_slice(id);
        legacy[16] = 1; // DecisionPath::Fast（旧 wire 线码）
        legacy[17..25].copy_from_slice(&5.0f64.to_le_bytes());
        legacy[25..29].copy_from_slice(&300u32.to_le_bytes());
        legacy[29..37].copy_from_slice(&1_700_000_000_000u64.to_le_bytes());
        legacy[37..69].fill(0xAA); // input_digest（全零会被拒）
        legacy[69] = 0; // solve_status Optimal
        let mut out =
            Vec::with_capacity(1 + RETIRED_WIRE_LEN + crate::telemetry::TRUSTED_WIRE_LEN + 1);
        out.push(DISP_SUBMIT);
        out.extend_from_slice(&legacy);
        out.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        out.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
        out
    }

    /// WP-69.4.1：`CAND` 主路是唯一活入口。删除前后都必须绿——
    /// ①候选 wire 可达 Committed；②非候选且非旧定长的垃圾体被拒绝。
    #[test]
    fn t20_typed_candidate_path_is_the_only_live_ingress() {
        let mut eng = engine();
        let resp = handle_submit(&mut eng, &request_from_worker());
        let (status, state, _c, _l, note) = decode_receipt(&resp).expect("receipt");
        assert_eq!(status, DISP_OK, "note={note}");
        assert_eq!(state, DispatchState::Committed, "note={note}");
        // 垃圾体（非 `CAND` magic、长度不在任何已识别形态）⇒ 显式拒绝。
        let mut eng2 = engine();
        let t = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        let mut req = vec![DISP_SUBMIT];
        req.extend_from_slice(b"garbage!");
        req.extend_from_slice(&crate::telemetry::encode_trusted(&t));
        req.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
        let resp = handle_submit(&mut eng2, &req);
        assert_eq!(resp[0], DISP_ERR, "非候选计划体必须被拒绝");
    }

    /// WP-69.4.1：旧 70B 定长体在读臂删除后必须被显式拒绝（fail-closed，
    /// 不得静默成功）。删除前本测**红**（旧臂还在时走 facade 成功），
    /// 删除后转绿——红测先行的具体形态。
    #[test]
    fn t21_non_candidate_plan_body_is_rejected_after_legacy_removal() {
        // catalog_version=1 与旧 facade 硬编码常量对齐——删除前旧臂可真达
        // Committed（红），删除后同一请求被尾部拒绝（绿）。
        let mut eng = engine_with_catalog_version(1);
        let resp = handle_submit(&mut eng, &legacy_request());
        let (status, _state, _c, _l, note) = decode_receipt(&resp).expect("receipt");
        assert_eq!(status, DISP_ERR, "旧 70B 形态必须被拒绝，note={note}");
        assert!(
            note.contains("CAND"),
            "拒绝回执须可诊断（指向唯一入口），note={note}"
        );
    }

    /// WP-69.4.1：主路词形字面锁（Z2）。`TYPED`/`TYPED-ERR` 两条串口词形
    /// 逐字节冻结（探针经分段拼接构造，防自匹配）；已退役的
    /// `SUBMIT …`/`SUBMIT-ERR` 词形复现即红（词形退役不可逆）。
    #[test]
    fn t22_typed_word_form_literal_lock() {
        let src = include_str!("dispatch_service.rs");
        let typed = concat!("[svc-control-dispatch] ", "TYPED state={:?} note={:?}");
        let typed_err = concat!("[svc-control-dispatch] ", "TYPED-ERR {e:?}");
        assert!(
            src.contains(typed),
            "TYPED 词形漂移（Z2 主路词形逐字节冻结）"
        );
        assert!(
            src.contains(typed_err),
            "TYPED-ERR 词形漂移（Z2 主路词形逐字节冻结）"
        );
        let submit = concat!("[svc-control-dispatch] ", "SUBMIT state");
        let submit_err = concat!("[svc-control-dispatch] ", "SUBMIT-ERR");
        assert!(
            !src.contains(submit) && !src.contains(submit_err),
            "已退役词形 SUBMIT/SUBMIT-ERR 复现（v0.329.0 随旧读臂退役）"
        );
    }

    // ============ 47（WP-47.1）协议写意图 ingress ============

    /// 意图请求（蓝图 66 WP-66.3.1：帧壳单源 encode_intent_request，
    /// 独立手拼已删）。
    fn intent_request(intent: &eneros_protocol_abstract::write::PointWriteIntent) -> Vec<u8> {
        eneros_protocol_abstract::write::encode_intent_request(intent)
    }

    /// 引用锁（蓝图 66 WP-66.3.1 判据：opcode 单源 + 帧字面零命中）。
    /// ① 命名常量与 telemetry-model 单源常量恒等；
    /// ② 本文件意图帧手拼字面（opcode/actor 匿名 `push`、actor 匿名
    /// 比较）零命中——漂移即红。（探针字面经分段拼接构造，防自匹配。）
    #[test]
    fn disp_submit_intent_locked_to_intent_request_opcode() {
        use eneros_telemetry_model::command::INTENT_REQUEST_OPCODE;
        assert_eq!(
            DISP_SUBMIT_INTENT, INTENT_REQUEST_OPCODE,
            "命名常量必须与单源 opcode 恒等（值冻结）"
        );
        let push2 = concat!("push", "(", "2", ");");
        let actor_ne2 = concat!("actor_kind", " !", "= ", "2");
        let src = include_str!("dispatch_service.rs");
        let offenders: Vec<&str> = src
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                (t.contains(push2) && !t.contains("//")) || t.contains(actor_ne2)
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "意图帧手拼字面复现（蓝图 66 已收编单源）：{offenders:?}"
        );
    }

    #[test]
    fn submit_intent_routes_to_engine_committed() {
        let mut engine = engine();
        let tel = crate::ai_worker::tests_helpers::minimal_trusted_at(1_700_000_000_000);
        let intent = eneros_protocol_abstract::write::PointWriteIntent::new(
            "mqtt",
            TEST_DEVICE_ID as u16,
            TEST_POINT_ID,
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
            TEST_DEVICE_ID as u16,
            TEST_POINT_ID,
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
            TEST_DEVICE_ID as u16,
            TEST_POINT_ID,
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
