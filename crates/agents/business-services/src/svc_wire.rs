//! R14-E（2026-09-24）：控制服务 bin 族信封单源——magic(4) ‖ cmd(1) ‖
//! payload 信封与三类错误词形。原 8 处手抄（svc-market/ledger/agent-bus/
//! cloud-sync/config/observability/ota 七 bin + `ota_service` 库内
//! A/B 命令路径）逐字收编；wire 字节零变化。
//!
//! 层位：本模块只裁**信封层**；market/ledger/cloud-sync 载荷 codec 已
//! 入 lib [`crate::mkt_wire`]/[`crate::ldg_wire`]/[`crate::cld_wire`]
//! （WP-75.2.6；其余服务 module 就地持有；跨 binary 的 obs_wire/
//! ota_wire 等单源在 agent-sdk，不在此重复）。MGT1 不走本
//! 信封族（session 形状判别单源在 [`crate::mgmt_principal`]）。

/// 信封不符（magic 不匹配或总长 < 5）。
pub const ERR_PROTO: &[u8] = b"ERR:proto";
/// 载荷长度不符。
pub const ERR_LEN: &[u8] = b"ERR:len";
/// 未知 opcode。
pub const ERR_CMD: &[u8] = b"ERR:cmd";

/// 信封判别：`req` = magic(4) ‖ cmd(1) ‖ payload。
///
/// magic 不符或总长 < 5 → `None`（调用方回 [`ERR_PROTO`]）；
/// 否则 `Some((cmd, payload))`——`payload` 即 `req[5..]`（借用自 `req`）。
pub fn parse_envelope<'a>(magic: &[u8; 4], req: &'a [u8]) -> Option<(u8, &'a [u8])> {
    if req.len() < 5 || &req[..4] != magic {
        return None;
    }
    Some((req[4], &req[5..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_accepts_exact_magic_and_splits_payload() {
        let req = [b'M', b'K', b'T', b'1', 0x07, 0xAA, 0xBB];
        let (cmd, payload) = parse_envelope(b"MKT1", &req).expect("合法信封");
        assert_eq!(cmd, 0x07);
        assert_eq!(payload, &[0xAA, 0xBB]);
        // 恰 5B（空 payload）也合法。
        let (cmd, payload) = parse_envelope(b"MKT1", b"MKT1\x01").expect("空 payload 信封");
        assert_eq!(cmd, 0x01);
        assert!(payload.is_empty());
    }

    #[test]
    fn envelope_rejects_wrong_magic_and_short_frame() {
        assert!(parse_envelope(b"MKT1", b"LDG1\x01").is_none(), "magic 不符");
        // len==4（纯 magic 无 cmd）= 下界边界（复查轮补）。
        assert!(parse_envelope(b"MKT1", b"MKT1").is_none(), "长度 ==4 边界");
        assert!(parse_envelope(b"MKT1", b"MKT").is_none(), "长度 <5");
        assert!(parse_envelope(b"MKT1", b"").is_none(), "空帧");
    }

    /// 错误词形冻结（请求方/QEMU 门可 grep 的 wire 形态；测试内字面 =
    /// 独立 oracle——改常量词形此测红）。
    #[test]
    fn err_word_forms_frozen() {
        assert_eq!(ERR_PROTO, b"ERR:proto");
        assert_eq!(ERR_LEN, b"ERR:len");
        assert_eq!(ERR_CMD, b"ERR:cmd");
    }
}
