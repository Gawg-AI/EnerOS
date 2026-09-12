//! MGT2 —— root 管理端点 wire（70.6 WP-70.6.3：受控维护与真实服务重启）。
//!
//! svc-mgmt ↔ root 的最小管理面：svc-mgmt 作为唯一授权发起方（授予边
//! `("svc-mgmt","root-mgmt")`，EIPC 槽由授予表按名解析），root 在主循环
//! 非阻塞轮询该端点、先处理再回复（[`ipc_reply`]——Poll 暗装 reply cap）。
//!
//! wire：`"MGT2" ‖ cmd(1) ‖ arg`，单 MR 帧（≤952B 上限内；实际 ≤ 21B）。
//!
//! | cmd | 请求 arg | 响应 |
//! |---|---|---|
//! | 0x01 RESTART | target(≤16B) | `1`（已受理）‖ `ERR:<原因>` |
//! | 0x02 INSTANCE | target(≤16B) | gen(8) ‖ lifecycle(1) ‖ restarts(8) ‖ ready(1) ‖ `ERR:<原因>` |
//! | 0x03 MAINT-ENTER | — | `1` ‖ `ERR:<原因>` |
//! | 0x04 MAINT-EXIT | — | `1` ‖ `ERR:<原因>` |
//! | 0x05 MAINT-QUERY | — | active(1) ‖ entered_tick(8) |
//! | 0x06 UPDATE-STATUS | — | OTA1 0x0E 投影文本 ‖ `ERR:<原因>` |
//! | 0x07 UPDATE-ROLLBACK | — | seq(8) ‖ `ERR:<原因>`（no-pending-trial 等） |
//!
//! 编解码为纯函数（host 可测）；目标 FFI 仅 [`RootMgmtClient`]。

use alloc::vec;
use alloc::vec::Vec;

/// 协议 magic。
pub const PROTO_MAGIC: &[u8; 4] = b"MGT2";
/// 重启请求（授权目标；root 侧 allow/deny 裁决）。
pub const CMD_RESTART: u8 = 0x01;
/// 实例查询（代次/生命周期/重启计数/就绪）。
pub const CMD_INSTANCE: u8 = 0x02;
/// 维护进入（停止新的控制写）。
pub const CMD_MAINT_ENTER: u8 = 0x03;
/// 维护退出（健康门：required 服务全 Ready 才放行）。
pub const CMD_MAINT_EXIT: u8 = 0x04;
/// 维护状态查询。
pub const CMD_MAINT_QUERY: u8 = 0x05;
/// 70.7.5：更新状态查询（root 中继 svc-ota OTA1 0x0E 只读投影）。
pub const CMD_UPDATE_STATUS: u8 = 0x06;
/// 70.7.5：回退（取消未确认 trial；root 中继 OTA1 0x0D）。
pub const CMD_UPDATE_ROLLBACK: u8 = 0x07;

/// 目标名定长（零填充；覆盖最长服务名 `svc-control-dispatch` 20B）。
pub const TARGET_LEN: usize = 24;
/// 帧头长度（magic 4 ‖ cmd 1）。
pub const HEADER_LEN: usize = 5;

/// 编码目标名（零填充 16B；超长/非 ASCII 以下划线截断替换——svc-mgmt
/// 侧只发已知服务名；root 侧再验名册）。
pub fn encode_target(target: &str) -> Vec<u8> {
    let mut out = vec![0u8; TARGET_LEN];
    let b = target.as_bytes();
    let n = core::cmp::min(b.len(), TARGET_LEN);
    out[..n].copy_from_slice(&b[..n]);
    out
}

/// 构造请求帧。
pub fn encode_request(cmd: u8, arg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + arg.len());
    out.extend_from_slice(PROTO_MAGIC);
    out.push(cmd);
    out.extend_from_slice(arg);
    out
}

pub fn encode_restart(target: &str) -> Vec<u8> {
    encode_request(CMD_RESTART, &encode_target(target))
}

pub fn encode_instance_query(target: &str) -> Vec<u8> {
    encode_request(CMD_INSTANCE, &encode_target(target))
}

pub fn encode_maint_enter() -> Vec<u8> {
    encode_request(CMD_MAINT_ENTER, &[])
}

pub fn encode_maint_exit() -> Vec<u8> {
    encode_request(CMD_MAINT_EXIT, &[])
}

pub fn encode_maint_query() -> Vec<u8> {
    encode_request(CMD_MAINT_QUERY, &[])
}

/// 70.7.5：更新状态查询帧（root 中继 OTA1 0x0E；只读，维护态可用）。
pub fn encode_update_status() -> Vec<u8> {
    encode_request(CMD_UPDATE_STATUS, &[])
}

/// 70.7.5：更新回退帧（取消未确认 trial；root 中继 OTA1 0x0D）。
pub fn encode_update_rollback() -> Vec<u8> {
    encode_request(CMD_UPDATE_ROLLBACK, &[])
}

/// 解析请求帧（root 侧；返回 (cmd, arg)）。
#[allow(clippy::result_unit_err)] // MGT2 解析错误面 = ()（协议常量型，MgmtAction::parse 先例）
pub fn parse_request(req: &[u8]) -> Result<(u8, &[u8]), ()> {
    if req.len() < HEADER_LEN || &req[..4] != PROTO_MAGIC {
        return Err(());
    }
    Ok((req[4], &req[5..]))
}

/// 解析目标名（零截断；空/超长拒绝）。
#[allow(clippy::result_unit_err)] // MGT2 解析错误面 = ()（协议常量型，MgmtAction::parse 先例）
pub fn parse_target(arg: &[u8]) -> Result<&str, ()> {
    if arg.len() != TARGET_LEN {
        return Err(());
    }
    let end = arg.iter().position(|&b| b == 0).unwrap_or(TARGET_LEN);
    let s = core::str::from_utf8(&arg[..end]).map_err(|_| ())?;
    if s.is_empty() {
        return Err(());
    }
    Ok(s)
}

/// 成功响应（`1`）。
pub fn ok_resp() -> Vec<u8> {
    vec![1]
}

/// 错误响应（`ERR:<原因>`）。
pub fn err_resp(reason: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + reason.len());
    out.extend_from_slice(b"ERR:");
    out.extend_from_slice(reason.as_bytes());
    out
}

/// 解析 ack 类响应（restart/maint-enter/exit）：`1` = Ok，`ERR:*` = Err(原因)。
pub fn parse_ack(resp: &[u8]) -> Result<(), &str> {
    if resp.first() == Some(&1) {
        return Ok(());
    }
    if let Some(reason) = resp.strip_prefix(b"ERR:") {
        return Err(core::str::from_utf8(reason).unwrap_or("?"));
    }
    Err("bad-resp")
}

/// 实例查询响应布局：gen(8) ‖ lifecycle(1) ‖ restarts(8) ‖ ready(1) = 18B。
pub const INSTANCE_RESP_LEN: usize = 18;

/// 编码实例查询响应（root 侧）。
pub fn encode_instance_resp(gen: u64, lifecycle: u8, restarts: u64, ready: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(INSTANCE_RESP_LEN);
    out.extend_from_slice(&gen.to_le_bytes());
    out.push(lifecycle);
    out.extend_from_slice(&restarts.to_le_bytes());
    out.push(u8::from(ready));
    out
}

/// 解析实例查询响应（svc-mgmt 侧）。
#[allow(clippy::result_unit_err)] // MGT2 解析错误面 = ()（协议常量型，MgmtAction::parse 先例）
pub fn parse_instance_resp(resp: &[u8]) -> Result<(u64, u8, u64, bool), ()> {
    if resp.len() < INSTANCE_RESP_LEN {
        return Err(());
    }
    Ok((
        u64::from_le_bytes(resp[0..8].try_into().map_err(|_| ())?),
        resp[8],
        u64::from_le_bytes(resp[9..17].try_into().map_err(|_| ())?),
        resp[17] == 1,
    ))
}

/// 维护查询响应布局：active(1) ‖ entered_tick(8) = 9B。
pub const MAINT_QUERY_RESP_LEN: usize = 9;

pub fn encode_maint_query_resp(active: bool, entered_tick: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAINT_QUERY_RESP_LEN);
    out.push(u8::from(active));
    out.extend_from_slice(&entered_tick.to_le_bytes());
    out
}

#[allow(clippy::result_unit_err)] // MGT2 解析错误面 = ()（协议常量型，MgmtAction::parse 先例）
pub fn parse_maint_query_resp(resp: &[u8]) -> Result<(bool, u64), ()> {
    if resp.len() < MAINT_QUERY_RESP_LEN {
        return Err(());
    }
    Ok((
        resp[0] == 1,
        u64::from_le_bytes(resp[1..9].try_into().map_err(|_| ())?),
    ))
}

// ============================================================================
// 目标 FFI（svc-mgmt 侧客户端；单 MR 帧 ipc_call 往返）
// ============================================================================

/// root-mgmt 端点客户端（svc-mgmt 侧；同步 Call——root 单帧内回复，
/// 阻塞时长 = root 单轮处理，与 drv_call 同纪律）。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub struct RootMgmtClient {
    ep: u64,
}

#[cfg(all(target_os = "none", feature = "sel4"))]
impl RootMgmtClient {
    /// 按授予槽绑定（授予边缺失 → None，fail-closed）。
    pub fn bind(ep: u64) -> Option<Self> {
        Some(RootMgmtClient { ep })
    }

    /// 单帧往返（帧经 MR 编解码单源；解析/长度错误 → None）。
    pub fn call(&mut self, req: &[u8]) -> Option<Vec<u8>> {
        use eneros_sel4_sys::safe::{get_mr, ipc_call, message_info_new, set_mr};
        let mrs = eneros_ipc_wire::mr::frame_into_mrs(req).ok()?;
        for (i, v) in mrs.iter().enumerate() {
            set_mr(i as u64, *v).ok()?;
        }
        let mi = message_info_new(0, 0, 0, mrs.len() as u64).ok()?;
        let resp_mi = ipc_call(self.ep, mi).ok()?;
        let len = eneros_sel4_sys::types::message_info_length(resp_mi);
        let mut words = alloc::vec::Vec::with_capacity(len as usize);
        for i in 0..len {
            words.push(get_mr(i).ok()?);
        }
        eneros_ipc_wire::mr::frame_from_mrs(&words).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_encode_parse_roundtrip() {
        let req = encode_restart("svc-observability");
        assert_eq!(&req[..4], b"MGT2");
        assert_eq!(req[4], CMD_RESTART);
        assert_eq!(req.len(), HEADER_LEN + TARGET_LEN);
        let (cmd, arg) = parse_request(&req).unwrap();
        assert_eq!(cmd, CMD_RESTART);
        assert_eq!(parse_target(arg).unwrap(), "svc-observability");
        // 最长真实服务名（20B）恰好在界内。
        let long = encode_restart("svc-control-dispatch");
        let (_, arg2) = parse_request(&long).unwrap();
        assert_eq!(parse_target(arg2).unwrap(), "svc-control-dispatch");
    }

    #[test]
    fn target_parse_rejects_empty_and_bad_length() {
        assert!(parse_target(&[0u8; 24]).is_err(), "空目标拒绝");
        assert!(parse_target(&[0u8; 15]).is_err());
        assert!(parse_target(&encode_target("ok-name")).is_ok());
    }

    #[test]
    fn parse_request_rejects_bad_magic_and_short() {
        assert!(parse_request(b"MGT1\x01").is_err());
        assert!(parse_request(b"MGT2").is_err());
        assert!(parse_request(&[]).is_err());
    }

    #[test]
    fn ack_resp_ok_and_err() {
        assert_eq!(parse_ack(&ok_resp()), Ok(()));
        assert_eq!(
            parse_ack(&err_resp("restart-denied")),
            Err("restart-denied")
        );
        assert!(parse_ack(b"").is_err());
        assert!(parse_ack(b"junk").is_err());
    }

    #[test]
    fn instance_and_maint_query_layouts() {
        let r = encode_instance_resp(7, 3, 2, true);
        assert_eq!(r.len(), INSTANCE_RESP_LEN);
        assert_eq!(parse_instance_resp(&r).unwrap(), (7, 3, 2, true));
        assert!(parse_instance_resp(&r[..10]).is_err());
        let m = encode_maint_query_resp(true, 42);
        assert_eq!(m.len(), MAINT_QUERY_RESP_LEN);
        assert_eq!(parse_maint_query_resp(&m).unwrap(), (true, 42));
        assert!(parse_maint_query_resp(&m[..4]).is_err());
    }
}
