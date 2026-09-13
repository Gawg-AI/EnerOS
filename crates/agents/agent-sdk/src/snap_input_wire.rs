//! 蓝图 70.8（WP-70.8.1 / WP-70.8.4）——可信快照输入 wire 单源。
//!
//! # 为什么需要本 wire
//!
//! 70.8 之前，目标侧决策输入 = `TrustedTelemetry`（144B，`TEL_REQ_SNAPSHOT`
//! 评估器产物）：它有 `provenance_digest`（样本摘要）但**没有 snapshot_id、
//! 没有目录代次、没有 cut/produced 时刻**。下游（ai-worker → adapter）因此
//! 只能伪造候选身份（`snapshot_id = input_digest[..16]`、`objective_digest`
//! 全零、`step_ns = 0`）——70.8 的欠账正是「输入同代、来源可追溯」。
//!
//! 本 wire 把**已冻结快照的身份**（svc-telemetry 事件面每 8 批构建一次的
//! `TrustedSnapshot`）与既有 144B 遥测载荷打成一份不可分割的输入：
//! 决策、证书、执行证据全程引用同一个 `snapshot_id` / `provenance_root` /
//! `catalog_version`。身份字段来自快照本体，不由调用方拼接。
//!
//! # 布局（定长，小端；`SNAP_INPUT_WIRE_LEN` = 234B）
//!
//! ```text
//!  0..4    magic "SIV1"
//!  4..20   snapshot_id[16]
//! 20..52   provenance_root[32]
//! 52..60   cut_time_ns
//! 60..68   produced_at_ns
//! 68..76   catalog_version（version_set.contract_version）
//! 76..84   catalog_digest8（目录 repo digest 前 8B——代次证据）
//! 84..86   accepted（快照观测条数）
//! 86..88   missing（缺项数）
//! 88       completeness 码（[`COMPLETE`] / [`INCOMPLETE`]）
//! 89       flags（bit0 = 输入派生自冻结快照；其余保留 0）
//! 90..234  144B `TrustedTelemetry` wire（既有单源编码，本模块不解释）
//! ```
//!
//! 身份 + 载荷一体：调用方无法只取载荷而不带走身份（`decode` 是唯一入口，
//! 长度/魔数/保留位不符即 `None`——fail-closed 不猜）。

use alloc::vec::Vec;

/// 快照输入 wire magic。
pub const SNAP_INPUT_MAGIC: [u8; 4] = *b"SIV1";
/// 内嵌遥测载荷长度（= control-services `TRUSTED_WIRE_LEN` 单一事实；
/// 本模块只搬运字节，不解释语义，避免 agent-sdk → safety-validator 的
/// 编码耦合）。
pub const SNAP_INPUT_TELEMETRY_LEN: usize = 144;
/// 身份段长度（magic…flags）。
pub const SNAP_INPUT_FACTS_LEN: usize = 90;
/// wire 总长。
pub const SNAP_INPUT_WIRE_LEN: usize = SNAP_INPUT_FACTS_LEN + SNAP_INPUT_TELEMETRY_LEN;

/// 完整度码：快照完整（全部必需点齐全且合格）。
pub const COMPLETE: u8 = 0;
/// 完整度码：快照不完整（`missing` 给出缺项数；调用方必须拒绝该输入）。
pub const INCOMPLETE: u8 = 1;

/// flags：输入派生自冻结快照（生产唯一合法形态）。
pub const FLAG_FROZEN: u8 = 0x01;

/// 遥测样本 wire 的前 4 个 f64（电压/电流/频率/SOC）在本模块内的只读
/// 访问偏移——**仅供不需要语义解码的调用方取值**（如日志/断言）；
/// 语义解码一律走 control-services `decode_trusted`（单一事实源）。
pub const TELEMETRY_OFF_V: usize = SNAP_INPUT_FACTS_LEN;
pub const TELEMETRY_OFF_I: usize = SNAP_INPUT_FACTS_LEN + 8;
pub const TELEMETRY_OFF_F: usize = SNAP_INPUT_FACTS_LEN + 16;
pub const TELEMETRY_OFF_SOC: usize = SNAP_INPUT_FACTS_LEN + 24;

/// 快照身份事实（定长、Copy、零堆）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotFacts {
    /// 快照 ID（`build` 产出的 SHA-256 派生值；全零 = 未绑定）。
    pub snapshot_id: [u8; 16],
    /// 快照 provenance root（源集合/序/事件摘要的可复算根）。
    pub provenance_root: [u8; 32],
    /// 快照 cut 时刻（ns）。
    pub cut_time_ns: u64,
    /// 快照产出时刻（ns）。
    pub produced_at_ns: u64,
    /// 目录合同版本（`version_set.contract_version`）。
    pub catalog_version: u64,
    /// 目录 digest 前 8 字节（代次证据）。
    pub catalog_digest8: [u8; 8],
    /// 快照观测条数。
    pub accepted: u16,
    /// 缺失必需点数。
    pub missing: u16,
    /// 完整度码（[`COMPLETE`] / [`INCOMPLETE`]）。
    pub completeness: u8,
}

impl SnapshotFacts {
    /// 零值（未绑定；不得作为控制输入）。
    pub const ZERO: Self = Self {
        snapshot_id: [0u8; 16],
        provenance_root: [0u8; 32],
        cut_time_ns: 0,
        produced_at_ns: 0,
        catalog_version: 0,
        catalog_digest8: [0u8; 8],
        accepted: 0,
        missing: 0,
        completeness: INCOMPLETE,
    };

    /// 未绑定判定（零 ID 或零目录版本——两者任一为零都不是真实快照身份）。
    pub fn is_unbound(&self) -> bool {
        self.snapshot_id == [0u8; 16] || self.catalog_version == 0
    }

    /// 身份段编码（定长 `SNAP_INPUT_FACTS_LEN`）。
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&SNAP_INPUT_MAGIC);
        out.extend_from_slice(&self.snapshot_id);
        out.extend_from_slice(&self.provenance_root);
        out.extend_from_slice(&self.cut_time_ns.to_le_bytes());
        out.extend_from_slice(&self.produced_at_ns.to_le_bytes());
        out.extend_from_slice(&self.catalog_version.to_le_bytes());
        out.extend_from_slice(&self.catalog_digest8);
        out.extend_from_slice(&self.accepted.to_le_bytes());
        out.extend_from_slice(&self.missing.to_le_bytes());
        out.push(self.completeness);
        out.push(FLAG_FROZEN);
    }

    /// 身份段解码（长度/魔数/保留位不符 → `None`）。
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < SNAP_INPUT_FACTS_LEN || buf[..4] != SNAP_INPUT_MAGIC {
            return None;
        }
        let mut snapshot_id = [0u8; 16];
        snapshot_id.copy_from_slice(&buf[4..20]);
        let mut provenance_root = [0u8; 32];
        provenance_root.copy_from_slice(&buf[20..52]);
        let be64 =
            |o: usize| -> Option<u64> { Some(u64::from_le_bytes(buf[o..o + 8].try_into().ok()?)) };
        let cut_time_ns = be64(52)?;
        let produced_at_ns = be64(60)?;
        let catalog_version = be64(68)?;
        let mut catalog_digest8 = [0u8; 8];
        catalog_digest8.copy_from_slice(&buf[76..84]);
        let accepted = u16::from_le_bytes(buf[84..86].try_into().ok()?);
        let missing = u16::from_le_bytes(buf[86..88].try_into().ok()?);
        let completeness = buf[88];
        if buf[89] != FLAG_FROZEN {
            return None; // 未知 flags：本版本不接受（前向兼容由版本号承担）
        }
        if completeness != COMPLETE && completeness != INCOMPLETE {
            return None; // 未知完整度码——不猜
        }
        Some(Self {
            snapshot_id,
            provenance_root,
            cut_time_ns,
            produced_at_ns,
            catalog_version,
            catalog_digest8,
            accepted,
            missing,
            completeness,
        })
    }

    /// 完整快照判定（完整度码 + 无缺项 + 已绑定身份）。
    pub fn is_complete(&self) -> bool {
        self.completeness == COMPLETE && self.missing == 0 && !self.is_unbound()
    }
}

/// 可信输入（身份 + 遥测载荷，一体）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedInput {
    facts: SnapshotFacts,
    telemetry: [u8; SNAP_INPUT_TELEMETRY_LEN],
}

impl TrustedInput {
    /// 构造（载荷长度在编译期固定；调用方持有 `[u8;144]`）。
    pub fn new(facts: SnapshotFacts, telemetry: [u8; SNAP_INPUT_TELEMETRY_LEN]) -> Self {
        Self { facts, telemetry }
    }

    /// 身份事实。
    pub fn facts(&self) -> &SnapshotFacts {
        &self.facts
    }

    /// 快照 ID（`snapshot_id` 便捷取用，避免调用方再次解结构）。
    pub fn snapshot_id(&self) -> &[u8; 16] {
        &self.facts.snapshot_id
    }

    /// 目录合同版本。
    pub fn catalog_version(&self) -> u64 {
        self.facts.catalog_version
    }

    /// 内嵌遥测载荷（144B；语义解码走 control-services `decode_trusted`）。
    pub fn telemetry(&self) -> &[u8; SNAP_INPUT_TELEMETRY_LEN] {
        &self.telemetry
    }

    /// 编码（定长 `SNAP_INPUT_WIRE_LEN`）。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SNAP_INPUT_WIRE_LEN);
        self.facts.encode_into(&mut out);
        out.extend_from_slice(&self.telemetry);
        out
    }

    /// 解码（长度/魔数/保留位不符 → `None`）。
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() != SNAP_INPUT_WIRE_LEN {
            return None;
        }
        let facts = SnapshotFacts::decode(buf)?;
        let mut telemetry = [0u8; SNAP_INPUT_TELEMETRY_LEN];
        telemetry.copy_from_slice(&buf[SNAP_INPUT_FACTS_LEN..]);
        Some(Self { facts, telemetry })
    }

    /// 载荷内某 8 字节 f64（诊断/断言用；语义解码仍走单一事实源）。
    ///
    /// 全函数：仅接受载荷内完整 8 字节窗口（`off ∈
    /// [SNAP_INPUT_FACTS_LEN, SNAP_INPUT_FACTS_LEN + 144 − 8]`）；
    /// 低于载荷起点（含 `usize::MAX`）或越过窗尾一律 `None`——
    /// checked 运算，绝无 usize 下溢/回绕。
    pub fn telemetry_f64(&self, off: usize) -> Option<f64> {
        let o = off.checked_sub(SNAP_INPUT_FACTS_LEN)?;
        let end = o.checked_add(8)?;
        if end > SNAP_INPUT_TELEMETRY_LEN {
            return None;
        }
        Some(f64::from_le_bytes(self.telemetry[o..end].try_into().ok()?))
    }
}

// ============================================================================
// 拒绝码（快照输入不可得时的结构化理由；服务端应答 [TEL_RESP_ERR, code]）
// ============================================================================

/// 无已冻结快照（冷启动/窗口未到/事件面未打开）——绝不回退评估器伪造。
pub const SNAP_REJECT_NO_FROZEN: u8 = 1;
/// 快照不完整（缺必需点）。
pub const SNAP_REJECT_INCOMPLETE: u8 = 2;
/// 快照过期（produced_at 超过输入新鲜度窗）。
pub const SNAP_REJECT_STALE: u8 = 3;
/// 保留背压（消费者落后导致删段受阻；不得继续发新决策）。
pub const SNAP_REJECT_BACKPRESSURE: u8 = 4;

/// 编码拒绝应答 `[TEL_RESP_ERR, code]`。
pub fn encode_reject(code: u8) -> Vec<u8> {
    alloc::vec![crate::tel_wire::TEL_RESP_ERR, code]
}

/// 拒绝原因词形（判定行/证据采集单源）。
pub fn reject_word(code: u8) -> &'static str {
    match code {
        SNAP_REJECT_NO_FROZEN => "no-frozen-snapshot",
        SNAP_REJECT_INCOMPLETE => "snapshot-incomplete",
        SNAP_REJECT_STALE => "snapshot-stale",
        SNAP_REJECT_BACKPRESSURE => "retention-backpressure",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> SnapshotFacts {
        SnapshotFacts {
            snapshot_id: [7u8; 16],
            provenance_root: [9u8; 32],
            cut_time_ns: 1_000_000,
            produced_at_ns: 2_000_000,
            catalog_version: 3,
            catalog_digest8: [1, 2, 3, 4, 5, 6, 7, 8],
            accepted: 7,
            missing: 0,
            completeness: COMPLETE,
        }
    }

    #[test]
    fn wire_len_frozen() {
        assert_eq!(SNAP_INPUT_FACTS_LEN, 90);
        assert_eq!(SNAP_INPUT_WIRE_LEN, 234);
        assert_eq!(SNAP_INPUT_WIRE_LEN, SNAP_INPUT_FACTS_LEN + 144);
    }

    #[test]
    fn roundtrip_preserves_identity_and_payload() {
        let mut tel = [0u8; 144];
        tel[0..8].copy_from_slice(&400.0f64.to_le_bytes());
        tel[24..32].copy_from_slice(&0.55f64.to_le_bytes());
        let input = TrustedInput::new(facts(), tel);
        let wire = input.encode();
        assert_eq!(wire.len(), SNAP_INPUT_WIRE_LEN);
        let back = TrustedInput::decode(&wire).expect("decode");
        assert_eq!(back, input);
        assert_eq!(back.snapshot_id(), &[7u8; 16]);
        assert_eq!(back.catalog_version(), 3);
        assert_eq!(back.telemetry_f64(TELEMETRY_OFF_V), Some(400.0));
        assert_eq!(back.telemetry_f64(TELEMETRY_OFF_SOC), Some(0.55));
        assert!(back.facts().is_complete());
    }

    #[test]
    fn telemetry_f64_offsets_are_total() {
        let mut tel = [0u8; 144];
        tel[0..8].copy_from_slice(&1.5f64.to_le_bytes());
        tel[SNAP_INPUT_TELEMETRY_LEN - 8..].copy_from_slice(&2.5f64.to_le_bytes());
        let input = TrustedInput::new(facts(), tel);
        // 最小合法偏移 = 载荷起点。
        assert_eq!(input.telemetry_f64(SNAP_INPUT_FACTS_LEN), Some(1.5));
        // 最后合法偏移 = 载荷尾 − 8。
        let last = SNAP_INPUT_FACTS_LEN + SNAP_INPUT_TELEMETRY_LEN - 8;
        assert_eq!(input.telemetry_f64(last), Some(2.5));
        // 一字节越界（窗口不完整）。
        assert_eq!(input.telemetry_f64(last + 1), None);
        // 载荷起点之下：不得下溢（debug panic / release 回绕）。
        assert_eq!(input.telemetry_f64(SNAP_INPUT_FACTS_LEN - 1), None);
        assert_eq!(input.telemetry_f64(0), None);
        // usize::MAX：checked 路径返回 None。
        assert_eq!(input.telemetry_f64(usize::MAX), None);
    }

    #[test]
    fn decode_rejects_truncated_magic_flags_and_unknown_completeness() {
        let wire = TrustedInput::new(facts(), [0u8; 144]).encode();
        assert!(TrustedInput::decode(&wire[..wire.len() - 1]).is_none());
        assert!(TrustedInput::decode(&[]).is_none());
        let mut bad_magic = wire.clone();
        bad_magic[0] = b'X';
        assert!(TrustedInput::decode(&bad_magic).is_none());
        let mut bad_flags = wire.clone();
        bad_flags[89] = 0x02;
        assert!(TrustedInput::decode(&bad_flags).is_none());
        let mut bad_code = wire.clone();
        bad_code[88] = 9;
        assert!(TrustedInput::decode(&bad_code).is_none());
    }

    #[test]
    fn unbound_and_incomplete_detection() {
        let mut f = facts();
        f.snapshot_id = [0u8; 16];
        assert!(f.is_unbound());
        assert!(!f.is_complete());
        let mut f2 = facts();
        f2.missing = 1;
        f2.completeness = INCOMPLETE;
        assert!(!f2.is_complete());
        assert!(!f2.is_unbound());
        assert!(facts().is_complete());
    }

    #[test]
    fn reject_codes_and_words_are_frozen() {
        assert_eq!(SNAP_REJECT_NO_FROZEN, 1);
        assert_eq!(SNAP_REJECT_INCOMPLETE, 2);
        assert_eq!(SNAP_REJECT_STALE, 3);
        assert_eq!(SNAP_REJECT_BACKPRESSURE, 4);
        assert_eq!(encode_reject(SNAP_REJECT_STALE), alloc::vec![0xFF, 3]);
        assert_eq!(
            reject_word(SNAP_REJECT_BACKPRESSURE),
            "retention-backpressure"
        );
    }
}
