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
use eneros_control_dispatch::evidence;
use eneros_control_dispatch::ControlDispatch;
#[cfg(test)]
use eneros_control_dispatch::DeliveryChannel;

use crate::telemetry::decode_trusted;

/// 蓝图 61 v0.314.35：proposal → CandidatePlan（resource_catalog_version
/// 由调用方注入——planner 输入与 Registry 同代；[08] §4.8 门）。
/// C2 收口（2026-09-24）：绑定全零（未绑定迁移形态）——asset/device/
/// point/unit 由 dispatch 编译期经 ResourceRegistry 解析；位置实参四元组
/// 随 legacy facade 入口退役。
pub fn candidate_from_proposal(
    plan: &eneros_dual_brain::decision::ProposedControlPlan,
    catalog_version: u64,
) -> Result<eneros_decision_contract::CandidatePlan, alloc::string::String> {
    eneros_control_dispatch::adapter::proposal_to_candidate_unbound(plan, catalog_version)
        .map_err(|e| alloc::format!("{e:?}"))
}

/// 请求码。
pub const DISP_SUBMIT: u8 = 1;
/// 47（WP-47.1 协议入站写）：协议写意图提交（intent wire + actor）。
pub const DISP_SUBMIT_INTENT: u8 = 2;
/// 蓝图 70.8 WP-70.8.4：**带可信快照输入**的 typed 提交（生产唯一入口）
/// ——布局 `[3][CAND wire][TrustedInput wire 234B][actor 1B]`。引擎在入口
/// 验证候选与可信输入同代（快照/目录/时域/摘要），不匹配整批拒绝。
pub const DISP_SUBMIT_TRUSTED: u8 = 3;
/// `[4][optional TrustedInput]`: advance/cancel/reconcile accepted plans.
pub const DISP_HELD_TICK: u8 = 4;
/// `[5][batch index]`: bounded per-command report, never a new submission.
pub const DISP_HELD_QUERY: u8 = 5;

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
    if code == DISP_HELD_TICK {
        let input = if req.len() == 1 {
            None
        } else {
            let Some(input) = eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&req[1..])
            else {
                return err_receipt("bad held tick input");
            };
            Some(input)
        };
        let telemetry = input
            .as_ref()
            .and_then(|input| decode_trusted(input.telemetry()));
        if input.is_some() && telemetry.is_none() {
            return err_receipt("bad held tick telemetry");
        }
        return match engine.tick_held(input.as_ref(), telemetry.as_ref()) {
            Ok(active) => alloc::vec![DISP_OK, active.min(u8::MAX as usize) as u8],
            Err(error) => err_receipt(&format!("{error:?}")),
        };
    }
    if code == DISP_HELD_QUERY {
        if req.len() != 2 {
            return err_receipt("bad held query");
        }
        return match engine.query_held() {
            Ok(reports) => match reports.get(req[1] as usize) {
                Some(report) => {
                    let mut out = alloc::vec![DISP_OK];
                    out.extend_from_slice(b"HQR1");
                    out.extend_from_slice(&report.batch_id);
                    out.push(report.final_state as u8);
                    out.extend_from_slice(&report.events.to_le_bytes());
                    out.push(report.commands.len() as u8);
                    for command in &report.commands {
                        out.extend_from_slice(&command.intent_id);
                        out.push(command.state as u8);
                        out.extend_from_slice(&command.attempts.to_le_bytes());
                        out.push(u8::from(command.readback.is_some()));
                        out.extend_from_slice(&command.readback.unwrap_or(0.0).to_le_bytes());
                        let reason = command.reason.as_deref().unwrap_or("").as_bytes();
                        let len = reason.len().min(128);
                        out.push(len as u8);
                        out.extend_from_slice(&reason[..len]);
                    }
                    out
                }
                None => err_receipt("held batch not found"),
            },
            Err(error) => err_receipt(&format!("{error:?}")),
        };
    }
    // 蓝图 70.8 WP-70.8.4：可信输入形态（生产入口）。
    if code == DISP_SUBMIT_TRUSTED {
        return handle_submit_trusted(engine, &req[1..]);
    }
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
    // R14-A：typed/trusted 入口的投递标签/清算超时改由引擎内部自派生
    // （原此处经单源常量传参——生产从未传过第二值，形参随 trait 收窄
    // 退役）；intent 入口仍显式传超时（下方绑定）。
    // 单读判别：唯一入口 = `CAND` magic（CandidatePlan V1）。
    if plan_len >= 4 && &body[0..4] == b"CAND" {
        let Ok(cand) = eneros_decision_contract::CandidatePlan::decode_v1(&body[..plan_len]) else {
            return err_receipt("bad candidate");
        };
        return match engine.submit_typed(&cand, &telemetry, &actor) {
            Ok(receipt) => {
                if !receipt.final_state.is_committed() {
                    eneros_runtime::println!(
                        "{}",
                        evidence::receipt_state_note(
                            evidence::WORD_TYPED,
                            receipt.final_state,
                            &receipt.note
                        )
                    );
                }
                encode_receipt(&receipt)
            }
            Err(e) => {
                eneros_runtime::println!("{}", evidence::submit_err(evidence::WORD_TYPED, &e));
                err_receipt(&format!("{e:?}"))
            }
        };
    }
    // 蓝图 69.4 v0.329.0：旧 70B proposal wire 读臂已删除（v0.314.32
    // 承诺的退役）——非候选形态 fail-closed 显式拒绝（如实回执，不得
    // 静默成功；t21 锁此语义）。
    err_receipt("plan body not CAND (legacy proposal wire retired v0.329.0)")
}

/// 蓝图 70.8 WP-70.8.4：`[CAND wire][TrustedInput wire][actor]` → 引擎
/// 可信准入提交。任一段解码失败 → 结构化错误回执（零投递）。
pub fn handle_submit_trusted(engine: &mut dyn ControlDispatch, body: &[u8]) -> Vec<u8> {
    use eneros_agent_sdk::snap_input_wire::{TrustedInput, SNAP_INPUT_WIRE_LEN};
    let in_len = SNAP_INPUT_WIRE_LEN;
    if body.len() <= in_len + 1 {
        return err_receipt("short trusted request");
    }
    let plan_len = body.len() - in_len - 1;
    let Some(input) = TrustedInput::decode(&body[plan_len..plan_len + in_len]) else {
        return err_receipt("bad trusted input");
    };
    let actor_kind = body[plan_len + in_len];
    if actor_kind != eneros_telemetry_model::command::ActorKind::Ai as u8 {
        return err_receipt("actor must be Ai");
    }
    let actor = AuthenticatedActor {
        kind: eneros_control_gateway::envelope::ActorKind::Ai,
        service: [0u8; 16],
        session_epoch: 0,
    };
    if plan_len < 4 || &body[0..4] != b"CAND" {
        return err_receipt("plan body not CAND");
    }
    let Ok(cand) = eneros_decision_contract::CandidatePlan::decode_v1(&body[..plan_len]) else {
        return err_receipt("bad candidate");
    };
    // 未绑定分配（asset 0）的解析由引擎在认证/编译前统一完成（单点，
    // 见 `adapter::resolve_unbound_assets`）——服务层不做二次改写。
    // 遥测载体 = 同一输入 wire 内嵌的 144B 载荷（决策侧与执行侧同一
    // 冻结产物——不再有第二个快照请求）。
    let Some(telemetry) = decode_trusted(input.telemetry()) else {
        return err_receipt("bad trusted telemetry payload");
    };
    match engine.submit_trusted(&cand, &input, &telemetry, &actor) {
        Ok(receipt) => {
            if !receipt.final_state.is_committed() {
                eneros_runtime::println!(
                    "{}",
                    evidence::receipt_state_note(
                        evidence::WORD_TRUSTED,
                        receipt.final_state,
                        &receipt.note
                    )
                );
            }
            encode_receipt(&receipt)
        }
        Err(e) => {
            eneros_runtime::println!("{}", evidence::submit_err(evidence::WORD_TRUSTED, &e));
            err_receipt(&format!("{e:?}"))
        }
    }
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
    // R13-B：逐意图提交路径同用单源清算超时（与 typed/trusted/持有/恢复
    // 全路径一致——收编前此处为第三处手抄同值字面）。
    let deadline_ms = eneros_control_dispatch::engine::HELD_DELIVERY_SETTLE_MS;
    match engine.submit_intent(&intent, tel, &actor, deadline_ms) {
        Ok(receipt) => {
            eneros_runtime::println!(
                "{}",
                evidence::receipt_state_note(
                    evidence::WORD_INTENT,
                    receipt.final_state,
                    &receipt.note
                )
            );
            encode_receipt(&receipt)
        }
        Err(e) => {
            eneros_runtime::println!("{}", evidence::submit_err(evidence::WORD_INTENT, &e));
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
    use eneros_control_dispatch::{DispatchConfig, DispatchEngine, InMemoryIntentStore};

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
            _input: &crate::ai_worker::DecisionInput<'_>,
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
                &crate::ai_worker::proposal_to_candidate_unbound(&plan, 0).expect("stub cand"),
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
        DispatchEngine::new(with_fixture_cert(engine_config(catalog_version)))
    }

    #[allow(dead_code)]
    fn engine_without_fixture_cert(catalog_version: u64) -> DispatchEngine {
        // 简化装配：budget 宽松；delivery 回 Applied（code 0）。
        // 蓝图 55：档案单源（TTL 600/代次 4 为本测试变体，值语义与旧
        // 手抄 build_chain 一致）。
        let _outcome = eneros_device_agent::OutcomeFrame {
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
        DispatchEngine::new(engine_config(catalog_version))
    }

    /// 夹具认证来源（蓝图 70.9 测试缝；真实评估器出证）。
    struct FixtureCert {
        inner: eneros_certifier::FixtureCertifier,
    }
    impl FixtureCert {
        fn new(key: &[u8]) -> Self {
            Self {
                inner: eneros_certifier::FixtureCertifier::new(key, 100.0, -500.0, 500.0, 0.5),
            }
        }
        fn ring(&self) -> eneros_certifier::proof::CertifierKeyRing {
            self.inner.ring().clone()
        }
    }
    impl eneros_control_dispatch::CertificateSource for FixtureCert {
        fn certify(
            &mut self,
            plan_wire: &[u8],
            now_ms: u64,
        ) -> Result<eneros_control_dispatch::CertifierReply, eneros_control_dispatch::CertifyReject>
        {
            let plan = eneros_decision_contract::CandidatePlan::decode_v1(plan_wire)
                .map_err(|e| eneros_control_dispatch::CertifyReject::Rejected(format!("{e:?}")))?;
            let r = self
                .inner
                .certify(&plan, now_ms)
                .map_err(|e| eneros_control_dispatch::CertifyReject::Rejected(format!("{e:?}")))?;
            Ok(eneros_control_dispatch::CertifierReply {
                certificate: r.certificate,
                key_id: r.key_id,
                key_version: r.key_version,
                mac: r.mac,
            })
        }
    }

    /// 夹具认证来源注入（测试单源）。
    fn with_fixture_cert(config: DispatchConfig) -> DispatchConfig {
        let c = FixtureCert::new(b"fixture-cert-key-0123456789abcdef");
        let ring = c.ring();
        DispatchConfig {
            certifier: Box::new(c),
            certifier_ring: ring,
            ..config
        }
    }

    /// 引擎配置装配（**迁移轨**：`LegacyMigration` 策略——本模块历史
    /// 用例消费旧派生身份；可信输入准入（`Required`）的用例自建配置）。
    fn engine_config(catalog_version: u64) -> DispatchConfig {
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
        let config = eneros_control_dispatch::ConfigSeed {
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
            // 单资产测试夹具（多资产由 dispatch_assembly 生产装配承载）。
            extra_routes: alloc::vec::Vec::new(),
        }
        .assemble();
        DispatchConfig {
            trusted_input: eneros_control_dispatch::TrustedInputPolicy::LegacyMigration,
            ..config
        }
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

    // ========================================================================
    // 蓝图 70.8 WP-70.8.4：可信输入入口（DISP_SUBMIT_TRUSTED）端到端
    // ========================================================================

    /// 生产形态：worker 经 `AI_PROPOSE_SNAP`（可信输入）产出**绑定**候选，
    /// dispatch 经 `DISP_SUBMIT_TRUSTED` 验证身份后提交 → Committed。
    /// 同时证明 worker 侧候选的 snapshot_id/objective_digest 来自冻结快照
    /// 身份而非自造值。
    #[test]
    fn trusted_submit_end_to_end_commits_and_binds_identity() {
        use eneros_agent_sdk::snap_input_wire::{
            SnapshotFacts, TrustedInput, COMPLETE, SNAP_INPUT_WIRE_LEN,
        };
        // ① 冻结快照身份（与引擎时钟一致：produced_at = now）。
        let now_ms = 1_700_000_000_000u64;
        let facts = SnapshotFacts {
            snapshot_id: [0x5C; 16],
            provenance_root: [0x3D; 32],
            cut_time_ns: now_ms * 1_000_000,
            produced_at_ns: now_ms * 1_000_000,
            catalog_version: 1, // 与引擎 resources 视图的 catalog_version 一致
            catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
            accepted: 7,
            missing: 0,
            completeness: COMPLETE,
        };
        let t = crate::ai_worker::tests_helpers::minimal_trusted_at(now_ms);
        let payload = crate::telemetry::encode_trusted(&t);
        let mut telemetry = [0u8; 144];
        telemetry.copy_from_slice(&payload);
        let input = TrustedInput::new(facts, telemetry);

        // ② worker：AI_PROPOSE_SNAP → 绑定候选（启发式后端 = 生产有界
        //    后端；测试后端 LocalBackend 为未绑定形态，不能证绑定面）。
        let mut worker =
            AiWorkerService::new(crate::ai_worker::HeuristicBackend::new(), [0xAA; 16]);
        let mut wreq = vec![crate::ai_worker::AI_PROPOSE_SNAP];
        wreq.extend_from_slice(&input.encode());
        let wresp = worker.propose(&wreq, now_ms);
        assert_eq!(wresp[0], crate::ai_worker::AI_OK);
        let cand =
            eneros_decision_contract::CandidatePlan::decode_v1(&wresp[1..]).expect("bound cand");
        assert_eq!(
            cand.snapshot_id().0,
            facts.snapshot_id,
            "候选身份 = 快照身份"
        );
        assert_eq!(
            cand.objective_digest(),
            &eneros_decision_contract::objective_binding_v1(
                &facts.snapshot_id,
                facts.catalog_version,
                now_ms,
                0,
                1
            ),
            "候选目标摘要 = 绑定公式输出"
        );

        // ③ dispatch：DISP_SUBMIT_TRUSTED → Committed（同代准入通过）。
        let mut eng = engine_with_catalog_version(1);
        let mut req = vec![DISP_SUBMIT_TRUSTED];
        req.extend_from_slice(&wresp[1..]);
        req.extend_from_slice(&input.encode());
        req.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
        let resp = handle_submit(&mut eng, &req);
        let (status, state, _c, _l, note) = decode_receipt(&resp).expect("receipt");
        assert_eq!(status, DISP_OK, "note={note}");
        assert_eq!(state, DispatchState::Committed, "note={note}");

        // ④ 负例：同一候选 + **另一代**输入 → 拒绝（零投递）。
        let mut eng2 = engine_with_catalog_version(1);
        let other = TrustedInput::new(
            SnapshotFacts {
                snapshot_id: [0x77; 16],
                ..facts
            },
            telemetry,
        );
        let mut req2 = vec![DISP_SUBMIT_TRUSTED];
        req2.extend_from_slice(&wresp[1..]);
        req2.extend_from_slice(&other.encode());
        req2.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
        let resp2 = handle_submit(&mut eng2, &req2);
        assert_eq!(resp2[0], DISP_ERR, "跨代输入必须拒绝");

        // ⑤ 负例：截断的可信输入 → 显式拒绝（不猜）。
        let mut eng3 = engine_with_catalog_version(1);
        let mut req3 = vec![DISP_SUBMIT_TRUSTED];
        req3.extend_from_slice(&wresp[1..]);
        req3.extend_from_slice(&input.encode()[..SNAP_INPUT_WIRE_LEN - 1]);
        req3.push(eneros_telemetry_model::command::ActorKind::Ai as u8);
        assert_eq!(handle_submit(&mut eng3, &req3)[0], DISP_ERR);
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

    /// WP-69.4.1：主路词形锁（Z2；C7 收口后锁**表**）。提交回执族
    /// （TYPED/TRUSTED/INTENT ± ERR 六形）词形唯一事实源 =
    /// `eneros_control_dispatch::evidence` 表——调用点零字面，正臂为表
    /// golden 等值（重构搬动调用点不再打翻锁；词形漂移必发生在表且必红）；
    /// 负臂两道：已退役 `SUBMIT …`/`SUBMIT-ERR` 词形复现即红（词形退役
    /// 不可逆）+ 调用点手抄格式片段回流即红（防词形双源）。
    #[test]
    fn t22_typed_word_form_literal_lock() {
        use eneros_control_dispatch::DispatchError;
        // 正臂：表 golden（与 QEMU 门 grep 的词形逐字节一致）。
        assert_eq!(
            evidence::receipt_state_note(evidence::WORD_TYPED, DispatchState::Failed, &None),
            concat!("[svc-control-dispatch] ", "TYPED state=Failed note=None")
        );
        assert_eq!(
            evidence::submit_err(evidence::WORD_TYPED, &DispatchError::DeliveryTimeout),
            concat!("[svc-control-dispatch] ", "TYPED-ERR DeliveryTimeout")
        );
        assert_eq!(
            evidence::receipt_state_note(
                evidence::WORD_TRUSTED,
                DispatchState::Indeterminate,
                &None
            ),
            concat!(
                "[svc-control-dispatch] ",
                "TRUSTED state=Indeterminate note=None"
            )
        );
        assert_eq!(
            evidence::submit_err(evidence::WORD_TRUSTED, &DispatchError::DeliveryUnreachable),
            concat!("[svc-control-dispatch] ", "TRUSTED-ERR DeliveryUnreachable")
        );
        assert_eq!(
            evidence::receipt_state_note(evidence::WORD_INTENT, DispatchState::Rejected, &None),
            concat!("[svc-control-dispatch] ", "INTENT state=Rejected note=None")
        );
        assert_eq!(
            evidence::submit_err(evidence::WORD_INTENT, &DispatchError::DeliveryTimeout),
            concat!("[svc-control-dispatch] ", "INTENT-ERR DeliveryTimeout")
        );
        // 负臂①：已退役词形复现即红（源扫描——禁词在任何重构形态下都不合法）。
        let src = include_str!("dispatch_service.rs");
        let submit = concat!("[svc-control-dispatch] ", "SUBMIT state");
        let submit_err = concat!("[svc-control-dispatch] ", "SUBMIT-ERR");
        assert!(
            !src.contains(submit) && !src.contains(submit_err),
            "已退役词形 SUBMIT/SUBMIT-ERR 复现（v0.329.0 随旧读臂退役）"
        );
        // 负臂②：调用点不得手抄格式片段（应引 evidence 表——防词形双源）。
        // 针经分段拼接构造（防自咬：golden 用具体值，不含格式片段）。
        let state_fmt: String = ["state=", "{", ":?}"].concat();
        let err_fmt: String = ["-ERR {", "e:?}"].concat();
        assert!(
            !src.contains(&state_fmt) && !src.contains(&err_fmt),
            "提交回执族词形手抄回流（应引 evidence::receipt_state_note/submit_err）"
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

    /// 守卫扩面（蓝图 75 卷二 WP-75.2.0）：EA tick 构帧点的 actor 语义
    /// 单源——`energy_agent_service.rs` 构帧 push 须走
    /// `ActorKind::Ai as u8`，匿名 `push(2)` 字面零命中（漂移即红）。
    /// 字节恒等前提同锁（Ai=2，wire 零变化）。
    /// 针经分段拼接构造（防自咬）；扫描目标为 EA 文件整体（含行尾注释
    /// 形态，不按 `//` 豁免——收编后该字面任何形态都不合法）。
    #[test]
    fn ea_tick_actor_kind_single_source_guard() {
        assert_eq!(
            eneros_telemetry_model::command::ActorKind::Ai as u8,
            2,
            "ActorKind::Ai 判别值冻结（wire 字节恒等前提）"
        );
        let push2 = concat!("push", "(", "2", ")");
        let src = include_str!("energy_agent_service.rs");
        let offenders: Vec<&str> = src.lines().filter(|l| l.contains(push2)).collect();
        assert!(
            offenders.is_empty(),
            "EA 构帧匿名 actor 字面复现（WP-75.2.0 已收编 ActorKind 单源）：{offenders:?}"
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
    // （2026-09-24 R14-A 退役）原 R13-B 源文本守卫
    // `dispatch_service_delivery_uses_shared_constants`：typed/trusted
    // 入口的标签/超时形参随 trait 收窄退役，typed/trusted 已无常量引用
    // 点。intent 入口仍显式传超时——复查轮恢复窄守卫（仅锁本文件裸
    // 超时字面；标签字面检查随 typed/trusted 退役一并撤销）。
    /// intent 入口清算超时须引单源常量；裸字面回流即红（针经字符数组
    /// 构造——防自咬，断言文本不得包含连续手抄字面，R13-B 同技法）。
    #[test]
    fn intent_delivery_deadline_uses_shared_constant() {
        let src = include_str!("dispatch_service.rs");
        assert!(
            src.contains("engine::HELD_DELIVERY_SETTLE_MS"),
            "intent 逐意图清算超时须引用单源常量（R13-B/R14 复查轮）"
        );
        // 针经字符数组构造（防自咬）。
        let hand_a: String = ['5', '_', '0', '0', '0'].iter().collect();
        let hand_b: String = ['5', '0', '0', '0'].iter().collect();
        assert!(
            !src.contains(&hand_a) && !src.contains(&hand_b),
            "intent 清算超时手抄字面回归（应引 HELD_DELIVERY_SETTLE_MS）"
        );
    }
}
