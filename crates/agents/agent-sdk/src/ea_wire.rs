//! EA（energy-agent 编排响应状态码）wire 单源（蓝图 64 卷 64.2
//! WP-64.2.1b）。
//!
//! 单一事实：`svc-energy-agent` TICK 响应首字节的状态码族（值 + 显示词形）。
//! 此前定义在 control-services `energy_agent_service.rs`，而 root-task
//! 对同一码表手写 4 臂 match 镜像（63.1 DispatchState 同型病灶）——常量与
//! 显示词形收进本模块（eneros-dispatch-state 叶内化先例疗法；root 与
//! control-services 均已依赖 agent-sdk，零新增依赖边）。
//!
//! 码值冻结（wire 字节，不得变更）：OK=0 / NOACTION=1 / UNAVAILABLE=2 /
//! INVALID=0xFF（未知码兜底同 INVALID——显示面词形与 45-63 卷证据行一致）。

/// 编排正常（receipt wire 随附）。
pub const EA_OK: u8 = 0;
/// 本轮无动作（预算/熔断/无意图）。
pub const EA_NOACTION: u8 = 1;
/// 下游不可用（worker/dispatch 降级——显式，不伪造成功）。
pub const EA_UNAVAILABLE: u8 = 2;
/// 坏请求/未知码（fail-closed 兜底）。
pub const EA_INVALID: u8 = 0xFF;

/// EA 状态（解码形态——未知码归 INVALID，与显示面兜底臂同一语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EaStatus {
    Ok,
    NoAction,
    Unavailable,
    Invalid,
}

/// 码 → 状态（未知码/空族外值归 [`EaStatus::Invalid`]——原 root 手写
/// match 兜底臂语义）。
pub const fn from_code(code: u8) -> EaStatus {
    match code {
        EA_OK => EaStatus::Ok,
        EA_NOACTION => EaStatus::NoAction,
        EA_UNAVAILABLE => EaStatus::Unavailable,
        _ => EaStatus::Invalid,
    }
}

/// 冻结显示词形（CONTROL-CHAIN 串口行 `agent=` 字段逐字节真值——
/// 与原 root 手写 4 臂逐字一致：`"OK"/"NOACTION"/"UNAVAILABLE"/"INVALID"`）。
pub const fn ea_display(code: u8) -> &'static str {
    match from_code(code) {
        EaStatus::Ok => "OK",
        EaStatus::NoAction => "NOACTION",
        EaStatus::Unavailable => "UNAVAILABLE",
        EaStatus::Invalid => "INVALID",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 词形锁：四词形逐字节（改一词形必须变红——R-64.g）。
    #[test]
    fn display_wordforms_are_byte_frozen() {
        assert_eq!(ea_display(EA_OK), "OK");
        assert_eq!(ea_display(EA_NOACTION), "NOACTION");
        assert_eq!(ea_display(EA_UNAVAILABLE), "UNAVAILABLE");
        assert_eq!(ea_display(EA_INVALID), "INVALID");
    }

    /// 往返锁：码值冻结（wire 字节零变化）+ 未知码兜底臂。
    #[test]
    fn code_values_frozen_and_unknown_falls_to_invalid() {
        assert_eq!(EA_OK, 0);
        assert_eq!(EA_NOACTION, 1);
        assert_eq!(EA_UNAVAILABLE, 2);
        assert_eq!(EA_INVALID, 0xFF);
        assert_eq!(from_code(0), EaStatus::Ok);
        assert_eq!(from_code(1), EaStatus::NoAction);
        assert_eq!(from_code(2), EaStatus::Unavailable);
        assert_eq!(from_code(0xFF), EaStatus::Invalid);
        // 未知码（3..=254）→ INVALID（原手写 `_` 臂语义）。
        assert_eq!(from_code(3), EaStatus::Invalid);
        assert_eq!(ea_display(3), "INVALID");
        assert_eq!(from_code(200), EaStatus::Invalid);
    }
}
