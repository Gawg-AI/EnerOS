//! OTA boot 事务 ↔ 管理面的错误语义桥（蓝图 77 卷二 WP-77.2.3）。
//!
//! 病灶（蓝图 §1.1）：svc-ota 把 BlockIo 的 Err 统一写成 `ERR:*io`，
//! BootInstall 拒绝词（`no-pending-trial`/`confirm-secver-mismatch`/
//! `write1`…的不同恢复含义）到不了站端；svc-mgmt rollback **先判 8B
//! seq 再判 `ERR:`**——未来四字符词的 8B `ERR:<word>` 会被误记
//! Completed seq。本模块把两处判定**下沉为纯函数**（host 可测）：
//! bin 只消费判定结果，词形 golden 由本模块测试锁死。
//!
//! wire 合同（§2.5，全部维持）：
//! - 已知合法拒绝词 → `ERR:<word>`（OTA1 边）→ mgmt detail
//!   `update-rollback:<word>` / `update-check:<word>`；
//! - 空载/未知词/其他状态/传输错 → 原 `ERR:*io` / `bad-frame` 族；
//! - 成功 seq 严格 8B、GUARD 严格 40B、status 文本非空且 UTF-8 合法；
//! - `write1`/`flush` 只报告阶段（介质可能已部分更新），不重试不宣称。

use alloc::format;
use alloc::string::String;

use crate::mgmt_rpc::RpcStatus;
use crate::svc_wire::ERR_LEN;

/// svc-ota boot 事务臂（0x0B trial / 0x0C confirm / 0x0D cancel /
/// 0x0E status）的 **Io 词形尾巴**（与收编前逐字节一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootRelayArm {
    Trial,
    Confirm,
    Cancel,
    Status,
}

impl BootRelayArm {
    fn io_suffix(self) -> &'static str {
        match self {
            BootRelayArm::Trial => "trial-io",
            BootRelayArm::Confirm => "confirm-io",
            BootRelayArm::Cancel => "cancel-io",
            BootRelayArm::Status => "status-io",
        }
    }
}

/// boot 事务臂的**失败响应构形**（单源）：已知拒绝 → `ERR:<word>`；
/// 一切 Io → `ERR:<arm>-io`。rejected 词形由 agent-sdk `BootCommandError`
/// 分类保证（仅四个事务子码 + ST_INVALID + 完整已知词）。
pub fn relay_boot_err(
    arm: BootRelayArm,
    io: &eneros_agent_sdk::block_io::BootCommandError,
) -> alloc::vec::Vec<u8> {
    use eneros_agent_sdk::block_io::BootCommandError;
    let mut out = alloc::vec::Vec::new();
    match io {
        BootCommandError::Rejected(reject) => {
            out.extend_from_slice(b"ERR:");
            out.extend_from_slice(reject.as_word().as_bytes());
        }
        BootCommandError::Io(_) => {
            out.extend_from_slice(b"ERR:");
            out.extend_from_slice(arm.io_suffix().as_bytes());
        }
    }
    out
}

/// boot 事务臂的**成功判据**：trial/confirm/cancel 成功段严格 8B seq；
/// status 成功文本非空且 UTF-8 合法（空载/坏字节 = 坏帧，不冒充状态）。
/// 返回 Ok(直通字节) / Err(ERR 响应)。
pub fn relay_boot_ok(
    arm: BootRelayArm,
    data: &[u8],
) -> Result<alloc::vec::Vec<u8>, alloc::vec::Vec<u8>> {
    match arm {
        BootRelayArm::Trial | BootRelayArm::Confirm | BootRelayArm::Cancel => {
            if data.len() == eneros_drv_svc::blk_wire::SEQ_RESP_LEN {
                Ok(alloc::vec::Vec::from(data))
            } else {
                Err(alloc::vec::Vec::from(ERR_LEN))
            }
        }
        BootRelayArm::Status => {
            if data.is_empty() || core::str::from_utf8(data).is_err() {
                Err(alloc::vec::Vec::from(ERR_LEN))
            } else {
                Ok(alloc::vec::Vec::from(data))
            }
        }
    }
}

/// guard/候选读臂（0x0A/0x0F——无 BootInstall 词的子码）失败构形：
/// 一律 Io 词形（`ERR:guard-io`/`ERR:read-io`）；`Rejected` 在这两个
/// 子码不可达（分类面就不给），防御臂仍走 Io 词形不 panic。
pub fn relay_io_err(
    io: &eneros_agent_sdk::block_io::BootCommandError,
    word: &str,
) -> alloc::vec::Vec<u8> {
    let _ = io; // Rejected/Io 同词形——分类面已保证该子码只会来 Io
    let mut out = alloc::vec::Vec::new();
    out.extend_from_slice(b"ERR:");
    out.extend_from_slice(word.as_bytes());
    out
}

/// MGT `update-check`（OTA1 0x0E 经 root 中继）响应 → (状态, detail)。
///
/// 判据（§2.5）：`ERR:` 前缀 → Failed `update-check:<word>`；坏 UTF-8 →
/// Failed `update-check:bad-frame`；**空文本 → Failed
/// `update-check:empty-status`**（旧实现把空文本判成功——服务端 status
/// 总有文本，属异常应答，本卷锁死）；其余 → Completed
/// `update-status:<text>`。
pub fn update_check_outcome(resp: Option<&[u8]>) -> (RpcStatus, String) {
    match resp {
        None => (RpcStatus::Failed, "update-check:root-unbound".into()),
        Some(r) if r.starts_with(b"ERR:") => (
            RpcStatus::Failed,
            format!(
                "update-check:{}",
                core::str::from_utf8(&r[4..]).unwrap_or("?")
            ),
        ),
        Some(r) => match core::str::from_utf8(r) {
            Ok("") => (RpcStatus::Failed, "update-check:empty-status".into()),
            Ok(text) => (RpcStatus::Completed, format!("update-status:{text}")),
            Err(_) => (RpcStatus::Failed, "update-check:bad-frame".into()),
        },
    }
}

/// MGT `update-rollback`（OTA1 0x0D 经 root 中继）响应 → (状态, detail)。
///
/// **判据次序 = `ERR:` 先于 8B seq**（本卷修复：旧实现 len==8 先判，
/// 四字符词的 8B `ERR:<word>` 会被解成 seq 误记 Completed）。无 pending
/// trial → Failed `update-rollback:no-pending-trial`（不是错误吞成
/// `bad-frame`，也不是假成功 seq）。
pub fn update_rollback_outcome(resp: Option<&[u8]>) -> (RpcStatus, String) {
    match resp {
        None => (RpcStatus::Failed, "update-rollback:bad-frame".into()),
        Some(r) if r.starts_with(b"ERR:") => (
            RpcStatus::Failed,
            format!(
                "update-rollback:{}",
                core::str::from_utf8(&r[4..]).unwrap_or("?")
            ),
        ),
        Some(r) if r.len() == 8 => {
            let seq = u64::from_le_bytes(r[..8].try_into().expect("8B seq"));
            (
                RpcStatus::Completed,
                format!("update-rollback:trial-canceled seq={seq}"),
            )
        }
        Some(_) => (RpcStatus::Failed, "update-rollback:bad-frame".into()),
    }
}

/// OTA1 boot 臂失败响应 = ERR 词形的**负例哨兵**（防 ERR_CMD/ERR_PROTO
/// 误入 relay 面——词形空间分立）。
#[cfg(test)]
fn assert_err_wordform_space() {
    use crate::svc_wire::{ERR_CMD, ERR_PROTO};
    assert_ne!(ERR_LEN, ERR_PROTO);
    assert_ne!(ERR_LEN, ERR_CMD);
}

#[cfg(test)]
mod tests {
    use eneros_agent_sdk::block_io::{BootCommandError, BootReject};
    use eneros_durable_state::StoreError;

    use super::*;

    // ==================== WP-77.2.3 三类对外结果 golden ====================

    /// golden①：`no-pending-trial` 穿透 OTA1 → mgmt 全链——
    /// `update-rollback:no-pending-trial`（不是 `ERR:*io`、不是
    /// `bad-frame`、更不是假成功 seq）。
    #[test]
    fn no_pending_trial_reaches_mgmt_detail() {
        let ota = relay_boot_err(
            BootRelayArm::Cancel,
            &BootCommandError::Rejected(BootReject::NoPendingTrial),
        );
        assert_eq!(ota, alloc::vec::Vec::from(&b"ERR:no-pending-trial"[..]));
        let (status, detail) = update_rollback_outcome(Some(&ota));
        assert_eq!(status, RpcStatus::Failed);
        assert_eq!(detail, "update-rollback:no-pending-trial");
    }

    /// golden②：`confirm-secver-mismatch` 穿透到 OTA1 边（虚增水位确认
    /// 的恢复语义不再是不可分 `confirm-io`）。
    #[test]
    fn confirm_secver_mismatch_reaches_ota1_boundary() {
        let ota = relay_boot_err(
            BootRelayArm::Confirm,
            &BootCommandError::Rejected(BootReject::ConfirmSecverMismatch),
        );
        assert_eq!(
            ota,
            alloc::vec::Vec::from(&b"ERR:confirm-secver-mismatch"[..])
        );
    }

    /// golden③：`write1` 阶段词穿透到 OTA1 边——只报告阶段（介质可能
    /// 已部分更新），**不得**被解释成"未写入"或触发自动重试。
    #[test]
    fn write1_stage_word_reaches_ota1_boundary() {
        let ota = relay_boot_err(
            BootRelayArm::Trial,
            &BootCommandError::Rejected(BootReject::Write1),
        );
        assert_eq!(ota, alloc::vec::Vec::from(&b"ERR:write1"[..]));
    }

    /// Io 词形维持原样（成功码与 `ERR:*io` 原有词保持——§2.5）。
    #[test]
    fn io_errors_keep_legacy_wordforms() {
        for (arm, suffix) in [
            (BootRelayArm::Trial, "trial-io"),
            (BootRelayArm::Confirm, "confirm-io"),
            (BootRelayArm::Cancel, "cancel-io"),
            (BootRelayArm::Status, "status-io"),
        ] {
            let ota = relay_boot_err(arm, &BootCommandError::Io(StoreError::Unavailable));
            assert_eq!(
                ota,
                alloc::vec::Vec::from(format!("ERR:{suffix}").as_bytes())
            );
        }
        assert_eq!(
            relay_io_err(&BootCommandError::Io(StoreError::Unavailable), "guard-io"),
            alloc::vec::Vec::from(&b"ERR:guard-io"[..])
        );
        assert_err_wordform_space();
    }

    /// 8B `ERR:<word>` 陷阱（本卷修复的次生面）：`ERR:` 判定**先于**
    /// len==8 seq 判定——假想四字符词的 8B 错误帧不得被解成 seq 记
    /// Completed。
    #[test]
    fn rollback_checks_err_prefix_before_eight_byte_seq() {
        // 4 字符词 = 恰 8B 的 ERR 帧（旧实现此处误记 Completed seq）。
        let trap: &[u8] = b"ERR:noop";
        assert_eq!(trap.len(), 8);
        let (status, detail) = update_rollback_outcome(Some(trap));
        assert_eq!(status, RpcStatus::Failed);
        assert_eq!(detail, "update-rollback:noop");
    }

    /// 成功 seq 8B → Completed（正例保持）；非 8B / 空载 → bad-frame。
    #[test]
    fn rollback_success_and_bad_frames() {
        let seq7 = 7u64.to_le_bytes();
        let (status, detail) = update_rollback_outcome(Some(&seq7));
        assert_eq!(status, RpcStatus::Completed);
        assert_eq!(detail, "update-rollback:trial-canceled seq=7");

        let (status, detail) = update_rollback_outcome(Some(&seq7[..4]));
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-rollback:bad-frame")
        );
        let (status, detail) = update_rollback_outcome(None);
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-rollback:bad-frame")
        );
    }

    /// update-check 判据：正常文本 Completed；`ERR:` 词 Failed；
    /// 空文本 = 异常应答显式失败（本卷新锁）；坏 UTF-8 bad-frame。
    #[test]
    fn update_check_text_verdicts() {
        let text = b"seq=2 floor=1 A=14/1/0/1 B=15/0/3/2";
        let (status, detail) = update_check_outcome(Some(text));
        assert_eq!(status, RpcStatus::Completed);
        assert_eq!(
            detail,
            format!("update-status:{}", core::str::from_utf8(text).unwrap())
        );

        let (status, detail) = update_check_outcome(Some(b"ERR:read0"));
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-check:read0")
        );

        let (status, detail) = update_check_outcome(Some(b""));
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-check:empty-status")
        );

        let (status, detail) = update_check_outcome(Some(&[0xff, 0xfe, 0xfd]));
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-check:bad-frame")
        );

        let (status, detail) = update_check_outcome(None);
        assert_eq!(
            (status, detail.as_str()),
            (RpcStatus::Failed, "update-check:root-unbound")
        );
    }

    /// 成功段判据：seq 严格 8B；status 文本非空且 UTF-8 合法。
    #[test]
    fn relay_ok_strict_lengths() {
        let seq = [2u8; 8];
        assert!(relay_boot_ok(BootRelayArm::Trial, &seq).is_ok());
        assert!(relay_boot_ok(BootRelayArm::Confirm, &seq).is_ok());
        assert!(relay_boot_ok(BootRelayArm::Cancel, &seq).is_ok());
        // 7B / 9B = 坏帧。
        assert!(relay_boot_ok(BootRelayArm::Trial, &seq[..7]).is_err());
        let mut nine = alloc::vec::Vec::from(&seq[..]);
        nine.push(0);
        assert!(relay_boot_ok(BootRelayArm::Cancel, &nine).is_err());
        // status：非空 UTF-8 过；空 / 坏字节拒。
        assert!(relay_boot_ok(BootRelayArm::Status, b"seq=2 floor=1").is_ok());
        assert!(relay_boot_ok(BootRelayArm::Status, b"").is_err());
        assert!(relay_boot_ok(BootRelayArm::Status, &[0xff, 0xfe]).is_err());
    }
}
