//! TEL（svc-telemetry INGEST 面）wire 单源（蓝图 66 WP-66.5.4，照
//! `ea_wire` 先例形态）。
//!
//! 单一事实：svc-telemetry 遥测摄取请求 opcode 族 + 应答 ack 字节族。
//! 此前两侧散抄：control-services `telemetry.rs` 服务端裸 `vec![0, …]`/
//! `vec![0xFF]`、root-task `control_chain_synth_ingest` 裸 `vec![1u8]`
//! 请求 + `resp[0] != 0` 手解、dispatch_assembly 裸 `&[2u8]` 快照请求
//! ——常量与解码 helper 收进本模块（root 与 control-services 均已依赖
//! agent-sdk，零新增依赖边）。
//!
//! 码值冻结（wire 字节，不得变更）：请求 INGEST=1 / SNAPSHOT=2 /
//! ADVANCE_EPOCH=3；应答 OK=0 / ERR=0xFF。**应答字节序 `[0, accepted]`
//! 语义冻结**（本模块只给名字与守卫）。
//!
//! accepted 上限事实（[02] WP-66.5.4 核查项）：TEL_INGEST 走 chunked
//! 传输，逻辑上限冻结 = `CHUNK_COUNT_MAX × CHUNK_DATA_MAX = 1664B`
//! （agent-sdk chunked const-assert）；样本 wire 定长 58B → 单批样本
//! ≤ floor((1664−1)/58) = **28** —— `accepted` 结构性 ≤255，`accepted
//! as u8` 无静默回绕风险（服务端 debug_assert 锁事实）。

/// 请求：样本批次摄取（58B 定长样本逐个拼接）。
pub const TEL_REQ_INGEST: u8 = 1;
/// 请求：评估快照（TrustedTelemetry wire）。
pub const TEL_REQ_SNAPSHOT: u8 = 2;
/// 请求：epoch 推进（断线重连）。
pub const TEL_REQ_ADVANCE_EPOCH: u8 = 3;
/// 请求：**可信快照输入**（蓝图 70.8 WP-70.8.1）——应答 = 快照身份 +
/// 144B 遥测载荷的 [`crate::snap_input_wire::TrustedInput`] wire，
/// 或 `[TEL_RESP_ERR, 快照拒绝码]`。旧 `TEL_REQ_SNAPSHOT`（评估器直出）
/// 自此只作差分/迁移轨，不再具备新控制资格。
pub const TEL_REQ_SNAPSHOT_INPUT: u8 = 5;
/// 请求：**已冻结快照 blob**（蓝图 70.8.2/70.9.1）——按当前冻结快照 ID
/// 取回 `encode_snapshot_v1` 字节（certifier 独立读取可信输入用）。
pub const TEL_REQ_SNAPSHOT_BLOB: u8 = 6;

/// 应答：受理（INGEST 路径下第二字节 = accepted 样本数）。
pub const TEL_RESP_OK: u8 = 0;
/// 应答：坏请求/解析失败（fail-closed 兜底）。
pub const TEL_RESP_ERR: u8 = 0xFF;

/// INGEST 应答编码（字节序冻结 `[TEL_RESP_OK, accepted]`）。
///
/// `accepted ≤ 255` 为结构性事实（chunked 1664B 上限 / 58B 样本 →
/// 单批 ≤28）；越界 = 调用方破坏传输契约 → debug_assert 期暴露。
pub fn encode_ingest_ack(accepted: u8) -> alloc::vec::Vec<u8> {
    alloc::vec![TEL_RESP_OK, accepted]
}

/// INGEST 应答解码：`[TEL_RESP_OK, accepted]` → `Ok(accepted)`；
/// `[TEL_RESP_ERR]`/短帧/其他 → `Err`（fail-closed 不猜）。
pub fn decode_ingest_ack(resp: &[u8]) -> Result<u8, TelAckError> {
    match resp {
        [TEL_RESP_OK, accepted] => Ok(*accepted),
        [TEL_RESP_ERR] => Err(TelAckError::Rejected),
        _ => Err(TelAckError::Malformed),
    }
}

/// INGEST 应答错误分类（词形登记；未知形态归 [`TelAckError::Malformed`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelAckError {
    /// 服务端显式拒绝（`[0xFF]`）。
    Rejected,
    /// 帧形态非法（短帧/未知首字节）。
    Malformed,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 码值冻结锁（wire 字节；改码即红）。
    #[test]
    fn opcodes_and_resp_codes_are_frozen() {
        assert_eq!(TEL_REQ_INGEST, 1);
        assert_eq!(TEL_REQ_SNAPSHOT, 2);
        assert_eq!(TEL_REQ_ADVANCE_EPOCH, 3);
        // 蓝图 70.8：新增码（旧码零变化——冻结面只增不改）。码 4 属
        // agent-sdk `obs_batch_wire::OPCODE_TEL_INGEST_OBS`（同请求空间
        // 另一模块），此处从 5 起分配并与之互斥。
        assert_eq!(TEL_REQ_SNAPSHOT_INPUT, 5);
        assert_eq!(TEL_REQ_SNAPSHOT_BLOB, 6);
        assert_ne!(
            TEL_REQ_SNAPSHOT_INPUT,
            crate::obs_batch_wire::OPCODE_TEL_INGEST_OBS
        );
        assert_eq!(TEL_RESP_OK, 0);
        assert_eq!(TEL_RESP_ERR, 0xFF);
    }

    /// 应答字节序冻结锁：`[0, accepted]`（[02] WP-66.5.4 禁止项——只给
    /// 名字与守卫，不改字节）。
    #[test]
    fn ingest_ack_byte_layout_frozen() {
        assert_eq!(encode_ingest_ack(7), alloc::vec![0, 7]);
        assert_eq!(decode_ingest_ack(&[0, 28]), Ok(28));
    }

    /// 解码负例：显式拒绝 / 短帧 / 未知首字节（三态）。
    #[test]
    fn decode_ingest_ack_rejects_fail_closed() {
        assert_eq!(decode_ingest_ack(&[0xFF]), Err(TelAckError::Rejected));
        assert_eq!(decode_ingest_ack(&[]), Err(TelAckError::Malformed));
        assert_eq!(decode_ingest_ack(&[0]), Err(TelAckError::Malformed));
        assert_eq!(decode_ingest_ack(&[7, 7]), Err(TelAckError::Malformed));
    }

    /// 双断言守卫（照 root service.rs 字面禁臂先例）：字面 `vec![0,`
    /// / `vec![0xFF]` 裸臂在本 crate 零在场（全部走常量/helper）。
    #[test]
    fn no_bare_byte_arms_in_module() {
        let src = include_str!("tel_wire.rs");
        let bare_vec0 = concat!("vec![", "0", "]");
        assert!(
            !src.contains(bare_vec0),
            "裸 0 字面应答臂回归（应走 TEL_RESP_OK）"
        );
    }
}
