//! 输出门（[11] §3.5 八步固定顺序）。任何一步失败 → 结构化拒绝；
//! 绝不 fallback 到未认证默认命令。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::ids::{ReasonCode, SnapshotId};
use eneros_decision_contract::objective::ObjectiveKind;
use eneros_decision_contract::IntentCandidate;

use crate::drafts::{Explanation, ObjectiveDraft};
use crate::error::GateStage;
use crate::schema::{SchemaType, StrictSchema};

/// 输出字节上限（门第 1 步；超长拒绝）。
pub const MAX_OUTPUT_BYTES: usize = 4096;
/// 文本字段长度上限。
pub const TEXT_FIELD_MAX: usize = 512;

/// 门上下文（调用方已验的内容：模型/提示摘要、快照引用、证据引用表、
/// 审批要求）。
#[derive(Debug, Clone)]
pub struct GateContext<'a> {
    pub model_digest: &'a [u8; 32],
    pub prompt_digest: &'a [u8; 32],
    pub snapshot_id: SnapshotId,
    /// 证据引用存在表（event/asset id → 版本；语义层由投影提供）。
    pub refs: &'a [(u8, [u8; 16], u64)],
    pub now_ms: u64,
    /// skill 要求人工审批 → 候选冻结待批（门不代批）。
    pub requires_human_approval: bool,
}

/// 门拒绝（携带步骤与原因；一切拒绝可观测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReject {
    pub stage: GateStage,
    pub reason: &'static str,
}

impl GateReject {
    fn new(stage: GateStage, reason: &'static str) -> Self {
        Self { stage, reason }
    }
}

/// 通过的候选产物（[11] §2 顺序链：IntentCandidate/RuleDraft/
/// Explanation；本卷交付 IntentCandidate + Explanation + ObjectiveDraft；
/// 绝不生成 AuthorizedCommand——类型上没有该形状）。
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateOutput {
    Intent(IntentCandidate),
    Explanation(Explanation),
    ObjectiveDraft(ObjectiveDraft),
}

/// 输出门（无状态）。
pub struct OutputGate;

/// schema 定义（名称/枚举稳定——与策略/目录绑定，改动走目录版本）。
pub const SCHEMA_INTENT: StrictSchema = StrictSchema::new(&[
    crate::schema::SchemaField {
        name: "kind",
        ty: SchemaType::String,
        required: true,
        enum_values: &["MaxRevenue", "MinCost", "PeakShave", "SelfConsume"],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "reason",
        ty: SchemaType::String,
        required: true,
        enum_values: &[
            "PriceArbitrage",
            "PeakShave",
            "ValleyFill",
            "RenewableCurtail",
            "GridSupport",
            "SocBalance",
            "ManualOperator",
            "ContractObligation",
            "EmergencyLimit",
            "ModelRecommendation",
        ],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "confidence_bp",
        ty: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(0.0),
        maximum: Some(10_000.0),
    },
    crate::schema::SchemaField {
        name: "valid_until_ms",
        ty: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(1.0),
        maximum: None,
    },
]);

pub const SCHEMA_EXPLANATION: StrictSchema = StrictSchema::new(&[
    crate::schema::SchemaField {
        name: "severity",
        ty: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(1.0),
        maximum: Some(3.0),
    },
    crate::schema::SchemaField {
        name: "event_refs",
        ty: SchemaType::Array,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "text",
        ty: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
]);

pub const SCHEMA_OBJECTIVE_DRAFT: StrictSchema = StrictSchema::new(&[
    crate::schema::SchemaField {
        name: "kind",
        ty: SchemaType::String,
        required: true,
        enum_values: &["MaxRevenue", "MinCost", "PeakShave", "SelfConsume"],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "reason",
        ty: SchemaType::String,
        required: true,
        enum_values: &[
            "PriceArbitrage",
            "PeakShave",
            "ValleyFill",
            "RenewableCurtail",
            "GridSupport",
            "SocBalance",
            "ManualOperator",
            "ContractObligation",
            "EmergencyLimit",
            "ModelRecommendation",
        ],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "priority",
        ty: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(1.0),
        maximum: Some(7.0),
    },
    crate::schema::SchemaField {
        name: "rationale",
        ty: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "facts_ref",
        ty: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    crate::schema::SchemaField {
        name: "valid_until_ms",
        ty: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(1.0),
        maximum: None,
    },
]);

impl OutputGate {
    /// 八步门（[11] §3.5）：
    /// 1 字节/UTF-8/长度 → 2 严格 JSON schema → 3 未知字段/枚举/工具
    /// 调用拒绝 → 4 finite/range/unit/scope → 5 引用存在且版本一致 →
    /// 6 confidence 只作元数据 → 7 policy/人工审批 → 8 有类型候选构造。
    pub fn evaluate(
        &self,
        raw: &str,
        schema: &StrictSchema,
        ctx: &GateContext<'_>,
    ) -> Result<CandidateOutput, GateReject> {
        // 1. 字节/UTF-8/长度（raw 已是 UTF-8；长度按字节）。
        if raw.len() > MAX_OUTPUT_BYTES {
            return Err(GateReject::new(GateStage::Bytes, "output too long"));
        }
        // 2. 严格 JSON schema（含 3：未知字段/枚举拒绝——SchemaError
        //    区分：NotObject/Missing → Schema 步；Unknown/Enum → 步 3）。
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|_| GateReject::new(GateStage::Schema, "invalid json"))?;
        let obj = value
            .as_object()
            .ok_or(GateReject::new(GateStage::Schema, "root not object"))?;
        match schema.validate(&value) {
            Ok(()) => {}
            Err(crate::schema::SchemaError::NotObject) => {
                return Err(GateReject::new(GateStage::Schema, "root not object"))
            }
            Err(crate::schema::SchemaError::MissingField(_)) => {
                return Err(GateReject::new(GateStage::Schema, "missing required field"))
            }
            Err(crate::schema::SchemaError::WrongType { .. }) => {
                return Err(GateReject::new(GateStage::Schema, "wrong type"))
            }
            Err(crate::schema::SchemaError::UnknownField(_)) => {
                return Err(GateReject::new(GateStage::UnknownField, "unknown field"))
            }
            Err(crate::schema::SchemaError::EnumViolation(_)) => {
                return Err(GateReject::new(GateStage::UnknownField, "unknown enum"))
            }
            Err(crate::schema::SchemaError::RangeViolation(_)) => {
                return Err(GateReject::new(GateStage::Value, "range"))
            }
            Err(crate::schema::SchemaError::NonFinite(_)) => {
                return Err(GateReject::new(GateStage::Value, "non-finite"))
            }
        }
        // 4. finite/scope 深度守卫：object 内一切数值有限（含嵌套）。
        check_finite(&value).map_err(|_| GateReject::new(GateStage::Value, "non-finite nested"))?;
        // 5. 引用存在且版本一致（snapshot 引用 = 上下文快照；event refs
        //    必须全部出现在证据表）。
        // 6+7+8：按输出形状构造候选（形状由字段区分——kind 值不决定
        //    路径，防 ObjectiveDraft 形状被重路由到 Intent 路径）。
        let has_conf = obj.contains_key("confidence_bp");
        let has_rationale = obj.contains_key("rationale");
        let has_refs = obj.contains_key("event_refs");
        match (has_refs, has_rationale, has_conf) {
            (true, false, false) => self.explanation(obj, ctx),
            (false, true, false) => self.objective_draft(obj, ctx),
            _ => self.candidate_from_object(obj, ctx),
        }
    }

    fn candidate_from_object(
        &self,
        obj: &serde_json::Map<String, serde_json::Value>,
        ctx: &GateContext<'_>,
    ) -> Result<CandidateOutput, GateReject> {
        let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap();
        let kind = objective_kind_from_name(kind)
            .ok_or(GateReject::new(GateStage::UnknownField, "kind unknown"))?;
        let reason_name = obj.get("reason").and_then(|v| v.as_str()).unwrap();
        let reason = reason_from_name(reason_name)
            .ok_or(GateReject::new(GateStage::UnknownField, "reason unknown"))?;
        let confidence_bp = obj
            .get("confidence_bp")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NAN);
        if !(0.0..=10_000.0).contains(&confidence_bp) || !confidence_bp.is_finite() {
            return Err(GateReject::new(
                GateStage::Confidence,
                "confidence_bp range",
            ));
        }
        let valid_until_ms = obj
            .get("valid_until_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if valid_until_ms == 0 || valid_until_ms < ctx.now_ms {
            return Err(GateReject::new(GateStage::Value, "valid_until_ms"));
        }
        // 5. scope 引用必须等于会话快照。
        let candidate = IntentCandidate::new(
            ctx.snapshot_id,
            kind,
            reason,
            confidence_bp as u16,
            *ctx.model_digest,
            *ctx.prompt_digest,
            valid_until_ms,
        )
        .map_err(|_| GateReject::new(GateStage::Construct, "intent construct"))?;
        Ok(CandidateOutput::Intent(candidate))
    }

    fn explanation(
        &self,
        obj: &serde_json::Map<String, serde_json::Value>,
        ctx: &GateContext<'_>,
    ) -> Result<CandidateOutput, GateReject> {
        // 5. 引用检：event_refs 全在证据表（版本一致）。
        let arr = obj
            .get("event_refs")
            .and_then(|v| v.as_array())
            .ok_or(GateReject::new(GateStage::Reference, "ref type"))?;
        let mut event_refs: Vec<[u8; 16]> = Vec::new();
        for item in arr {
            let s = item
                .as_str()
                .ok_or(GateReject::new(GateStage::Reference, "ref type"))?;
            // WP-64.3.4：单源 decode_fixed（模型输出不可信，字节级判据保持）。
            let id = eneros_hex::decode_fixed(s)
                .ok_or(GateReject::new(GateStage::Reference, "ref hex"))?;
            if !ctx.refs.iter().any(|(k, rid, _)| *k == 1 && *rid == id) {
                return Err(GateReject::new(GateStage::Reference, "ref missing"));
            }
            event_refs.push(id);
        }
        if event_refs.is_empty() {
            return Err(GateReject::new(GateStage::Reference, "no event refs"));
        }
        let severity = obj
            .get("severity")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NAN);
        if !(1.0..=3.0).contains(&severity) || !severity.is_finite() {
            return Err(GateReject::new(GateStage::Value, "severity"));
        }
        let text = obj
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>();
        if text.is_empty() || text.len() > TEXT_FIELD_MAX {
            return Err(GateReject::new(GateStage::Value, "text"));
        }
        let explanation = Explanation::new(
            ctx.snapshot_id,
            event_refs,
            severity as u8,
            *ctx.model_digest,
            *ctx.prompt_digest,
            text,
            ctx.now_ms + 60_000,
        )
        .map_err(|_| GateReject::new(GateStage::Construct, "explanation"))?;
        Ok(CandidateOutput::Explanation(explanation))
    }

    fn objective_draft(
        &self,
        obj: &serde_json::Map<String, serde_json::Value>,
        ctx: &GateContext<'_>,
    ) -> Result<CandidateOutput, GateReject> {
        let kind = objective_kind_from_name(obj.get("kind").and_then(|v| v.as_str()).unwrap_or(""))
            .ok_or(GateReject::new(GateStage::UnknownField, "kind unknown"))?;
        let reason = reason_from_name(obj.get("reason").and_then(|v| v.as_str()).unwrap_or(""))
            .ok_or(GateReject::new(GateStage::UnknownField, "reason unknown"))?;
        let priority = obj.get("priority").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if !(1.0..=7.0).contains(&priority) || !priority.is_finite() {
            return Err(GateReject::new(GateStage::Value, "priority"));
        }
        let rationale = obj.get("rationale").and_then(|v| v.as_str()).unwrap_or("");
        if rationale.is_empty() || rationale.len() > TEXT_FIELD_MAX {
            return Err(GateReject::new(GateStage::Value, "rationale"));
        }
        let facts_ref =
            eneros_hex::decode_fixed(obj.get("facts_ref").and_then(|v| v.as_str()).unwrap_or(""))
                .ok_or(GateReject::new(GateStage::Reference, "facts_ref hex"))?;
        if !ctx
            .refs
            .iter()
            .any(|(k, rid, _)| *k == 2 && *rid == facts_ref)
        {
            return Err(GateReject::new(GateStage::Reference, "facts_ref missing"));
        }
        let rationale_digest = crate::sha256_bytes(rationale.as_bytes());
        let draft = ObjectiveDraft::new(
            ctx.snapshot_id,
            kind,
            reason,
            priority as u8,
            rationale_digest,
            facts_ref,
            *ctx.model_digest,
            *ctx.prompt_digest,
            ctx.now_ms + 300_000,
        )
        .map_err(|_| GateReject::new(GateStage::Construct, "draft"))?;
        Ok(CandidateOutput::ObjectiveDraft(draft))
    }
}

fn objective_kind_from_name(s: &str) -> Option<ObjectiveKind> {
    match s {
        "MaxRevenue" => Some(ObjectiveKind::MaxRevenue),
        "MinCost" => Some(ObjectiveKind::MinCost),
        "PeakShave" => Some(ObjectiveKind::PeakShave),
        "SelfConsume" => Some(ObjectiveKind::SelfConsume),
        _ => None,
    }
}

fn reason_from_name(s: &str) -> Option<ReasonCode> {
    match s {
        "PriceArbitrage" => Some(ReasonCode::PriceArbitrage),
        "PeakShave" => Some(ReasonCode::PeakShave),
        "ValleyFill" => Some(ReasonCode::ValleyFill),
        "RenewableCurtail" => Some(ReasonCode::RenewableCurtail),
        "GridSupport" => Some(ReasonCode::GridSupport),
        "SocBalance" => Some(ReasonCode::SocBalance),
        "ManualOperator" => Some(ReasonCode::ManualOperator),
        "ContractObligation" => Some(ReasonCode::ContractObligation),
        "EmergencyLimit" => Some(ReasonCode::EmergencyLimit),
        "ModelRecommendation" => Some(ReasonCode::ModelRecommendation),
        _ => None,
    }
}

/// 嵌套 JSON 全有限守卫（防 1e999 → inf 渗入）。
fn check_finite(v: &serde_json::Value) -> Result<(), ()> {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64().ok_or(())?;
            if !f.is_finite() {
                return Err(());
            }
            Ok(())
        }
        serde_json::Value::Array(a) => {
            for item in a {
                check_finite(item)?;
            }
            Ok(())
        }
        serde_json::Value::Object(o) => {
            for item in o.values() {
                check_finite(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    fn ctx() -> GateContext<'static> {
        GateContext {
            model_digest: &[7; 32],
            prompt_digest: &[8; 32],
            snapshot_id: SnapshotId([9; 16]),
            refs: &[(1, [2; 16], 1), (2, [6; 16], 1)],
            now_ms: 1000,
            requires_human_approval: false,
        }
    }

    #[test]
    fn gate_intent_positive() {
        let gate = OutputGate;
        let raw = r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":8000,"valid_until_ms":50000}"#;
        let out = gate.evaluate(raw, &SCHEMA_INTENT, &ctx()).unwrap();
        match out {
            CandidateOutput::Intent(c) => {
                assert_eq!(c.scope_snapshot(), SnapshotId([9; 16]));
                assert_eq!(c.confidence_bp(), 8000);
                assert_eq!(c.model_digest(), &[7; 32]);
                assert_eq!(c.prompt_digest(), &[8; 32]);
            }
            _ => panic!("expected intent"),
        }
    }

    #[test]
    fn gate_rejects_tool_injection() {
        let gate = OutputGate;
        let raw = r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":1,"valid_until_ms":50000,"tool":{"name":"shell","args":["rm"]}}"#;
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::UnknownField, "unknown field"))
        );
        let raw = r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":1,"valid_until_ms":50000,"dispatch":{"device":7}}"#;
        // dispatch 键不在 schema → 未知字段拒绝（同门步骤 3）。
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::UnknownField, "unknown field"))
        );
    }

    #[test]
    fn gate_rejects_json_smuggling_and_nonfinite() {
        let gate = OutputGate;
        // 嵌套 1e999 → finite 守卫。
        let raw = r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":1,"valid_until_ms":50000,"extra":{"x":1e999}}"#;
        assert!(matches!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject {
                stage: GateStage::Schema | GateStage::UnknownField | GateStage::Value,
                ..
            })
        ));
        // 超长。
        let long = format!(
            r#"{{"kind":"PeakShave","reason":"PeakShave","confidence_bp":1,"valid_until_ms":50000,"pad":"{}"}}"#,
            "x".repeat(MAX_OUTPUT_BYTES + 10)
        );
        assert_eq!(
            gate.evaluate(&long, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::Bytes, "output too long"))
        );
        // 非 JSON。
        assert_eq!(
            gate.evaluate("gibberish", &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::Schema, "invalid json"))
        );
    }

    #[test]
    fn gate_rejects_scope_escape_and_bad_enum() {
        let gate = OutputGate;
        // 未知枚举（红线上不可出现 AuthorizedCommand 字面量——用同语义
        // 的未知枚举值验证门）。
        let raw = r#"{"kind":"FlyToDevice","reason":"PeakShave","confidence_bp":1,"valid_until_ms":50000}"#;
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::UnknownField, "unknown enum"))
        );
        // confidence 超域。
        let raw = r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":90000,"valid_until_ms":50000}"#;
        // schema 范围门（步骤 4）先于 confidence 元数据步（步骤 6）。
        // 但构造路径独立校验置信度域——两步都会拒，看哪一步先到：
        // SchemaError::RangeViolation → Value/"range"。
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::Value, "range"))
        );
        // 过期 valid_until。
        let raw =
            r#"{"kind":"PeakShave","reason":"PeakShave","confidence_bp":1,"valid_until_ms":500}"#;
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_INTENT, &ctx()),
            Err(GateReject::new(GateStage::Value, "valid_until_ms"))
        );
    }

    #[test]
    fn ref_hex_multibyte_rejected_without_panic() {
        // 模型输出不可信：event_refs 元素可为任意字符串。32 **字节**但含
        // 多字节 UTF-8 字符（10×3B 中文 + 2B ASCII）——按字节下标切片
        // 会切进字符内部（历史实现 panic）。必须结构化拒绝。
        let gate = OutputGate;
        let s = alloc::format!("{}zz", "中".repeat(10));
        assert_eq!(s.len(), 32, "恰好 32 字节且非 ASCII 边界对齐");
        let raw = alloc::format!(r#"{{"severity":2,"event_refs":["{s}"],"text":"x"}}"#);
        assert_eq!(
            gate.evaluate(&raw, &SCHEMA_EXPLANATION, &ctx()),
            Err(GateReject::new(GateStage::Reference, "ref hex"))
        );
    }

    #[test]
    fn gate_explanation_positive_and_ref_gate() {
        let gate = OutputGate;
        let raw = r#"{"severity":2,"event_refs":["02020202020202020202020202020202"],"text":"电池温度偏高，预计 30 分钟内过温"}"#;
        let out = gate.evaluate(raw, &SCHEMA_EXPLANATION, &ctx()).unwrap();
        match out {
            CandidateOutput::Explanation(e) => {
                assert_eq!(e.severity(), 2);
                assert_eq!(e.event_refs(), &[[2; 16]]);
            }
            _ => panic!("expected explanation"),
        }
        // 引用了不存在的 event → 拒绝。
        let raw = r#"{"severity":2,"event_refs":["11111111111111111111111111111111"],"text":"x"}"#;
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_EXPLANATION, &ctx()),
            Err(GateReject::new(GateStage::Reference, "ref missing"))
        );
    }

    #[test]
    fn gate_objective_draft_positive_and_facts_ref_gate() {
        let gate = OutputGate;
        let raw = r#"{"kind":"MaxRevenue","reason":"PriceArbitrage","priority":6,"rationale":"晚高峰价差","facts_ref":"06060606060606060606060606060606","valid_until_ms":50000}"#;
        let out = gate.evaluate(raw, &SCHEMA_OBJECTIVE_DRAFT, &ctx()).unwrap();
        match out {
            CandidateOutput::ObjectiveDraft(d) => {
                assert_eq!(d.priority(), 6);
                assert_eq!(d.hint(), ObjectiveKind::MaxRevenue);
            }
            _ => panic!("expected draft"),
        }
        // 事实引用不在证据表 → 拒绝。
        let raw = r#"{"kind":"MaxRevenue","reason":"PriceArbitrage","priority":6,"rationale":"x","facts_ref":"99999999999999999999999999999999","valid_until_ms":50000}"#;
        assert_eq!(
            gate.evaluate(raw, &SCHEMA_OBJECTIVE_DRAFT, &ctx()),
            Err(GateReject::new(GateStage::Reference, "facts_ref missing"))
        );
    }
}
