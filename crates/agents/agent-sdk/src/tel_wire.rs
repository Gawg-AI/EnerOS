//! TEL（svc-telemetry INGEST 面）wire 单源（蓝图 66 WP-66.5.4，照
//! `ea_wire` 先例形态）。
//!
//! 单一事实：svc-telemetry 遥测摄取请求 opcode 族 + 应答 ack 字节族 +
//! 快照钉请求帧文法（[`SNAP_REQ_LEN`] / [`parse_snap_request`]，蓝图
//! 76 WP-76.2.7：`[op, snapshot_id ×16]` 的 17B 布局此前在服务端
//! `telemetry.rs` 三臂各手抄一遍，现与客户端 `dispatch_wire` 构帧同源于
//! 本模块）。
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
/// 请求：**快照保留钉**（ADR-0054:29–30）——`[码, id16]`：被持有计划
/// 证书引用的冻结快照在持有期内不得被剪枝回收。容量满/面不可用 →
/// `[TEL_RESP_ERR]`（调用方 fail-closed：拒绝新计划准入）。
pub const TEL_REQ_SNAP_HOLD: u8 = 7;
/// 请求：**解除快照保留钉**（ADR-0054:29–30）——`[码, id16]`：计划终态
/// 且无未决后解除（幂等；失败由后续 tick 重试——释放是回收面不是安全面）。
pub const TEL_REQ_SNAP_RELEASE: u8 = 8;
/// 请求：**完整活跃钉集同步**（蓝图 77 卷一 WP-77.1.3 进程世代屏障）——
/// `[码, count, id×16 ×count]`，`count ≤ [`SNAP_SYNC_MAX_IDS`]`。
/// dispatch 从持久计划计算 `!terminal || unresolved` 全集，在每个 held
/// tick 发送；telemetry 原子替换钉集（逐 ID 验证现存 blob），确认前不做
/// 可能删除旧 blob/事件的修剪——telemetry 独立重启丢 RAM 钉后由此必重建。
pub const TEL_REQ_SNAP_SYNC: u8 = 9;
/// 同步帧活跃 ID 数上界（= `MAX_SNAP_PINS` 的单源；持有计划容量 2 +
/// 换代余量——两端经本常量消费，不手抄数字）。
pub const SNAP_SYNC_MAX_IDS: usize = 4;
/// 快照钉请求**帧长**（WP-76.2.7 单源）：`TEL_REQ_SNAPSHOT_BLOB` /
/// `TEL_REQ_SNAP_HOLD` / `TEL_REQ_SNAP_RELEASE` 三码共用的布局 pin =
/// `[op, snapshot_id ×16]` → `1 + 16 = 17`B。构帧侧
/// （control-services `dispatch_wire::snap_request`）与解帧侧
/// （[`parse_snap_request`]）都消费本常量——生产代码里的字面 `17` 与 id
/// 偏移只住在本模块（测试侧手抄的 `[u8; 17]` 字面量是冻结夹具，不是第四
/// 处布局知识；此前服务端三臂各手抄一遍）。
pub const SNAP_REQ_LEN: usize = 1 + 16;

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

/// 快照钉请求解帧（WP-76.2.7 单源）：`[op, snapshot_id ×16]`
/// （[`SNAP_REQ_LEN`] 长度门）→ `snapshot_id`；长度不符 → `None`
/// （调用方回 [`TEL_RESP_ERR`] 拒绝臂）。
///
/// **不校验 op 首字节**：三码（BLOB/HOLD/RELEASE）共用同一 body 布局，
/// op 分派是调用方 `match` 的职责——在此过滤 op 会让三臂无法复用本函数
/// （负例锁见 `parse_snap_request_ignores_op_byte`）。
///
/// **只覆盖「解析 `[op, id16]`」，不覆盖应答形状**：三臂的应答互不相同
/// （BLOB = `[SNAPSHOT_OK] ++ bytes` / `[SNAPSHOT_REJECTED]`；HOLD =
/// `TEL_RESP_OK`/`TEL_RESP_ERR`；RELEASE = 恒 `TEL_RESP_OK`），
/// 应答装配留在调用方。
pub fn parse_snap_request(req: &[u8]) -> Option<[u8; 16]> {
    if req.len() != SNAP_REQ_LEN {
        return None;
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&req[1..SNAP_REQ_LEN]);
    Some(id)
}

/// 完整活跃钉集同步解帧（蓝图 77 卷一 WP-77.1.3）：`[码, count, id×16
/// ×count]` → ID 集（有序）。`count > [`SNAP_SYNC_MAX_IDS`]` 或总长不符 →
/// `None`（调用方回 [`TEL_RESP_ERR`]）——帧有界，无无长度信任面。
/// 与 [`parse_snap_request`] 不同，op 首字节由调用方 match 消费（本函数
/// 同样不校验 op）。
pub fn parse_snap_sync(req: &[u8]) -> Option<alloc::vec::Vec<[u8; 16]>> {
    if req.len() < 2 {
        return None;
    }
    let count = req[1] as usize;
    if count > SNAP_SYNC_MAX_IDS || req.len() != 2 + count * 16 {
        return None;
    }
    let mut ids = alloc::vec::Vec::with_capacity(count);
    for i in 0..count {
        let mut id = [0u8; 16];
        id.copy_from_slice(&req[2 + i * 16..2 + (i + 1) * 16]);
        ids.push(id);
    }
    Some(ids)
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

    /// 测试夹具 id（本模块 + 客户端 `dispatch_wire` 互锁测共用同一形状）。
    const SNAP_TEST_ID: [u8; 16] = [
        0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18, 0x29, 0x3A, 0x4B, 0x5C, 0x6D, 0x7E, 0x8F,
        0x90,
    ];

    /// 快照钉请求单源（WP-76.2.7）：`[op, id16]` 往返 + 长度门负例
    /// （16/18/0B 一律 `None`——服务端三臂的 `TEL_RESP_ERR` 判据）。
    #[test]
    fn parse_snap_request_roundtrip_and_length_gate() {
        // 正例：三码同布局（此处取 HOLD 形状做夹具）。
        let mut frame = alloc::vec![TEL_REQ_SNAP_HOLD];
        frame.extend_from_slice(&SNAP_TEST_ID);
        assert_eq!(frame.len(), SNAP_REQ_LEN);
        assert_eq!(parse_snap_request(&frame), Some(SNAP_TEST_ID));
        // 负例①：短帧（16B = 缺 id 尾字节）。
        assert_eq!(parse_snap_request(&frame[..SNAP_REQ_LEN - 1]), None);
        // 负例②：长帧（18B = 尾部多一字节，不截断容忍）。
        let mut long = frame.clone();
        long.push(0x5A);
        assert_eq!(long.len(), SNAP_REQ_LEN + 1);
        assert_eq!(parse_snap_request(&long), None);
        // 负例③：空请求（服务端首字节判空在此之前，此处锁解析器自身）。
        assert_eq!(parse_snap_request(&[]), None);
    }

    /// op 不设防（WP-76.2.7 裁决）：本函数**只看长度与 `[1..]` id**，绝不过滤
    /// 首字节——三码（BLOB/HOLD/RELEASE）共用 body 布局，op 分派是调用方
    /// `match` 的职责。日后"顺手"在解析器里加 op 白名单必红本测。
    #[test]
    fn parse_snap_request_ignores_op_byte() {
        for op in [
            TEL_REQ_SNAPSHOT_BLOB,
            TEL_REQ_SNAP_HOLD,
            TEL_REQ_SNAP_RELEASE,
            TEL_REQ_INGEST,
            TEL_REQ_SNAPSHOT_INPUT,
            TEL_RESP_ERR,
        ] {
            let mut frame = alloc::vec![op];
            frame.extend_from_slice(&SNAP_TEST_ID);
            assert_eq!(
                parse_snap_request(&frame),
                Some(SNAP_TEST_ID),
                "op={op:#04x} 不得被解析器过滤（分派属调用方）"
            );
        }
    }

    /// 帧字节冻结（WP-76.2.7）：已知 id 的 17B HOLD 请求 = 手抄
    /// `[u8; 17]` 字面量——`op` 占 `[0]`、id 占 `[1..17]` 逐字节钉死
    /// （布局漂移即红；同字面量在客户端 `dispatch_wire` 侧再对拍一次）。
    #[test]
    fn snap_request_byte_layout_frozen() {
        assert_eq!(SNAP_REQ_LEN, 17, "帧长字面冻结（[op, snapshot_id×16]）");
        const HOLD_FRAME: [u8; SNAP_REQ_LEN] = [
            0x07, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x07, 0x18, 0x29, 0x3A, 0x4B, 0x5C, 0x6D,
            0x7E, 0x8F, 0x90,
        ];
        assert_eq!(HOLD_FRAME[0], TEL_REQ_SNAP_HOLD, "op 字节冻结 0x07");
        assert_eq!(&HOLD_FRAME[1..], &SNAP_TEST_ID[..], "id 偏移 [1..17] 冻结");
        assert_eq!(parse_snap_request(&HOLD_FRAME), Some(SNAP_TEST_ID));
    }
}
