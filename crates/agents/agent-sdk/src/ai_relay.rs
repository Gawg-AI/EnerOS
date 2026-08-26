//! 49（WP-49.2 远程 dual-brain）：svc-ai-worker ↔ svc-mqtt AI 中继线码。
//!
//! 单一事实源（schema.rs 纪律同款：两侧常量禁止各自定义漂移）：
//! - svc-ai-worker → svc-mqtt（drv-svc EXECUTE 载荷子码，PAYLOAD_CAP=64B
//!   单帧约束 → 请求 JSON 分片组装）；
//! - svc-mqtt → svc-ai-worker（ChunkedClient 分块推送，载荷首字节子码）。
//!
//! 契约解释权在 intent-contract::peer + control-services::remote_brain；
//! 本模块只定义**传输线码**，不含语义。

/// EXECUTE 载荷子码：AI 请求分片首帧。载荷 = `[AI_REQ_BEGIN][total u16 LE]
/// [data ≤61B]`（payload_len ≥ 3）。
pub const AI_REQ_BEGIN: u8 = 0x2A;
/// EXECUTE 载荷子码：AI 请求分片中继帧。载荷 = `[AI_REQ_CONT][data ≤63B]`。
pub const AI_REQ_CONT: u8 = 0x2B;
/// EXECUTE 载荷子码：AI 请求收尾帧（组装完成 → 插件 outbox 入队）。
/// 载荷 = `[AI_REQ_END]`（payload_len == 1）。
pub const AI_REQ_END: u8 = 0x2C;

/// ChunkedClient 推送载荷子码：peer 应答转发（svc-mqtt → svc-ai-worker）。
/// 载荷 = `[AI_PEER_MSG][应答 JSON 字节]`。
pub const AI_PEER_MSG: u8 = 0x10;

/// 组装总长上限（= intent-contract PEER_REQUEST_MAX_LEN 同量级；传输面
/// 只认字节数——超限即中止组装并计数）。
pub const AI_REQUEST_MAX: usize = 1024;

/// 分片数据容量（drv 帧 PAYLOAD_CAP=64 − 子码 1B；BEGIN 再减 total u16 2B）。
pub const AI_REQ_DATA_BEGIN_MAX: usize = 64 - 3;
/// CONT/END 数据容量。
pub const AI_REQ_DATA_MAX: usize = 64 - 1;

/// 分片失败（空载荷/超限；蓝图 59 起单源在 relay_face，此处 re-export
/// 保持既有导入路径稳定）。
pub use crate::relay_face::FragmentError;

/// AI 请求中继面声明（蓝图 59：分片算法单源在 relay_face）。
pub const FACE: crate::relay_face::RelayFaceSpec = crate::relay_face::RelayFaceSpec {
    begin: AI_REQ_BEGIN,
    cont: AI_REQ_CONT,
    end: AI_REQ_END,
    max_total: AI_REQUEST_MAX,
};

/// 请求 JSON → (total_len, frames)：frames[0] = BEGIN 载荷（不含 EXECUTE
/// opcode——drv_call 侧再拼），其余为 CONT 载荷；末尾追加 END 帧。
/// 发送方按序 drv_call；中途失败直接放弃（接收方 BEGIN 隐式中止残片）。
/// 蓝图 59：委托 [`FACE::fragment`]（原手写算法已收敛至 relay_face 单源）。
pub fn fragment_request(
    json: &[u8],
) -> Result<(u16, alloc::vec::Vec<alloc::vec::Vec<u8>>), FragmentError> {
    FACE.fragment(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 49：线码互斥性（防手滑撞既有子码）。
    #[test]
    fn wire_codes_disjoint_from_existing_subcodes() {
        let ai = [AI_REQ_BEGIN, AI_REQ_CONT, AI_REQ_END];
        // drv EXECUTE 已占用子码面：0x11 = OP_MASTER_WRITE
        // （protocol-abstract::write 同值锚定——漂移即本测试失效提示）。
        const OP_MASTER_WRITE: u8 = 0x11;
        for c in ai {
            assert_ne!(c, OP_MASTER_WRITE);
        }
        // chunked push 面与控制服务请求码不冲突：ai_worker AI_PROPOSE=1。
        assert_ne!(AI_PEER_MSG, 1u8);
    }

    /// 49：容量常量自洽（单帧不越 PAYLOAD_CAP）。
    #[test]
    fn frame_capacity_within_payload_cap() {
        // drv-svc runtime PAYLOAD_CAP=64 同值锚定（编译期断言，防常量漂移）。
        const PAYLOAD_CAP: usize = 64;
        const _: () = assert!(1 + 2 + AI_REQ_DATA_BEGIN_MAX <= PAYLOAD_CAP);
        const _: () = assert!(AI_REQ_DATA_MAX < PAYLOAD_CAP);
        assert_eq!(AI_REQUEST_MAX, 1024);
    }

    /// 49：分片往返——frames 重组还原原 JSON（与 ai_relay 组装器同语义）。
    #[test]
    fn fragment_request_roundtrip() {
        use alloc::{vec, vec::Vec};
        let json: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8 + 1).collect();
        let (total, frames) = fragment_request(&json).expect("fragment");
        assert_eq!(total as usize, json.len());
        // 首帧 = BEGIN[total][head]。
        assert_eq!(frames[0][0], AI_REQ_BEGIN);
        assert_eq!(&frames[0][1..3], &total.to_le_bytes());
        // 重组。
        let mut out = Vec::new();
        out.extend_from_slice(&frames[0][3..]);
        for f in &frames[1..frames.len() - 1] {
            assert_eq!(f[0], AI_REQ_CONT);
            out.extend_from_slice(&f[1..]);
        }
        let last = frames.last().expect("end frame");
        assert_eq!(last, &vec![AI_REQ_END]);
        assert_eq!(out, json);
    }

    /// 49：空载荷 / 超限拒绝。
    #[test]
    fn fragment_request_rejects_empty_and_oversize() {
        assert!(fragment_request(&[]).is_err());
        let big = [0u8; AI_REQUEST_MAX + 1];
        assert!(fragment_request(&big).is_err());
    }
}
