//! 70.6（WP-70.6.1）：管理动作能力表与可信操作账本。
//!
//! - **能力表**：22 个 MgmtAction 逐一标注 `Unsupported / ReadOnly /
//!   SideEffect / Stream` 与支持说明——"6 个白名单"不再是交付面口径；
//!   未接实的危险动作保持显式 Unsupported（fail-closed 不空成功）。
//! - **操作账本**：副作用操作绑定 principal/request_id/请求摘要/配置
//!   代次，`Running → Completed | Failed` 终态单写；同 (principal,
//!   request_id) 同摘要 = 幂等回放（返回已记录终态，不二次执行），同
//!   request_id 不同摘要 = 幂等冲突拒绝；表满 = 副作用背压拒绝（只读
//!   不受限）。持久化形态 = [`MgmtOpsState`]（Encode/Decode，经
//!   RegionStateStore 落 `DataRegion::MgmtOps`）；恢复时在途 Running
//!   记录诚实转 Unknown（附可继续对账的操作号，不冒称已完成）。
//!
//! 账本即唯一幂等事实源：svc-mgmt 的易失 replay 窗口（mgmt_rpc）只
//! 承担无账本动作（只读/未知）的防重放，不冒充已执行操作账。

use alloc::string::String;
use alloc::vec::Vec;

use crate::mgmt_rpc::MgmtAction;

/// 动作能力分类（WP-70.6.1 能力表的值域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionCapability {
    /// 未接实：显式拒绝（`action-not-wired` / 专属词形），无副作用。
    Unsupported,
    /// 只读查询/诊断：无副作用，无操作记录（仅审计）。
    ReadOnly,
    /// 副作用操作：必须入账本（幂等/对账/重启回查）。
    SideEffect,
    /// 流式/分页读取：快照代次绑定，无副作用。
    Stream,
}

/// 单动作能力行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionCap {
    pub capability: ActionCapability,
    /// 支持说明（能力表披露口径；执行器词形与此对齐）。
    pub note: &'static str,
}

const fn ro(note: &'static str) -> ActionCap {
    ActionCap {
        capability: ActionCapability::ReadOnly,
        note,
    }
}
const fn unsupported(note: &'static str) -> ActionCap {
    ActionCap {
        capability: ActionCapability::Unsupported,
        note,
    }
}
const fn side_effect(note: &'static str) -> ActionCap {
    ActionCap {
        capability: ActionCapability::SideEffect,
        note,
    }
}

/// 22 动作能力表（单一事实源——svc-mgmt 执行器与披露文档同源）。
///
/// 70.6 接实面：既有 6 白名单 + MaintenanceEnter/Exit + AgentRestart 升级
/// 为真实 root 执行 + ConfigSet（stage/activate 事务）。其余动作保持
/// Unsupported 并写明缺失原因——不把"接口存在"外推为"能力交付"。
pub fn action_capability(action: &MgmtAction) -> ActionCap {
    match action {
        // ===== 只读查询 =====
        MgmtAction::StatusQuery => ro("service status; arg: ''|'op:<id>'|'ops'|'caps'"),
        MgmtAction::SloQuery => unsupported("slo:action-not-wired（无 SLO 观测源）"),
        MgmtAction::AgentList => unsupported("agent-list:action-not-wired（服务表在 root）"),
        MgmtAction::LogQuery => ro("logging config payload; arg 'runtime' = 未接实显式拒绝"),
        MgmtAction::HistoryQuery => ro("telemetry history page（SPEC-C 二进制行区）"),
        MgmtAction::CrashDumpSymbolize => {
            unsupported("crash-symbolize:action-not-wired（无 dump 源）")
        }
        MgmtAction::UpdateCheck => ro("bootctrl 状态投影（seq/floor/槽位；MGT2 root 中继）"),
        // ===== 只读诊断（operator）=====
        MgmtAction::ConfigGet => ro("signed config entry payload + digest/generation"),
        MgmtAction::SupportBundleExport => ActionCap {
            capability: ActionCapability::Stream,
            note: "paged support bundle snapshot（seq/page 分页读取）",
        },
        // ===== 副作用（root 执行/配置事务）=====
        MgmtAction::AgentRestart => side_effect("managed restart via root (MGT2); allow-list"),
        MgmtAction::MaintenanceEnter => {
            side_effect("maintenance enter via root; stops control writes")
        }
        MgmtAction::MaintenanceExit => side_effect("maintenance exit via root; health gate"),
        MgmtAction::ConfigSet => side_effect("signed config stage/activate transaction"),
        // ===== 其余：显式未接实（危险动作保持拒绝）=====
        MgmtAction::AgentStart => {
            unsupported("agent-start:action-not-wired（root 无 spawn-at-runtime 面）")
        }
        MgmtAction::AgentStop => unsupported("agent-stop:action-not-wired（无停机排空裁决）"),
        MgmtAction::UpdateApply => {
            side_effect("update-apply:no-staged-package（外部包注入依赖 MGMT-DOWNLINK，登记 Open）")
        }
        MgmtAction::UpdateRollback => {
            side_effect("取消未确认 trial（BootControl 双副本+flush；MGT2 root 中继）")
        }
        MgmtAction::BackupCreate => unsupported("backup-create:action-not-wired（无备份介质裁决）"),
        MgmtAction::Restore => unsupported("restore:action-not-wired（恢复介质未冻结）"),
        MgmtAction::Revoke => unsupported("revoke:action-not-wired（身份撤销面归信任链）"),
        MgmtAction::FactoryReset => unsupported("factory-reset:action-not-wired（破坏性未裁决）"),
        MgmtAction::SecureErase => unsupported("secure-erase:action-not-wired（破坏性未裁决）"),
    }
}

/// 操作状态（Accepted 后不终结的路径不存在：Running 必达终态或诚实
/// Unknown——Unknown 附对账路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpStatus {
    /// 已受理执行中（重启后恢复 = 转 Unknown，不冒称仍在跑）。
    Running,
    /// 终态：完成。
    Completed,
    /// 终态：失败（detail 携原因）。
    Failed,
    /// 结果未知（进程重启时在途；query 可继续对账）。
    Unknown,
}

impl OpStatus {
    pub const fn code(self) -> u8 {
        match self {
            OpStatus::Running => 0,
            OpStatus::Completed => 1,
            OpStatus::Failed => 2,
            OpStatus::Unknown => 3,
        }
    }
    pub const fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(OpStatus::Running),
            1 => Some(OpStatus::Completed),
            2 => Some(OpStatus::Failed),
            3 => Some(OpStatus::Unknown),
            _ => None,
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OpStatus::Completed | OpStatus::Failed | OpStatus::Unknown
        )
    }
}

/// 操作记录（账本行；字符串域在 push 处截断封顶——常驻字节有界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    /// 操作号（账本单调分配；跨重启经持久化延续）。
    pub op_id: u64,
    pub request_id: u64,
    /// 发起 principal（≤24B 截断）。
    pub principal: String,
    /// 动作名（≤24B 截断）。
    pub action: String,
    /// 请求摘要（SM3(arg) 前 8B）——幂等判别键第二分量。
    pub arg_digest: [u8; 8],
    pub status: OpStatus,
    /// 受理时的配置代次（副作用与配置同代绑定）。
    pub config_generation: u64,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// 终态/进度说明（≤96B 截断）。
    pub detail: String,
}

/// 账本容量硬上限（条数；每条 String 域截断后常驻 ≤ ~250B，总常驻
/// ≤ 24×250 = 6 KiB 量级——与 MgmtOps 区段 4 KiB 内的编码行预算匹配：
/// 编码后每行定长 120B，24 行 + 头 ≤ 3 KiB）。
pub const MGMT_OPS_MAX: usize = 24;

/// String 域截断上限。
const PRINCIPAL_CAP: usize = 24;
const ACTION_CAP_MAX: usize = 24;
const DETAIL_CAP: usize = 96;

fn cap_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        String::from(s)
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        String::from(&s[..end])
    }
}

/// 幂等门结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpGate {
    /// 新操作受理（执行器必须随后 finish）。
    New(u64),
    /// 同 (principal, request_id, arg_digest) 已有记录——幂等回放，
    /// **不**重复执行；调用方返回已记录状态。
    Replayed(u64),
    /// 同 request_id 不同摘要 = 幂等冲突（拒绝）。
    Conflict,
    /// 账本满（副作用背压拒绝；只读不受限）。
    Full,
}

/// 操作账本（单一幂等事实源；有界丢最旧并计数——但**在途 Running
/// 不逐出**：丢最旧只在无在途记录时进行，保证 Accepted 后有回查）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationLedger {
    records: Vec<OperationRecord>,
    next_op_id: u64,
    dropped: u64,
    /// 装配侧持久化信号（begin/finish/恢复置位；take_dirty 取走清零）。
    dirty: bool,
}

/// 账本持久化状态（RegionStateStore 载荷；编解码见文底 impl）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgmtOpsState {
    pub ledger: OperationLedger,
}

impl OperationLedger {
    pub fn new() -> Self {
        OperationLedger {
            records: Vec::new(),
            next_op_id: 1,
            dropped: 0,
            dirty: false,
        }
    }

    /// 装配侧持久化信号（自 begin/finish/恢复步以来有变更）。
    pub fn take_dirty(&mut self) -> bool {
        core::mem::replace(&mut self.dirty, false)
    }

    pub fn records(&self) -> &[OperationRecord] {
        &self.records
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    /// 幂等门（副作用操作前置判定）。`arg_digest` = SM3(arg) 前 8B。
    pub fn begin(
        &mut self,
        request_id: u64,
        principal: &str,
        action: &str,
        arg_digest: [u8; 8],
        config_generation: u64,
        now_ms: u64,
    ) -> OpGate {
        // 幂等判别：同 principal + request_id。
        let existing = self
            .records
            .iter()
            .find(|r| r.request_id == request_id && r.principal == principal);
        if let Some(r) = existing {
            if r.arg_digest == arg_digest {
                return OpGate::Replayed(r.op_id);
            }
            return OpGate::Conflict;
        }
        if self.records.len() >= MGMT_OPS_MAX {
            // 丢最旧——但存在在途 Running 时拒绝背压（Accepted 后必须
            // 可回查：在途占位不逐出）。
            if self.records.iter().any(|r| r.status == OpStatus::Running) {
                return OpGate::Full;
            }
            self.records.remove(0);
            self.dropped += 1;
        }
        let op_id = self.next_op_id;
        self.next_op_id = self.next_op_id.wrapping_add(1).max(1);
        self.dirty = true;
        self.records.push(OperationRecord {
            op_id,
            request_id,
            principal: cap_str(principal, PRINCIPAL_CAP),
            action: cap_str(action, ACTION_CAP_MAX),
            arg_digest,
            status: OpStatus::Running,
            config_generation,
            created_ms: now_ms,
            updated_ms: now_ms,
            detail: String::from("accepted"),
        });
        OpGate::New(op_id)
    }

    /// 终态/进度写回（op_id 不存在 = 忽略——账本截断后的迟到回调）。
    pub fn finish(&mut self, op_id: u64, status: OpStatus, detail: &str, now_ms: u64) {
        if let Some(r) = self.records.iter_mut().find(|r| r.op_id == op_id) {
            r.status = status;
            r.detail = cap_str(detail, DETAIL_CAP);
            r.updated_ms = now_ms;
            self.dirty = true;
        }
    }

    pub fn get(&self, op_id: u64) -> Option<&OperationRecord> {
        self.records.iter().find(|r| r.op_id == op_id)
    }

    /// 恢复步（进程重启装载后调用）：在途 Running → Unknown（诚实——
    /// 执行体已随进程消失；detail 附操作号供对账继续）。
    pub fn mark_unknown_inflight(&mut self, now_ms: u64) {
        let ids: Vec<u64> = self
            .records
            .iter()
            .filter(|r| r.status == OpStatus::Running)
            .map(|r| r.op_id)
            .collect();
        for id in ids {
            self.finish(
                id,
                OpStatus::Unknown,
                "unknown:process-restart;requery-op",
                now_ms,
            );
        }
        self.dirty = false; // 恢复步本身不触发落盘
    }
}

impl Default for OperationLedger {
    fn default() -> Self {
        Self::new()
    }
}

/// 请求摘要（SM3(arg) 前 8B；跨进程稳定——同一请求字节 ⇒ 同摘要）。
pub fn arg_digest(arg: &[u8]) -> [u8; 8] {
    let h = eneros_crypto::sm3_hash(arg);
    let mut out = [0u8; 8];
    out.copy_from_slice(&h[..8]);
    out
}

// ============================================================================
// 持久化编码（durable-state Encode/Decode；定长行布局——记录数/字节双有界）
// ============================================================================

/// 编码行定长：op_id(8) request_id(8) principal(24) action(24) digest(8)
/// status(1) generation(8) created(8) updated(8) detail_len(1) detail(≤31)
/// = 8+8+24+24+8+1+8+8+8+1+31 = 129B。行定长简化恢复端截断处理。
const ENC_ROW: usize = 129;
const ENC_DETAIL_MAX: usize = 31;

fn put_str(out: &mut Vec<u8>, s: &str, fixed: usize) {
    let b = s.as_bytes();
    let n = core::cmp::min(b.len(), fixed);
    out.extend_from_slice(&b[..n]);
    out.extend(core::iter::repeat(0u8).take(fixed - n));
}

fn get_str(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl eneros_durable_state::Encode for MgmtOpsState {
    fn encode(&self) -> Result<Vec<u8>, eneros_durable_state::StoreError> {
        // 头：next_op_id(8) ‖ dropped(8) ‖ count(1)。行定长。
        let mut out = Vec::with_capacity(17 + self.ledger.records.len() * ENC_ROW);
        if self.ledger.records.len() > MGMT_OPS_MAX {
            return Err(eneros_durable_state::StoreError::Corrupt);
        }
        out.extend_from_slice(&self.ledger.next_op_id.to_le_bytes());
        out.extend_from_slice(&self.ledger.dropped.to_le_bytes());
        out.push(self.ledger.records.len() as u8);
        for r in &self.ledger.records {
            let start = out.len();
            out.extend_from_slice(&r.op_id.to_le_bytes());
            out.extend_from_slice(&r.request_id.to_le_bytes());
            put_str(&mut out, &r.principal, PRINCIPAL_CAP);
            put_str(&mut out, &r.action, ACTION_CAP_MAX);
            out.extend_from_slice(&r.arg_digest);
            out.push(r.status.code());
            out.extend_from_slice(&r.config_generation.to_le_bytes());
            out.extend_from_slice(&r.created_ms.to_le_bytes());
            out.extend_from_slice(&r.updated_ms.to_le_bytes());
            let detail = cap_str(&r.detail, ENC_DETAIL_MAX);
            out.push(detail.len() as u8);
            put_str(&mut out, &detail, ENC_DETAIL_MAX);
            debug_assert_eq!(out.len() - start, ENC_ROW, "行定长被破坏");
        }
        Ok(out)
    }
}

impl eneros_durable_state::Decode for MgmtOpsState {
    fn decode(
        bytes: &[u8],
    ) -> Result<(eneros_durable_state::Generation, Self), eneros_durable_state::StoreError> {
        use eneros_durable_state::{Generation, StoreError};
        // **RegionStateStore payload 布局：generation(8) ‖ state**——前缀由
        // 本 impl 消费（与 ledger.rs::LedgerSnapshot / ota_service.rs::
        // TrialState / agent_bus.rs::SessionCursor / learn.rs::LearnSnapshot
        // 同源契约）。70.6 首版漏读该前缀：`Generation(1)` 硬编码返回 + 状态
        // 解析整体右移 8B（count 从 state[16] 读成 state[8] 低位）→ 恢复
        // 恒 n=0 且代次恒 1（REG-70.6-OPS-RESTORE-N0 真根因，实弹
        // `MGMT-OPS ledger restored n=0`，宿主/旁证双复现）。
        //
        // 单元测试（`persistence_roundtrip_preserves_ledger`）只喂 `encode()`
        // 原始字节（无前缀）——生产路径恒有前缀，故该测试对本次缺陷全盲；
        // 回归锁见 `decode_consumes_store_generation_prefix`（前缀形态）。
        if bytes.len() < 8 + 17 {
            return Err(StoreError::Corrupt);
        }
        let generation = Generation(u64::from_le_bytes(bytes[0..8].try_into().expect("u64")));
        let state = &bytes[8..];
        let next_op_id = u64::from_le_bytes(state[0..8].try_into().expect("u64"));
        let dropped = u64::from_le_bytes(state[8..16].try_into().expect("u64"));
        let count = state[16] as usize;
        if count > MGMT_OPS_MAX || state.len() < 17 + count * ENC_ROW {
            return Err(StoreError::Corrupt);
        }
        let mut records = Vec::with_capacity(count);
        let mut pos = 17;
        for _ in 0..count {
            let row = &state[pos..pos + ENC_ROW];
            pos += ENC_ROW;
            let status = OpStatus::from_code(row[72]).ok_or(StoreError::Corrupt)?;
            let detail_len = row[97] as usize;
            if detail_len > ENC_DETAIL_MAX {
                return Err(StoreError::Corrupt);
            }
            records.push(OperationRecord {
                op_id: u64::from_le_bytes(row[0..8].try_into().expect("u64")),
                request_id: u64::from_le_bytes(row[8..16].try_into().expect("u64")),
                principal: get_str(&row[16..40]),
                action: get_str(&row[40..64]),
                arg_digest: row[64..72].try_into().expect("8B"),
                status,
                config_generation: u64::from_le_bytes(row[73..81].try_into().expect("u64")),
                created_ms: u64::from_le_bytes(row[81..89].try_into().expect("u64")),
                updated_ms: u64::from_le_bytes(row[89..97].try_into().expect("u64")),
                detail: get_str(&row[98..98 + detail_len]),
            });
        }
        Ok((
            generation,
            MgmtOpsState {
                ledger: OperationLedger {
                    records,
                    next_op_id: next_op_id.max(1),
                    dropped,
                    dirty: false,
                },
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEN: u64 = 3;

    #[test]
    fn capability_table_covers_all_22_actions() {
        use crate::mgmt_rpc::MgmtAction::*;
        let all = [
            StatusQuery,
            SloQuery,
            AgentList,
            AgentStart,
            AgentStop,
            AgentRestart,
            ConfigGet,
            ConfigSet,
            LogQuery,
            SupportBundleExport,
            CrashDumpSymbolize,
            UpdateCheck,
            UpdateApply,
            UpdateRollback,
            MaintenanceEnter,
            MaintenanceExit,
            BackupCreate,
            Restore,
            Revoke,
            FactoryReset,
            SecureErase,
            HistoryQuery,
        ];
        assert_eq!(all.len(), 22);
        let (mut n_unsup, mut n_ro, mut n_side, mut n_stream) = (0, 0, 0, 0);
        for a in all {
            let cap = action_capability(&a);
            assert!(!cap.note.is_empty(), "{a:?} 能力行缺说明");
            match cap.capability {
                ActionCapability::Unsupported => n_unsup += 1,
                ActionCapability::ReadOnly => n_ro += 1,
                ActionCapability::SideEffect => n_side += 1,
                ActionCapability::Stream => n_stream += 1,
            }
        }
        // 70.7.5 面貌钉扎：5 只读 + 1 流式 + 6 副作用 + 10 显式未接实
        //（update-check=只读投影 / update-rollback=副作用 / update-apply=
        // 副作用面-诚实无包拒绝；70.6 面貌为 4/1/4/13）。
        assert_eq!((n_ro, n_stream, n_side, n_unsup), (5, 1, 6, 10));
        // 破坏性动作语义保持：UpdateRollback 已接实但仍属强认证破坏面
        //（is_destructive），其余危险动作显式 Unsupported（不空成功）。
        for a in [FactoryReset, SecureErase, Revoke, Restore] {
            assert_eq!(
                action_capability(&a).capability,
                ActionCapability::Unsupported,
                "{a:?} 必须显式未接实"
            );
        }
    }

    #[test]
    fn idempotent_replay_returns_same_op_without_double_begin() {
        let mut led = OperationLedger::new();
        let d = arg_digest(b"svc-ota");
        let g1 = led.begin(7, "ops-1", "agent-restart", d, GEN, 100);
        assert!(matches!(g1, OpGate::New(1)));
        led.finish(1, OpStatus::Completed, "restart-complete:gen=2", 200);
        // 同 request_id 同摘要 → Replayed（同 op 号），不再产生新记录。
        let g2 = led.begin(7, "ops-1", "agent-restart", d, GEN, 300);
        assert_eq!(g2, OpGate::Replayed(1));
        assert_eq!(led.records().len(), 1, "幂等回放不得产生第二条");
        // 不同 principal 同 request_id = 不同操作（scope 按 principal）。
        let g3 = led.begin(7, "ops-2", "agent-restart", d, GEN, 400);
        assert!(matches!(g3, OpGate::New(2)));
    }

    #[test]
    fn same_request_id_different_arg_conflicts() {
        let mut led = OperationLedger::new();
        assert!(matches!(
            led.begin(9, "ops-1", "config-set", arg_digest(b"aaa"), GEN, 100),
            OpGate::New(_)
        ));
        assert_eq!(
            led.begin(9, "ops-1", "config-set", arg_digest(b"bbb"), GEN, 200),
            OpGate::Conflict,
            "同 request_id 不同参数必须拒绝"
        );
    }

    #[test]
    fn bounded_full_backpressure_running_not_evicted() {
        let mut led = OperationLedger::new();
        // 填满（终态记录可逐出）。
        for i in 0..MGMT_OPS_MAX {
            assert!(matches!(
                led.begin(
                    100 + i as u64,
                    "ops",
                    "maintenance-enter",
                    arg_digest(&[i as u8]),
                    GEN,
                    i as u64
                ),
                OpGate::New(_)
            ));
            led.finish(i as u64 + 1, OpStatus::Completed, "ok", i as u64);
        }
        assert_eq!(led.records().len(), MGMT_OPS_MAX);
        // 满且全终态：新操作逐出最旧。
        assert!(matches!(
            led.begin(999, "ops", "config-set", arg_digest(b"x"), GEN, 1),
            OpGate::New(_)
        ));
        assert_eq!(led.dropped_count(), 1);
        // 满且有在途：背压拒绝（Accepted 后必须可回查）。
        let mut led2 = OperationLedger::new();
        for i in 0..MGMT_OPS_MAX {
            assert!(matches!(
                led2.begin(
                    i as u64,
                    "ops",
                    "agent-restart",
                    arg_digest(&[i as u8]),
                    GEN,
                    0
                ),
                OpGate::New(_)
            ));
        }
        assert_eq!(
            led2.begin(9999, "ops", "agent-restart", arg_digest(b"y"), GEN, 0),
            OpGate::Full,
            "在途占位不逐出——副作用背压"
        );
    }

    #[test]
    fn restore_marks_running_unknown_with_reconciliation_hint() {
        let mut led = OperationLedger::new();
        assert!(matches!(
            led.begin(5, "ops", "agent-restart", arg_digest(b"t"), GEN, 100),
            OpGate::New(_)
        ));
        led.mark_unknown_inflight(999);
        let r = led.get(1).expect("record");
        assert_eq!(r.status, OpStatus::Unknown, "在途恢复 = 诚实 Unknown");
        assert!(r.detail.contains("unknown"), "{}", r.detail);
        assert!(r.detail.contains("requery"), "必须附对账路径");
        // 终态记录不受恢复步影响。
        let mut led2 = OperationLedger::new();
        assert!(matches!(
            led2.begin(6, "ops", "config-set", arg_digest(b"t"), GEN, 100),
            OpGate::New(_)
        ));
        led2.finish(1, OpStatus::Failed, "stage-reject", 150);
        led2.mark_unknown_inflight(999);
        assert_eq!(led2.get(1).unwrap().status, OpStatus::Failed);
    }

    #[test]
    fn persistence_roundtrip_preserves_ledger() {
        use eneros_durable_state::{Decode, Encode};
        let mut state = MgmtOpsState {
            ledger: OperationLedger::new(),
        };
        assert!(matches!(
            state.ledger.begin(
                1,
                "station-ops",
                "agent-restart",
                arg_digest(b"svc-ota"),
                7,
                100
            ),
            OpGate::New(_)
        ));
        state
            .ledger
            .finish(1, OpStatus::Completed, "restart-complete:gen=2", 200);
        assert!(matches!(
            state.ledger.begin(
                2,
                "station-ops",
                "config-set",
                arg_digest(b"bundle"),
                7,
                300
            ),
            OpGate::New(_)
        ));
        // 在途保持 Running。
        let bytes = state.encode().unwrap();
        // 生产路径形态 = store payload（generation(8) ‖ state）——直接喂
        // `encode()` 原样字节会绕开前缀契约（70.6 缺陷正是这样被漏过的）。
        let mut store_payload = 4u64.to_le_bytes().to_vec();
        store_payload.extend_from_slice(&bytes);
        let (gen, restored) = MgmtOpsState::decode(&store_payload).unwrap();
        assert_eq!(gen.0, 4, "代次 = store 前缀（不得硬编码）");
        assert_eq!(restored.ledger.records().len(), 2);
        assert_eq!(restored.ledger.records()[0].principal, "station-ops");
        assert_eq!(restored.ledger.records()[0].status, OpStatus::Completed);
        assert_eq!(
            restored.ledger.records()[0].detail,
            "restart-complete:gen=2"
        );
        assert_eq!(restored.ledger.records()[1].status, OpStatus::Running);
        assert_eq!(
            restored.ledger.records()[1].arg_digest,
            arg_digest(b"bundle")
        );
        // 恢复步在恢复态上执行。
        let mut s2 = restored;
        s2.ledger.mark_unknown_inflight(500);
        assert_eq!(s2.ledger.records()[1].status, OpStatus::Unknown);
        // op_id 延续：恢复后新操作号不回退。
        let g = s2
            .ledger
            .begin(3, "x", "maintenance-enter", arg_digest(b"m"), 7, 600);
        assert!(matches!(g, OpGate::New(3)), "op_id 必须跨持久化延续");
    }

    /// REG-70.6-OPS-RESTORE-N0 回归锁（70.7 复查发现）：**store payload 前缀**
    /// = generation(8) ‖ encode()。缺陷形态 = 解码从 state[0] 读 next_op_id
    /// （整体右移 8B）且返回硬编码 `Generation(1)` → 恢复恒 n=0、代次恒 1
    ///（跨重启幂等账本凭空清空，已确认操作可被重复执行）。
    ///
    /// 断言两侧：① 前缀形态解码出真实记录与真实代次；② **无前缀**形态必须
    /// 被拒（若 `encode()` 将来自己带上 generation，本锁即红——迫使
    /// RegionStateStore 与 codec 的前缀归属重新对齐，而不是两边各写一份）。
    #[test]
    fn decode_consumes_store_generation_prefix() {
        use eneros_durable_state::{Decode, Encode};
        let mut state = MgmtOpsState {
            ledger: OperationLedger::new(),
        };
        for i in 1..=3u64 {
            let gate = state.ledger.begin(
                i,
                "station-ops",
                "config-set",
                arg_digest(&i.to_le_bytes()),
                5,
                1000 + i,
            );
            // 断言先于取值：gate 必须为 New（失败信息含实得形态），
            // 取值臂留非 panic 兜底（仓内禁 panic 宏）。
            assert!(
                matches!(gate, OpGate::New(_)),
                "新请求必须受理，实得 {gate:?}"
            );
            let id = match gate {
                OpGate::New(id) => id,
                _ => 0,
            };
            state.ledger.finish(
                id,
                OpStatus::Completed,
                "config-stage-part accepted",
                1000 + i,
            );
        }
        let body = state.encode().expect("encode");
        let mut payload = 9u64.to_le_bytes().to_vec();
        payload.extend_from_slice(&body);
        let (gen, restored) = MgmtOpsState::decode(&payload).expect("生产形态必须解码");
        assert_eq!(gen.0, 9, "代次取自 store 前缀");
        assert_eq!(restored.ledger.records().len(), 3, "记录数不得归零");
        assert_eq!(restored.ledger.records()[2].op_id, 3);
        assert_eq!(restored.ledger.records()[0].principal, "station-ops");
        // 无前缀形态（= `encode()` 原样字节）不得被当作有效状态解出记录：
        // 前缀被吃掉 8B → 长度下界不足，或 count 落在错误偏移 → 拒绝
        //（fail-closed，不静默恢复「空账本却是有效记录」）。70.6 缺陷形态
        // 正是后者：返回 Ok 且 n=0。
        match MgmtOpsState::decode(&body) {
            Err(_) => {}
            Ok((_, bogus)) => assert_eq!(
                bogus.ledger.records().len(),
                0,
                "无前缀形态一旦解出非零记录，前缀归属已被破坏"
            ),
        }
        assert!(MgmtOpsState::decode(&[]).is_err(), "空 payload 必须拒绝");
    }

    #[test]
    fn long_fields_truncated_to_caps() {
        let mut led = OperationLedger::new();
        let long_principal = "principal-way-too-long-for-the-field-abcdef";
        assert!(matches!(
            led.begin(1, long_principal, "agent-restart", arg_digest(b"t"), GEN, 0),
            OpGate::New(_)
        ));
        led.finish(1, OpStatus::Failed, &"x".repeat(500), 10);
        let r = led.get(1).unwrap();
        assert_eq!(r.principal.len(), PRINCIPAL_CAP);
        assert_eq!(r.detail.len(), DETAIL_CAP);
        // 编码往返在截断后保持稳定（store payload 形态：generation(8) ‖ encode）。
        use eneros_durable_state::{Decode, Encode};
        let bytes = MgmtOpsState {
            ledger: led.clone(),
        }
        .encode()
        .unwrap();
        let mut payload = GEN.to_le_bytes().to_vec();
        payload.extend_from_slice(&bytes);
        let (_, back) = MgmtOpsState::decode(&payload).unwrap();
        assert_eq!(back.ledger.records()[0].detail.len(), ENC_DETAIL_MAX);
    }

    #[test]
    fn op_status_code_roundtrip() {
        for c in 0u8..=3 {
            let s = OpStatus::from_code(c).expect("code");
            assert_eq!(s.code(), c);
        }
        assert!(OpStatus::from_code(4).is_none());
        assert!(!OpStatus::Running.is_terminal());
        assert!(OpStatus::Unknown.is_terminal());
    }
}
