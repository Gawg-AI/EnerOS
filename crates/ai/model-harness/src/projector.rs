//! 输入投影（[11] §3.4）：模型不读取完整 TrustedSnapshot 或任意日志。
//! `InputProjector` 按 skill schema 只提取已被允许的 normalized 字段、
//! event/snapshot references、单位/有效期/不确定度、数据分类与脱敏
//! 结果；外部文档和市场文本始终标为 untrusted（不能改变 system/tool
//! policy）。Restricted 数据默认拒绝远端模型。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::ids::SnapshotId;
use eneros_snapshot_service::TrustedSnapshot;
use sha2::{Digest, Sha256};

use crate::error::HarnessError;

/// 投影规格（来自 skill 的 input_schema）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionSpec {
    /// 允许的点位 id（空 = 不投影任何观测）。
    pub allowed_point_ids: Vec<u32>,
    /// 允许的 schema id（空 = 不限）。
    pub allowed_schemas: Vec<u32>,
    pub max_fields: usize,
    /// 快照最大年龄（ns；0 = 不限制）。
    pub max_age_ns: u64,
    /// 是否携带外部文本区（untrusted）。
    pub include_untrusted: bool,
    /// 本任务请求的数据分类（Classification 码；≤ manifest data_class_max）。
    pub requested_class: u8,
    /// 最大 untrusted 段数与单段字节。
    pub untrusted_max_sections: usize,
    pub untrusted_max_bytes: usize,
}

impl Default for ProjectionSpec {
    fn default() -> Self {
        Self {
            allowed_point_ids: Vec::new(),
            allowed_schemas: Vec::new(),
            max_fields: 64,
            max_age_ns: 0,
            include_untrusted: false,
            requested_class: 1,
            untrusted_max_sections: 4,
            untrusted_max_bytes: 2048,
        }
    }
}

/// 投影字段（normalized 单值）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectedField {
    pub point_id: u32,
    pub asset_id: u64,
    pub device_id: u16,
    pub value: f64,
    pub unit_code: u16,
    pub quality_ok: bool,
    pub time_ns: u64,
}

/// 不可信内容区（外部文档/市场文本；不得改变 policy）。
#[derive(Debug, Clone, PartialEq)]
pub struct UntrustedSection {
    pub kind: u8,
    pub text: String,
}

impl UntrustedSection {
    pub const KIND_MARKET_TEXT: u8 = 1;
    pub const KIND_EXTERNAL_DOC: u8 = 2;
}

/// 投影结果（含摘要——prompt digest 的组成部分）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedInput {
    pub snapshot_ref: SnapshotId,
    pub tenant_id: u32,
    pub site_id: u32,
    pub cut_time_ns: u64,
    pub fields: Vec<ProjectedField>,
    pub data_class: u8,
    pub untrusted: Vec<UntrustedSection>,
    /// canonical 投影摘要（SHA-256，覆盖字段与 untrusted 区）。
    pub digest: [u8; 32],
}

/// 投影器（[11] §3.4：只提取已允许 normalized 字段）。
pub struct InputProjector;

/// 观测值 → f64（Bytes16 不可数值化 → None；fail-closed）。
fn value_to_f64(v: &eneros_telemetry_model::event::ObservationValue) -> Option<f64> {
    match v {
        eneros_telemetry_model::event::ObservationValue::F64(x) => Some(*x),
        eneros_telemetry_model::event::ObservationValue::I64(x) => Some(*x as f64),
        eneros_telemetry_model::event::ObservationValue::U64(x) => Some(*x as f64),
        eneros_telemetry_model::event::ObservationValue::Bool(b) => Some(u8::from(*b) as f64),
        eneros_telemetry_model::event::ObservationValue::Enum(x) => Some(*x as f64),
        eneros_telemetry_model::event::ObservationValue::Bytes16(_) => None,
    }
}

impl InputProjector {
    /// 投影（fail-closed）：分类越权/未知单位/非有限值/为空 → 拒绝。
    pub fn project(
        snapshot: &TrustedSnapshot,
        spec: &ProjectionSpec,
        data_class_max: u8,
        remote: bool,
        untrusted: &[UntrustedSection],
        now_ns: u64,
    ) -> Result<ProjectedInput, HarnessError> {
        if spec.requested_class > data_class_max {
            return Err(HarnessError::Projection(
                "requested_class > manifest data_class_max",
            ));
        }
        if remote && spec.requested_class >= crate::DATA_CLASS_MAX_RESTRICTED {
            return Err(HarnessError::Projection(
                "Restricted data refused for remote model",
            ));
        }
        if spec.max_age_ns != 0 && now_ns.saturating_sub(snapshot.cut_time_ns) > spec.max_age_ns {
            return Err(HarnessError::Projection("snapshot stale"));
        }
        let mut fields: Vec<ProjectedField> = Vec::new();
        for obs in &snapshot.observations {
            let ev = &obs.event;
            if !spec.allowed_point_ids.is_empty() && !spec.allowed_point_ids.contains(&ev.point_id)
            {
                continue;
            }
            if !spec.allowed_schemas.is_empty() && !spec.allowed_schemas.contains(&ev.schema_id) {
                continue;
            }
            if ev.unit_code == 0 {
                // 未知单位不得进模型（与「不得进控制快照」同纪律）。
                continue;
            }
            let value = match value_to_f64(&ev.value) {
                Some(v) if v.is_finite() => v,
                _ => return Err(HarnessError::Projection("non-finite value")),
            };
            fields.push(ProjectedField {
                point_id: ev.point_id,
                asset_id: ev.asset_id,
                device_id: ev.device_id,
                value,
                unit_code: ev.unit_code,
                quality_ok: ev.quality_bits & eneros_telemetry_model::event::quality_bits::VALID
                    != 0,
                time_ns: ev.event_time_ns,
            });
            if fields.len() >= spec.max_fields {
                break;
            }
        }
        // 空投影（无数据给模型）→ 拒绝——避免模型在无依据时编造。
        if fields.is_empty() {
            return Err(HarnessError::Projection("no allowed fields"));
        }
        let mut untrusted_out: Vec<UntrustedSection> = Vec::new();
        if spec.include_untrusted {
            let mut total = 0usize;
            for u in untrusted.iter().take(spec.untrusted_max_sections) {
                total += u.text.len();
                if total > spec.untrusted_max_bytes || u.text.is_empty() {
                    return Err(HarnessError::Projection("untrusted overflow"));
                }
                untrusted_out.push(u.clone());
            }
        }
        let data_class = spec.requested_class;
        let p = ProjectedInput {
            snapshot_ref: SnapshotId(snapshot.snapshot_id),
            tenant_id: snapshot.tenant_id,
            site_id: snapshot.site_id,
            cut_time_ns: snapshot.cut_time_ns,
            fields,
            data_class,
            untrusted: untrusted_out,
            digest: [0; 32],
        };
        let mut p = p;
        p.digest = p.compute_digest();
        Ok(p)
    }
}

impl ProjectedInput {
    /// canonical 字节（字段序 + untrusted 区；与摘要算法同序）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(self.fields.len() * 40 + 64);
        b.extend_from_slice(&self.snapshot_ref.0);
        b.extend_from_slice(&self.tenant_id.to_le_bytes());
        b.extend_from_slice(&self.site_id.to_le_bytes());
        b.extend_from_slice(&self.cut_time_ns.to_le_bytes());
        b.push(self.data_class);
        for f in &self.fields {
            b.extend_from_slice(&f.point_id.to_le_bytes());
            b.extend_from_slice(&f.asset_id.to_le_bytes());
            b.extend_from_slice(&f.value.to_le_bytes());
            b.extend_from_slice(&f.unit_code.to_le_bytes());
            b.push(f.quality_ok as u8);
        }
        for u in &self.untrusted {
            b.push(u.kind);
            b.extend_from_slice(u.text.as_bytes());
            b.push(0xFF); // 段分隔（文本内控制字符已脱敏）
        }
        b
    }

    pub fn compute_digest(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.canonical_bytes());
        h.finalize().into()
    }

    pub fn is_valid_at(&self, now_ns: u64) -> bool {
        now_ns.saturating_sub(self.cut_time_ns) < 86_400_000_000_000
    }
}

impl core::fmt::Display for ProjectedField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "point={} asset={} value={} unit={} q={}",
            self.point_id, self.asset_id, self.value, self.unit_code, self.quality_ok
        )
    }
}

/// 投影 → 模型可见文本（指令区 + 数据区 + 不可信区分离，[11] §3.4）。
pub fn render_projection(p: &ProjectedInput) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "snapshot={} tenant={} site={} cut_ns={}\n",
        hex16(&p.snapshot_ref.0),
        p.tenant_id,
        p.site_id,
        p.cut_time_ns
    ));
    s.push_str("fields:\n");
    for f in &p.fields {
        s.push_str(&format!("  {f}\n"));
        if s.len() > 4096 {
            s.push_str("  ...(truncated)\n");
            break;
        }
    }
    if !p.untrusted.is_empty() {
        s.push_str("untrusted:\n");
        for u in &p.untrusted {
            s.push_str(&format!("  [{}] {}\n", u.kind, u.text));
        }
    }
    s
}

pub fn hex16(b: &[u8; 16]) -> String {
    use alloc::fmt::Write;
    let mut s = String::new();
    for byte in b {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use eneros_decision_contract::ids::SnapshotId;
    use eneros_snapshot_service::TrustedSnapshot;
    use eneros_telemetry_model::event::{ObservationEvent, ObservationValue};

    use super::*;

    fn ev(point: u32, unit: u16, value: f64) -> ObservationEvent {
        let mut e = ObservationEvent {
            tenant_id: 1,
            site_id: 1,
            source_id: 5,
            source_epoch: 1,
            sequence: point as u64,
            event_time_ns: 1000,
            ingest_time_ns: 1000,
            asset_id: 100 + point as u64,
            device_id: 0,
            unit_code: unit,
            point_id: point,
            schema_id: 1,
            schema_version: 1,
            quality_bits: eneros_telemetry_model::event::quality_bits::VALID,
            mapping_version: 1,
            value: ObservationValue::F64(value),
            integrity: eneros_telemetry_model::event::IntegrityTag::Authenticated,
            clock_id: 3,
            flags: 0,
        };
        // 校验：epoch 非零无 TEST 也可（VALID），TIME_MISSING 规则。
        e.flags = 0;
        e
    }

    fn snapshot() -> TrustedSnapshot {
        TrustedSnapshot {
            snapshot_id: [9; 16],
            tenant_id: 1,
            site_id: 1,
            cut_time_ns: 1000,
            produced_at_ns: 1001,
            policy_version: 1,
            version_set: eneros_semantic_catalog::VersionSet {
                generation: 1,
                mapping_version: 1,
                topology_version: 1,
                asset_version: 1,
                contract_version: 1,
                safety_version: 1,
            },
            observations: vec![
                eneros_snapshot_service::NormalizedObservation {
                    event: ev(1, 30, 50.0),
                },
                eneros_snapshot_service::NormalizedObservation {
                    event: ev(2, 30, 0.8),
                },
                eneros_snapshot_service::NormalizedObservation {
                    event: ev(3, 0, 99.0),
                },
            ],
            source_watermarks: vec![],
            completeness: eneros_snapshot_service::CompletenessReport {
                required: vec![],
                accepted: vec![],
                missing: vec![],
                stale: vec![],
                bad: vec![],
                conflict: vec![],
                substituted: vec![],
            },
            conflicts: vec![],
            derived: vec![],
            provenance_root: [0; 32],
        }
    }

    #[test]
    fn projects_allowed_fields_only() {
        let spec = ProjectionSpec {
            allowed_point_ids: vec![1, 2],
            ..Default::default()
        };
        let p = InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000).unwrap();
        assert_eq!(p.fields.len(), 2);
        assert_eq!(p.fields[0].point_id, 1);
        assert_eq!(p.fields[1].value, 0.8);
        // 未知单位（unit=0）被跳过。
        assert!(p.fields.iter().all(|f| f.point_id != 3));
        assert!(p.is_valid_at(2000));
    }

    #[test]
    fn classification_guards() {
        let spec = ProjectionSpec {
            requested_class: 3,
            ..Default::default()
        };
        // 远端模型 + Restricted → 拒。
        assert_eq!(
            InputProjector::project(&snapshot(), &spec, 3, true, &[], 2000),
            Err(HarnessError::Projection(
                "Restricted data refused for remote model"
            ))
        );
        // 请求分类超 manifest 上限 → 拒。
        let spec = ProjectionSpec {
            requested_class: 3,
            ..Default::default()
        };
        assert_eq!(
            InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000),
            Err(HarnessError::Projection(
                "requested_class > manifest data_class_max"
            ))
        );
    }

    #[test]
    fn stale_snapshot_rejected() {
        let spec = ProjectionSpec {
            max_age_ns: 500,
            ..Default::default()
        };
        assert_eq!(
            InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000),
            Err(HarnessError::Projection("snapshot stale"))
        );
    }

    #[test]
    fn empty_projection_rejected() {
        let spec = ProjectionSpec {
            allowed_point_ids: vec![99],
            ..Default::default()
        };
        assert_eq!(
            InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000),
            Err(HarnessError::Projection("no allowed fields"))
        );
    }

    #[test]
    fn untrusted_sections_bounded() {
        let spec = ProjectionSpec {
            include_untrusted: true,
            untrusted_max_sections: 2,
            untrusted_max_bytes: 64,
            ..Default::default()
        };
        let u = [
            UntrustedSection {
                kind: 1,
                text: "市场文本 1".into(),
            },
            UntrustedSection {
                kind: 2,
                text: "x".repeat(100),
            },
            UntrustedSection {
                kind: 1,
                text: "溢出段".into(),
            },
        ];
        // 第 2 段超字节上限 → 整投影拒绝（fail-closed，不截断）。
        assert_eq!(
            InputProjector::project(&snapshot(), &spec, 2, false, &u, 2000),
            Err(HarnessError::Projection("untrusted overflow"))
        );
        // 全部在界内 → 通过且按上限取段。
        let u_ok = [
            UntrustedSection {
                kind: 1,
                text: "短文本".into(),
            },
            UntrustedSection {
                kind: 2,
                text: "再一段".into(),
            },
            UntrustedSection {
                kind: 1,
                text: "第三段".into(),
            },
        ];
        let p = InputProjector::project(&snapshot(), &spec, 2, false, &u_ok, 2000).unwrap();
        assert_eq!(p.untrusted.len(), 2, "取前 untrusted_max_sections 段");
    }

    #[test]
    fn digest_stable_and_sensitive() {
        let spec = ProjectionSpec::default();
        let p1 = InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000).unwrap();
        let p2 = InputProjector::project(&snapshot(), &spec, 2, false, &[], 2000).unwrap();
        assert_eq!(p1.digest, p2.digest);
        // 任一字段值变化 → 摘要变化（prompt/证据绑定面）。
        let mut snap = snapshot();
        snap.observations[0].event.value = ObservationValue::F64(51.0);
        let p3 = InputProjector::project(&snap, &spec, 2, false, &[], 2000).unwrap();
        assert_ne!(p1.digest, p3.digest);
    }

    #[test]
    fn render_separates_untrusted() {
        let spec = ProjectionSpec {
            include_untrusted: true,
            ..Default::default()
        };
        let p = InputProjector::project(
            &snapshot(),
            &spec,
            2,
            false,
            &[UntrustedSection {
                kind: 1,
                text: "文本".into(),
            }],
            2000,
        )
        .unwrap();
        let r = render_projection(&p);
        assert!(r.contains("untrusted:"));
        assert!(r.contains("文本"));
        assert!(r.starts_with("snapshot="));
        let _ = SnapshotId([0; 16]);
    }
}
