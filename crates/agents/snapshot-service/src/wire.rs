//! 快照规范编码（[02] §5：跨 binary 必须编码为版本化分页或 BlobRef，
//! 不传 Rust 布局）。V1 = 定长小端、保留位清零；分页承载由传输层
//! （agent-sdk chunked）负责——本模块只做字节 ↔ 快照的规范往返。
//!
//! 布局：`magic "SNAP" u32 ‖ wire_version u16 ‖ header_len u16 ‖
//! snapshot_id 16 ‖ version_set 6×u64 ‖ cut u64 ‖ produced u64 ‖
//! policy u64 ‖ tenant u32 ‖ site u32 ‖ obs_count u16 ‖ wm_count u16 ‖
//! conflict_count u16 ‖ derived_count u16 ‖ 每观测 128B（ObservationEvent
//! V1 帧）‖ 每水位 36B ‖ 每冲突 22B+4B×源 ‖ 每派生 20B ‖ completeness
//! 计数块 ‖ provenance_root 32 ‖ payload_sha256 32`。保留位全零；解码
//! 拒绝任何未知版本/长度/保留位（fail-closed）。身份（tenant/site）入
//! 头——解码不得编造默认身份。

use alloc::vec::Vec;

use eneros_telemetry_model::event::ObservationEvent;

use crate::{
    CompletenessReport, Conflict, ConflictDecision, DerivedValue, NormalizedObservation,
    SourceWatermark, TrustedSnapshot, VersionSet,
};

pub const SNAPSHOT_MAGIC: [u8; 4] = *b"SNAP";
pub const SNAPSHOT_WIRE_V1: u16 = 1;

/// 编码（确定性：同快照同字节）。
pub fn encode_snapshot_v1(s: &TrustedSnapshot) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&SNAPSHOT_MAGIC);
    out.extend_from_slice(&SNAPSHOT_WIRE_V1.to_le_bytes());
    // header（定长 112B：4+2+2 头 + 16 + 48 + 8+8+8 + 4+4 + 2×4）。
    out.extend_from_slice(&(112u16).to_le_bytes());
    out.extend_from_slice(&s.snapshot_id);
    let vs = s.version_set;
    for v in [
        vs.generation,
        vs.mapping_version,
        vs.topology_version,
        vs.asset_version,
        vs.contract_version,
        vs.safety_version,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&s.cut_time_ns.to_le_bytes());
    out.extend_from_slice(&s.produced_at_ns.to_le_bytes());
    out.extend_from_slice(&s.policy_version.to_le_bytes());
    // 身份字段（tenant/site）——解码侧不得编造。
    out.extend_from_slice(&s.tenant_id.to_le_bytes());
    out.extend_from_slice(&s.site_id.to_le_bytes());
    out.extend_from_slice(&(s.observations.len() as u16).to_le_bytes());
    out.extend_from_slice(&(s.source_watermarks.len() as u16).to_le_bytes());
    out.extend_from_slice(&(s.conflicts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(s.derived.len() as u16).to_le_bytes());
    // 观测（128B 帧）。
    for o in s.observations.iter() {
        out.extend_from_slice(&o.event.encode_v1());
    }
    // 水位（source u32 + max_event u64 + max_ingest u64 + epoch u64 + seq u64 = 40B）。
    for w in s.source_watermarks.iter() {
        out.extend_from_slice(&w.source_id.to_le_bytes());
        out.extend_from_slice(&w.max_event_time_ns.to_le_bytes());
        out.extend_from_slice(&w.max_ingest_time_ns.to_le_bytes());
        out.extend_from_slice(&w.epoch.to_le_bytes());
        out.extend_from_slice(&w.sequence.to_le_bytes());
    }
    // 冲突（point u32 + decision u8 + pad u8 + source u32 + sequence u64 + sources_count u16 + pad u16 = 28B + 每源 4B）。
    for c in s.conflicts.iter() {
        out.extend_from_slice(&c.point_id.to_le_bytes());
        match c.decision {
            ConflictDecision::Accepted { source, sequence } => {
                out.push(1);
                out.push(0);
                out.extend_from_slice(&source.to_le_bytes());
                out.extend_from_slice(&sequence.to_le_bytes());
            }
            ConflictDecision::Unresolved => {
                out.push(0);
                out.push(0);
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            }
        }
        out.extend_from_slice(&(c.sources.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for src in c.sources.iter() {
            out.extend_from_slice(&src.to_le_bytes());
        }
    }
    // 派生（point u32 + formula u32 + fver u16 + unit u16 + value f64 = 24B）。
    for d in s.derived.iter() {
        out.extend_from_slice(&d.point_id.to_le_bytes());
        out.extend_from_slice(&d.formula_id.to_le_bytes());
        out.extend_from_slice(&d.formula_version.to_le_bytes());
        out.extend_from_slice(&d.unit_code.to_le_bytes());
        out.extend_from_slice(&d.value.to_le_bytes());
    }
    // 完整度计数块（7 × u16 + 各键列表 u32）。
    encode_completeness(&mut out, &s.completeness);
    out.extend_from_slice(&s.provenance_root);
    // payload SHA-256（篡改检出）。
    {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(&out);
        let d = h.finalize();
        out.extend_from_slice(&d);
    }
    out
}

fn encode_completeness(out: &mut Vec<u8>, c: &CompletenessReport) {
    for list in [
        &c.required,
        &c.accepted,
        &c.missing,
        &c.stale,
        &c.bad,
        &c.conflict,
        &c.substituted,
    ] {
        out.extend_from_slice(&(list.len() as u16).to_le_bytes());
        for p in list {
            out.extend_from_slice(&p.to_le_bytes());
        }
    }
}

/// 解码游标（严格；截断/越界 → 错误）。
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], alloc::string::String> {
        if self.pos + n > self.buf.len() {
            return Err(alloc::format!("truncated:{what}"));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
}

/// 解码（严格）；失败返回 `Some(错误字符串)` 语义——调用方 fail-closed。
pub fn decode_snapshot_v1(buf: &[u8]) -> Result<TrustedSnapshot, alloc::string::String> {
    let mut cur = Cursor::new(buf);
    // take 是闭包借用 cur；改用直接方法调用
    macro_rules! take {
        ($n:expr, $what:expr) => {
            cur.take($n, $what)?
        };
    }
    let magic = take!(4, "magic");
    if magic != SNAPSHOT_MAGIC {
        return Err("bad-magic".into());
    }
    let version = u16::from_le_bytes(take!(2, "version").try_into().unwrap());
    if version != SNAPSHOT_WIRE_V1 {
        return Err(alloc::format!("bad-version:{version}"));
    }
    let header_len = u16::from_le_bytes(take!(2, "header_len").try_into().unwrap());
    if header_len != 112 {
        return Err(alloc::format!("bad-header-len:{header_len}"));
    }
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(take!(16, "snapshot_id"));
    let mut vs = VersionSet::ZERO;
    for slot in [
        &mut vs.generation,
        &mut vs.mapping_version,
        &mut vs.topology_version,
        &mut vs.asset_version,
        &mut vs.contract_version,
        &mut vs.safety_version,
    ] {
        *slot = u64::from_le_bytes(take!(8, "version").try_into().unwrap());
    }
    let cut_time_ns = u64::from_le_bytes(take!(8, "cut").try_into().unwrap());
    let produced_at_ns = u64::from_le_bytes(take!(8, "produced").try_into().unwrap());
    let policy_version = u64::from_le_bytes(take!(8, "policy").try_into().unwrap());
    let tenant_id = u32::from_le_bytes(take!(4, "tenant").try_into().unwrap());
    let site_id = u32::from_le_bytes(take!(4, "site").try_into().unwrap());
    let obs_count = u16::from_le_bytes(take!(2, "obs_count").try_into().unwrap());
    let wm_count = u16::from_le_bytes(take!(2, "wm_count").try_into().unwrap());
    let conflict_count = u16::from_le_bytes(take!(2, "conflict_count").try_into().unwrap());
    let derived_count = u16::from_le_bytes(take!(2, "derived_count").try_into().unwrap());
    let mut observations = Vec::with_capacity(obs_count as usize);
    for _ in 0..obs_count {
        let frame = take!(128, "obs");
        let ev =
            ObservationEvent::decode_v1(frame).map_err(|e| alloc::format!("obs-decode:{e:?}"))?;
        observations.push(NormalizedObservation { event: ev });
    }
    let mut source_watermarks = Vec::with_capacity(wm_count as usize);
    for _ in 0..wm_count {
        let source_id = u32::from_le_bytes(take!(4, "wm").try_into().unwrap());
        let max_event_time_ns = u64::from_le_bytes(take!(8, "wm").try_into().unwrap());
        let max_ingest_time_ns = u64::from_le_bytes(take!(8, "wm").try_into().unwrap());
        let epoch = u64::from_le_bytes(take!(8, "wm").try_into().unwrap());
        let sequence = u64::from_le_bytes(take!(8, "wm").try_into().unwrap());
        source_watermarks.push(SourceWatermark {
            source_id,
            max_event_time_ns,
            max_ingest_time_ns,
            epoch,
            sequence,
        });
    }
    let mut conflicts = Vec::with_capacity(conflict_count as usize);
    for _ in 0..conflict_count {
        let point_id = u32::from_le_bytes(take!(4, "conflict").try_into().unwrap());
        let decision_code = take!(2, "conflict")[0];
        let _pad = take!(0, "");
        let source = u32::from_le_bytes(take!(4, "conflict").try_into().unwrap());
        let sequence = u64::from_le_bytes(take!(8, "conflict").try_into().unwrap());
        let sources_count = u16::from_le_bytes(take!(2, "conflict").try_into().unwrap());
        let _pad2 = take!(2, "conflict");
        let mut sources = Vec::with_capacity(sources_count as usize);
        for _ in 0..sources_count {
            sources.push(u32::from_le_bytes(take!(4, "conflict").try_into().unwrap()));
        }
        let decision = match decision_code {
            1 => ConflictDecision::Accepted { source, sequence },
            0 => ConflictDecision::Unresolved,
            other => return Err(alloc::format!("bad-decision:{other}")),
        };
        conflicts.push(Conflict {
            point_id,
            sources,
            decision,
            rule: "decoded",
        });
    }
    let mut derived = Vec::with_capacity(derived_count as usize);
    for _ in 0..derived_count {
        let point_id = u32::from_le_bytes(take!(4, "derived").try_into().unwrap());
        let formula_id = u32::from_le_bytes(take!(4, "derived").try_into().unwrap());
        let formula_version = u16::from_le_bytes(take!(2, "derived").try_into().unwrap());
        let unit_code = u16::from_le_bytes(take!(2, "derived").try_into().unwrap());
        let value = f64::from_le_bytes(take!(8, "derived").try_into().unwrap());
        if !value.is_finite() {
            return Err("derived-non-finite".into());
        }
        derived.push(DerivedValue {
            point_id,
            formula_id,
            formula_version,
            value,
            unit_code,
        });
    }
    let completeness = decode_completeness(&mut cur)?;
    let mut provenance_root = [0u8; 32];
    provenance_root.copy_from_slice(take!(32, "root"));
    let payload_sha = take!(32, "sha256");
    if cur.pos != buf.len() {
        return Err("trailing-bytes".into());
    }
    // payload 摘要复验（覆盖全部前置字节）。
    {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(&buf[..buf.len() - 32]);
        if h.finalize().as_slice() != payload_sha {
            return Err("sha256-mismatch".into());
        }
    }
    Ok(TrustedSnapshot {
        snapshot_id,
        tenant_id,
        site_id,
        cut_time_ns,
        produced_at_ns,
        policy_version,
        version_set: vs,
        observations,
        source_watermarks,
        completeness,
        conflicts,
        derived,
        provenance_root,
    })
}

fn decode_completeness(cur: &mut Cursor<'_>) -> Result<CompletenessReport, alloc::string::String> {
    let mut lists: Vec<Vec<u32>> = Vec::with_capacity(7);
    for _ in 0..7 {
        let n = u16::from_le_bytes(cur.take(2, "completeness")?.try_into().unwrap());
        let mut items = Vec::with_capacity(n as usize);
        for _ in 0..n {
            items.push(u32::from_le_bytes(
                cur.take(4, "completeness")?.try_into().unwrap(),
            ));
        }
        lists.push(items);
    }
    let mut it = lists.into_iter();
    Ok(CompletenessReport {
        required: it.next().unwrap(),
        accepted: it.next().unwrap(),
        missing: it.next().unwrap(),
        stale: it.next().unwrap(),
        bad: it.next().unwrap(),
        conflict: it.next().unwrap(),
        substituted: it.next().unwrap(),
    })
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;

    use eneros_semantic_catalog::SemanticCatalog;

    use super::*;
    use crate::tests::{obs7, prod_catalog, test_policy};
    use crate::{
        conflict::{ConflictResolver, ConflictRule},
        policy::SnapshotPolicyRegistry,
        ProjectorCapacity, SnapshotModule, SnapshotRequest,
    };

    #[test]
    fn encode_decode_roundtrip() {
        let catalog = prod_catalog();
        let vs = catalog.version_set();
        let mut policies = SnapshotPolicyRegistry::new();
        policies.register(test_policy(vs.contract_version));
        let mut m = SnapshotModule::new(
            Box::new(catalog),
            ProjectorCapacity::default(),
            policies,
            ConflictResolver::new(ConflictRule::PrimaryBackup),
            10_000,
        );
        let now = 3_000_000_000u64;
        m.apply_events(&obs7(now, 100, 2, &[400.0, 50.0, 50.0, 0.6, 1.0, 0.0, 0.0]));
        let snap = m
            .build(
                SnapshotRequest {
                    tenant_id: 1,
                    site_id: 1,
                    purpose_id: 1,
                    as_of_ns: now,
                    max_wait_ns: 0,
                    policy_version: 1,
                },
                now,
            )
            .unwrap();
        let bytes = encode_snapshot_v1(&snap);
        let dec = decode_snapshot_v1(&bytes).unwrap();
        assert_eq!(dec.snapshot_id, snap.snapshot_id);
        assert_eq!(dec.provenance_root, snap.provenance_root);
        assert_eq!(dec.observations.len(), 7);
        assert_eq!(dec.observations[0].event.point_id, 1);
        assert_eq!(dec.completeness, snap.completeness);
        // 身份字段往返（不得编造默认租户/站点）。
        let mut tagged = snap.clone();
        tagged.tenant_id = 7;
        tagged.site_id = 9;
        let dec2 = decode_snapshot_v1(&encode_snapshot_v1(&tagged)).unwrap();
        assert_eq!(dec2.tenant_id, 7);
        assert_eq!(dec2.site_id, 9);
        // 篡改：翻转 payload 一字节 → 摘要失败。
        let mut evil = bytes.clone();
        let mid = evil.len() / 2;
        evil[mid] ^= 0x01;
        assert!(decode_snapshot_v1(&evil).is_err());
    }
}
