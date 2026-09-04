//! 蓝图 63 WP-63.1.1（v0.316.0）：`DispatchState` 12 态与 u8 码表**单一实现**。
//!
//! 历史：码表在控制面三处各自手写逐臂重复（dispatch_service.rs 回执
//! encode/decode、durable_intent.rs 持久化解码、root service.rs 内联 4 态
//! 显示 + tick_resp[2] 偏移知识），当前一致、零守卫。蓝图 63 把码表收进
//! 本 crate 唯一实现；所有 wire/持久化/显示消费面只经
//! [`DispatchState::to_code`] / [`from_code`] / [`display_name`]。
//!
//! 本 crate 刻意**零依赖**——root-task（seL4 orchestrator）链接它做状态码
//! 解码/显示收窄，但不许被拖入 control-dispatch 的依赖闭包（dual-brain /
//! decision-contract / serde_json 等）。
//!
//! 冻结：码值 0–11 布局与词形（"Committed" 等）是冻结资产（回执 wire /
//! 持久化布局 / CONTROL-CHAIN 串口词形依赖），不得改动。

#![cfg_attr(not(test), no_std)]

/// 投递状态机（WP-40.6 12 态；`Committed` 是唯一成功终态）。
///
/// 判别序 = 码表序（Proposed..ReadbackVerified 为推进链，Rejected/Failed/
/// Indeterminate 为终局臂）。`PartialOrd` 语义沿用历史（dispatch.rs 原
/// `#[derive(... PartialOrd, Ord)]`）——推进链可比较，与码值正交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DispatchState {
    /// 已提交给 dispatch（proposal 校验前）。
    Proposed,
    /// proposal schema/units/ranges/idempotency 校验通过。
    Validated,
    /// actor/capability/lease/fencing + safety catalog 授权通过。
    Authorized,
    /// rate/energy/concurrency 预算 reserve 通过。
    Reserved,
    /// durable intent 已写入。
    Persisted,
    /// ControlEnvelope 已投递（device-agent 收到）。
    Delivered,
    /// 协议/控制器 transport ACK。
    TransportAcked,
    /// 独立 readback 符合容差/时限。
    ReadbackVerified,
    /// 审计、预算、反馈完成（唯一成功终态）。
    Committed,
    /// 被拒（授权/校验失败，零投递）。
    Rejected,
    /// 失败（写失败/读回不符/设备拒绝）。
    Failed,
    /// 不确定（投递后超时/重启/读回不可得——不猜成功，执行对账）。
    Indeterminate,
}

impl DispatchState {
    /// 12 臂码表唯一 encode 实现（码值 0–11 冻结：回执 wire 字节 /
    /// durable 持久化字节 / CONTROL-CHAIN 词形依赖）。
    pub const fn to_code(self) -> u8 {
        match self {
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

    /// 12 臂码表唯一 decode 实现（未知码 → None）。
    pub const fn from_code(c: u8) -> Option<Self> {
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

    /// 冻结显示词形（"Committed" 等——CONTROL-CHAIN 串口行 / settle
    /// audit 之外人类可读面的逐字节真值）。与 [`as_str`] 的小写审计标签
    /// 正交：`as_str` 走控制面内部审计，本函数供显示/解码面。
    pub const fn display_name(self) -> &'static str {
        match self {
            DispatchState::Proposed => "Proposed",
            DispatchState::Validated => "Validated",
            DispatchState::Authorized => "Authorized",
            DispatchState::Reserved => "Reserved",
            DispatchState::Persisted => "Persisted",
            DispatchState::Delivered => "Delivered",
            DispatchState::TransportAcked => "TransportAcked",
            DispatchState::ReadbackVerified => "ReadbackVerified",
            DispatchState::Committed => "Committed",
            DispatchState::Rejected => "Rejected",
            DispatchState::Failed => "Failed",
            DispatchState::Indeterminate => "Indeterminate",
        }
    }

    /// 审计标签（小写——控制面内部审计/摘要 wordform；与 [`display_name`]
    /// 大写显示词形正交）。历史定义在 control-dispatch dispatch.rs，蓝图
    /// 63 随类型归位单源。
    pub fn as_str(&self) -> &'static str {
        match self {
            DispatchState::Proposed => "proposed",
            DispatchState::Validated => "validated",
            DispatchState::Authorized => "authorized",
            DispatchState::Reserved => "reserved",
            DispatchState::Persisted => "persisted",
            DispatchState::Delivered => "delivered",
            DispatchState::TransportAcked => "transport-acked",
            DispatchState::ReadbackVerified => "readback-verified",
            DispatchState::Committed => "committed",
            DispatchState::Rejected => "rejected",
            DispatchState::Failed => "failed",
            DispatchState::Indeterminate => "indeterminate",
        }
    }

    /// 是否成功终态（唯一学习/成功判据）。
    pub const fn is_committed(&self) -> bool {
        matches!(self, DispatchState::Committed)
    }

    /// 是否终局态（推进链之外的 4 终局臂；root 显示过滤用）。
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            DispatchState::Committed
                | DispatchState::Rejected
                | DispatchState::Failed
                | DispatchState::Indeterminate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::DispatchState;
    use crate::DispatchState as D;

    /// 12 变体往返一致（码表单源的完整覆盖：每个变体 to_code 后
    /// from_code 回原变体）。
    #[test]
    fn to_from_roundtrip_all_variants() {
        let all = [
            D::Proposed,
            D::Validated,
            D::Authorized,
            D::Reserved,
            D::Persisted,
            D::Delivered,
            D::TransportAcked,
            D::ReadbackVerified,
            D::Committed,
            D::Rejected,
            D::Failed,
            D::Indeterminate,
        ];
        for v in all {
            assert_eq!(
                DispatchState::from_code(v.to_code()),
                Some(v),
                "码表往返不一致：{v:?}"
            );
        }
    }

    /// 未知码 → None（decode fail-closed：持久化 Corrupt / wire 未知
    /// 字节不得解码成伪状态）。
    #[test]
    fn unknown_code_is_none() {
        assert_eq!(DispatchState::from_code(12), None);
        assert_eq!(DispatchState::from_code(255), None);
    }

    /// 冻结码值（0–11 布局逐字节锁——回执 wire / durable 字节依赖）。
    #[test]
    fn frozen_code_values_locked() {
        let pairs = [
            (D::Proposed, 0u8),
            (D::Validated, 1),
            (D::Authorized, 2),
            (D::Reserved, 3),
            (D::Persisted, 4),
            (D::Delivered, 5),
            (D::TransportAcked, 6),
            (D::ReadbackVerified, 7),
            (D::Committed, 8),
            (D::Rejected, 9),
            (D::Failed, 10),
            (D::Indeterminate, 11),
        ];
        for (v, code) in pairs {
            assert_eq!(v.to_code(), code, "{v:?} 码值漂移（冻结布局）");
        }
    }

    /// 冻结显示词形逐字节一致（CONTROL-CHAIN 串口行依赖）。
    #[test]
    fn display_names_match_frozen_wordforms() {
        let pairs = [
            (D::Proposed, "Proposed"),
            (D::Validated, "Validated"),
            (D::Authorized, "Authorized"),
            (D::Reserved, "Reserved"),
            (D::Persisted, "Persisted"),
            (D::Delivered, "Delivered"),
            (D::TransportAcked, "TransportAcked"),
            (D::ReadbackVerified, "ReadbackVerified"),
            (D::Committed, "Committed"),
            (D::Rejected, "Rejected"),
            (D::Failed, "Failed"),
            (D::Indeterminate, "Indeterminate"),
        ];
        for (v, name) in pairs {
            assert_eq!(v.display_name(), name, "{v:?} 词形漂移");
        }
    }

    /// 冻结审计标签（小写）逐字节一致（内部审计/摘要 digest wordform）。
    #[test]
    fn as_str_labels_match_frozen_wordforms() {
        let pairs = [
            (D::Proposed, "proposed"),
            (D::Validated, "validated"),
            (D::Authorized, "authorized"),
            (D::Reserved, "reserved"),
            (D::Persisted, "persisted"),
            (D::Delivered, "delivered"),
            (D::TransportAcked, "transport-acked"),
            (D::ReadbackVerified, "readback-verified"),
            (D::Committed, "committed"),
            (D::Rejected, "rejected"),
            (D::Failed, "failed"),
            (D::Indeterminate, "indeterminate"),
        ];
        for (v, name) in pairs {
            assert_eq!(v.as_str(), name, "{v:?} 审计标签漂移");
        }
    }

    #[test]
    fn committed_and_terminal_semantics() {
        assert!(D::Committed.is_committed());
        assert!(!D::Indeterminate.is_committed());
        assert!(!D::Failed.is_committed());
        for t in [D::Committed, D::Rejected, D::Failed, D::Indeterminate] {
            assert!(t.is_terminal(), "{t:?} 应为终局态");
        }
        for m in [
            D::Proposed,
            D::Validated,
            D::Authorized,
            D::Reserved,
            D::Persisted,
            D::Delivered,
            D::TransportAcked,
            D::ReadbackVerified,
        ] {
            assert!(!m.is_terminal(), "{m:?} 应为推进态");
        }
    }
}

/// root tick 应答终态投影（蓝图 66 WP-66.5.1 单源；历史 root service.rs
/// :1491-1497 手读 `tick_resp[2]` + 内联 from_code 收编）。
///
/// 布局契约（EA_OK 路径）：`[EA_OK(1)][receipt status(1)][terminal state(1)]
/// …`——终态字节在 `resp[2]`（offset 由 svc-energy-agent 装配面冻结）。
/// `ea_ok` 参数 = 呼叫方语义源（agent-sdk `ea_wire::EA_OK`）——本 crate
/// 刻意零依赖，不复制 EA 状态码语义；root 侧调用点传
/// `eneros_agent_sdk::ea_wire::EA_OK`。
///
/// 等价性：与 root 原手读逐分支等价——非 OK 首字节 → None；短帧（<3B）
/// → None；OK 帧尾字节走 [`DispatchState::from_code`]（未知码 → None，
/// fail-closed 不猜）。
pub fn ea_tick_terminal_state(resp: &[u8], ea_ok: u8) -> Option<DispatchState> {
    if resp.first() == Some(&ea_ok) && resp.len() >= 3 {
        DispatchState::from_code(resp[2])
    } else {
        None
    }
}

#[cfg(test)]
mod ea_tick_tests {
    use super::{ea_tick_terminal_state, DispatchState};

    const EA_OK: u8 = 0; // 测试镜像调用方语义源（agent-sdk ea_wire::EA_OK）
    const EA_NOACTION: u8 = 1;

    #[test]
    fn ok_frame_projects_terminal_state() {
        assert_eq!(
            ea_tick_terminal_state(
                &[EA_OK, 0, DispatchState::Committed.to_code(), 1, 2, 3],
                EA_OK
            ),
            Some(DispatchState::Committed)
        );
        assert_eq!(
            ea_tick_terminal_state(&[EA_OK, 0, DispatchState::Failed.to_code()], EA_OK),
            Some(DispatchState::Failed)
        );
    }

    #[test]
    fn short_frame_is_none() {
        assert_eq!(ea_tick_terminal_state(&[], EA_OK), None);
        assert_eq!(ea_tick_terminal_state(&[EA_OK], EA_OK), None);
        assert_eq!(ea_tick_terminal_state(&[EA_OK, 0], EA_OK), None);
    }

    #[test]
    fn non_ok_status_is_none_even_with_trailing_bytes() {
        // NOACTION + 任意尾随字节 → None（原手读同分支等价）。
        assert_eq!(ea_tick_terminal_state(&[EA_NOACTION, 7, 7, 7], EA_OK), None);
    }

    #[test]
    fn unknown_terminal_code_is_none() {
        // OK 帧尾字节 0xFF（INVALID）→ from_code 未知码 None。
        assert_eq!(ea_tick_terminal_state(&[EA_OK, 0, 0xFF], EA_OK), None);
    }
}
