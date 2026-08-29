//! 蓝图 49 WP-49.2：target↔peer 消息契约（远程 dual-brain，方案 B）。
//!
//! System2 = 站端 LLM 服务（不受信 peer）。本模块定义双向 JSON 契约与
//! **target 侧全量校验**（ADR-0045 决策细则 #1）：peer 应答在进入决策前
//! 必须通过 schema（`deny_unknown_fields`）/版本枚举闭集/数值域/sequence
//! 单调/req_id 配对校验；任何一项失败 → 坏应答丢弃计数 → 本次降级。
//!
//! 宣称纪律（ADR-0045 #4）约束措辞：本链路为「AI 辅助决策（需配套站端
//! 服务）」，非设备端本地推理。

use alloc::string::{String, ToString};

use serde::{Deserialize, Serialize};

/// 契约版本（枚举闭集：仅 v1——未知版本拒绝）。
pub const PEER_CONTRACT_VERSION: u64 = 1;

/// 请求 JSON 尺寸上限（防闭包膨胀；超出即拒绝组装）。
pub const PEER_REQUEST_MAX_LEN: usize = 1024;

// ============================================================================
// target → peer：决策请求（点值快照 + 预算 + 约束）
// ============================================================================

/// 决策请求（target 构建，经 MQTT QoS1 发布到 `ai_request_topic`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRequest {
    /// 契约版本（恒 1）。
    pub v: u64,
    /// 请求相关 id（单调递增；应答必须回带）。
    pub req_id: u64,
    /// 请求构建时间（target 毫秒时钟）。
    pub ts_ms: u64,
    /// 受信遥测快照要素（safety-validator 已裁决的 trusted 输入投影）。
    pub telemetry: PeerTelemetry,
    /// 预算/约束（越界提案将被 target 拒绝）。
    pub budget: PeerBudget,
}

/// 遥测快照投影（只给决策所需字段，不透传全部内部状态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerTelemetry {
    pub soc_pct: f64,
    pub voltage_v: f64,
    pub current_a: f64,
    pub frequency_hz: f64,
    /// 快照生成时间（trusted 链基准）。
    pub generated_at_ms: u64,
    /// provenance digest hex（64 字符小写——审计对账键）。
    pub digest_hex: String,
}

/// 预算/约束。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerBudget {
    /// 功率幅值上限 kW（|power_kw| 超出即越权提案，拒收）。
    pub max_power_kw: f64,
    /// 提案 TTL 上限秒（plan.ttl_secs 超出即拒收）。
    pub ttl_secs: u32,
}

impl PeerRequest {
    pub fn encode(&self) -> Result<String, PeerContractError> {
        if self.v != PEER_CONTRACT_VERSION {
            return Err(PeerContractError::Version(self.v));
        }
        let s = serde_json::to_string(self)
            .map_err(|e| PeerContractError::Serialization(e.to_string()))?;
        if s.len() > PEER_REQUEST_MAX_LEN {
            return Err(PeerContractError::TooLarge(s.len()));
        }
        Ok(s)
    }
}

// ============================================================================
// peer → target：计划提案应答（不受信输入——全量校验后才可进入决策）
// ============================================================================

/// 计划提案应答。**`deny_unknown_fields`：schema 闭集，未知键整体拒绝。**
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerResponse {
    /// 契约版本（仅接受 1）。
    pub v: u64,
    /// 必须等于对应请求的 req_id。
    pub req_id: u64,
    /// peer 会话内单调递增序号（防重放/乱序采信）。
    pub seq: u64,
    pub plan: PeerPlan,
    /// 决策理由码/摘要（非空；入审计截断）。
    pub reason: String,
}

/// 提案计划体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerPlan {
    /// 有功功率 kW（正=放电，负=充电；数值域校验见 [`validate_response`]）。
    pub power_kw: f64,
    /// 目标设备（非空；dispatch 授权按点表绑定裁决）。
    pub target_device: String,
    /// TTL 秒（∈ [1, budget.ttl_secs]）。
    pub ttl_secs: u32,
}

/// 契约错误（仅 Debug，D9 同款简化）。
#[derive(Debug)]
pub enum PeerContractError {
    /// JSON 解析失败或未知键（schema 违规）。
    Schema(String),
    /// 版本不在闭集。
    Version(u64),
    /// req_id 与在途请求不配对。
    Correlation { expected: u64, got: u64 },
    /// sequence 非单调（重放/乱序）。
    Sequence { last: Option<u64>, got: u64 },
    /// 数值域/必填校验失败（字段, 原因）。
    Range(String, String),
    /// 序列化失败。
    Serialization(String),
    /// 编码超限。
    TooLarge(usize),
}

/// 应答校验上下文（target 在途状态投影）。
#[derive(Debug, Clone, Copy)]
pub struct ResponseChecks<'a> {
    /// 在途请求 req_id（配对强制）。
    pub expect_req_id: u64,
    /// 已接受的最后 seq（None=会话首帧）；必须严格大于。
    pub expect_seq_gt: Option<u64>,
    /// 发出请求时的预算（数值域上界）。
    pub budget: &'a PeerBudget,
}

/// 解析 + 全量校验一条不受信应答（蓝图 WP-49.2 #1 判据面）。
///
/// SPEC-SERIES-2026-08-27/E WP-E.1（清洗单源）：入口先经
/// `normalize_llm_json`（intent-parser 单源，本 crate 已依赖、零新依赖边）
/// 剥围栏/多余文本并做字符串数字归一，再走 schema 校验——围栏包裹/数字
/// 加引号的合法应答不再被误拒。清洗只做形态归一：无 JSON 可提取或提取
/// 后仍非法时原样/提取结果交由 serde 拒绝（Bad 语义不变，**清洗不扩权
/// 放行**——越权/未知键/域违规照旧拦截）。
///
/// 校验顺序（短路）：归一化 → schema 解析（含未知键拒绝）→ 版本闭集 →
/// req_id 配对 → sequence 单调 → reason/target 非空 → 功率有限且 ≤ 预算
/// → TTL ∈ [1, budget.ttl_secs]。
pub fn parse_and_validate(
    json: &str,
    checks: &ResponseChecks<'_>,
) -> Result<PeerResponse, PeerContractError> {
    let normalized = eneros_intent_parser::parser::normalize_llm_json(json);
    let resp: PeerResponse =
        serde_json::from_str(&normalized).map_err(|e| PeerContractError::Schema(e.to_string()))?;
    validate_response(&resp, checks)?;
    Ok(resp)
}

/// 对已解析应答执行全量域校验。
pub fn validate_response(
    resp: &PeerResponse,
    checks: &ResponseChecks<'_>,
) -> Result<(), PeerContractError> {
    // 版本枚举闭集。
    if resp.v != PEER_CONTRACT_VERSION {
        return Err(PeerContractError::Version(resp.v));
    }
    // 请求相关性。
    if resp.req_id != checks.expect_req_id {
        return Err(PeerContractError::Correlation {
            expected: checks.expect_req_id,
            got: resp.req_id,
        });
    }
    // sequence 单调（严格递增；相等也拒——重放）。
    if let Some(last) = checks.expect_seq_gt {
        if resp.seq <= last {
            return Err(PeerContractError::Sequence {
                last: Some(last),
                got: resp.seq,
            });
        }
    }
    // 必填非空。
    if resp.reason.trim().is_empty() {
        return Err(PeerContractError::Range(
            String::from("reason"),
            String::from("empty"),
        ));
    }
    if resp.plan.target_device.trim().is_empty() {
        return Err(PeerContractError::Range(
            String::from("plan.target_device"),
            String::from("empty"),
        ));
    }
    // 数值域：功率有限 + 幅值 ≤ 预算（越权提案在此拦截）。
    if !resp.plan.power_kw.is_finite() {
        return Err(PeerContractError::Range(
            String::from("plan.power_kw"),
            String::from("non-finite"),
        ));
    }
    if resp.plan.power_kw.abs() > checks.budget.max_power_kw {
        return Err(PeerContractError::Range(
            String::from("plan.power_kw"),
            "exceeds budget".into(),
        ));
    }
    // TTL 数值域。
    if resp.plan.ttl_secs == 0 || resp.plan.ttl_secs > checks.budget.ttl_secs {
        return Err(PeerContractError::Range(
            String::from("plan.ttl_secs"),
            String::from("out of [1, budget.ttl_secs]"),
        ));
    }
    Ok(())
}

/// 审计摘要截断（ADR-0045 #3：prompt ≤48 / 应答 ≤64，不落全文）。
pub fn truncate_for_audit(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return String::from(s);
    }
    // 字节边界安全截断（ASCII 域内等价 chars 截断）。
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::from(&s[..end]);
    out.push('…');
    out
}

impl PeerRequest {
    /// 构造请求（bin 共用；遥测投影由调用方组装——参数数受
    /// clippy::too_many_arguments 纪律约束，不设 9 参便利函数）。
    pub fn new(req_id: u64, ts_ms: u64, telemetry: PeerTelemetry, budget: PeerBudget) -> Self {
        Self {
            v: PEER_CONTRACT_VERSION,
            req_id,
            ts_ms,
            telemetry,
            budget,
        }
    }
}

/// 便利构造（预算）。
pub fn budget(max_power_kw: f64, ttl_secs: u32) -> PeerBudget {
    PeerBudget {
        max_power_kw,
        ttl_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: PeerBudget = PeerBudget {
        max_power_kw: 5.0,
        ttl_secs: 60,
    };

    fn valid_json(seq: u64) -> String {
        format!(
            r#"{{"v":1,"req_id":7,"seq":{seq},"plan":{{"power_kw":-3.0,"target_device":"pcs","ttl_secs":60}},"reason":"soc low"}}"#
        )
    }

    #[test]
    fn request_roundtrip_and_size_cap() {
        let req = PeerRequest::new(
            7,
            1000,
            PeerTelemetry {
                soc_pct: 0.6,
                voltage_v: 380.0,
                current_a: 10.0,
                frequency_hz: 50.0,
                generated_at_ms: 900,
                digest_hex: "ab".repeat(32),
            },
            budget(5.0, 60),
        );
        let json = req.encode().expect("encode");
        assert!(json.contains("\"req_id\":7"));
        assert!(json.len() <= PEER_REQUEST_MAX_LEN);
    }

    #[test]
    fn request_rejects_wrong_version() {
        let mut req = PeerRequest::new(
            1,
            1,
            PeerTelemetry {
                soc_pct: 0.5,
                voltage_v: 1.0,
                current_a: 1.0,
                frequency_hz: 50.0,
                generated_at_ms: 1,
                digest_hex: String::from("ab"),
            },
            budget(5.0, 60),
        );
        req.v = 2;
        assert!(matches!(req.encode(), Err(PeerContractError::Version(2))));
    }

    #[test]
    fn valid_response_accepted() {
        let r = parse_and_validate(
            &valid_json(3),
            &ResponseChecks {
                expect_req_id: 7,
                expect_seq_gt: Some(2),
                budget: &BUDGET,
            },
        )
        .expect("valid");
        assert_eq!(r.plan.power_kw, -3.0);
        assert_eq!(r.seq, 3);
    }

    #[test]
    fn first_frame_seq_none_accepted() {
        let r = parse_and_validate(
            &valid_json(0),
            &ResponseChecks {
                expect_req_id: 7,
                expect_seq_gt: None,
                budget: &BUDGET,
            },
        );
        assert!(r.is_ok());
    }

    #[test]
    fn unknown_key_rejected_schema_closed() {
        let bad = r#"{"v":1,"req_id":7,"seq":9,"plan":{"power_kw":1.0,"target_device":"pcs","ttl_secs":60},"reason":"x","extra":1}"#;
        assert!(matches!(
            parse_and_validate(
                bad,
                &ResponseChecks {
                    expect_req_id: 7,
                    expect_seq_gt: None,
                    budget: &BUDGET
                }
            ),
            Err(PeerContractError::Schema(_))
        ));
    }

    #[test]
    fn wrong_version_rejected() {
        let bad = valid_json(1).replace("\"v\":1", "\"v\":2");
        assert!(matches!(
            parse_and_validate(
                &bad,
                &ResponseChecks {
                    expect_req_id: 7,
                    expect_seq_gt: None,
                    budget: &BUDGET
                }
            ),
            Err(PeerContractError::Version(2))
        ));
    }

    #[test]
    fn correlation_mismatch_rejected() {
        assert!(matches!(
            parse_and_validate(
                &valid_json(1),
                &ResponseChecks {
                    expect_req_id: 8,
                    expect_seq_gt: None,
                    budget: &BUDGET
                }
            ),
            Err(PeerContractError::Correlation {
                expected: 8,
                got: 7
            })
        ));
    }

    #[test]
    fn sequence_replay_and_regression_rejected() {
        let checks = |gt| ResponseChecks {
            expect_req_id: 7,
            expect_seq_gt: gt,
            budget: &BUDGET,
        };
        // 相等 = 重放。
        assert!(matches!(
            parse_and_validate(&valid_json(5), &checks(Some(5))),
            Err(PeerContractError::Sequence { .. })
        ));
        // 回退。
        assert!(matches!(
            parse_and_validate(&valid_json(4), &checks(Some(5))),
            Err(PeerContractError::Sequence { .. })
        ));
    }

    #[test]
    fn over_budget_power_rejected() {
        let bad = valid_json(1).replace("-3.0", "-5.5");
        assert!(matches!(
            parse_and_validate(&bad, &ResponseChecks { expect_req_id: 7, expect_seq_gt: None, budget: &BUDGET }),
            Err(PeerContractError::Range(f, _)) if f == "plan.power_kw"
        ));
    }

    #[test]
    fn nonfinite_power_unparsable_or_rejected() {
        // JSON 标准不允许 NaN/Inf 字面量——serde 直接 Schema 拒绝。
        let bad = valid_json(1).replace("-3.0", "NaN");
        assert!(matches!(
            parse_and_validate(
                &bad,
                &ResponseChecks {
                    expect_req_id: 7,
                    expect_seq_gt: None,
                    budget: &BUDGET
                }
            ),
            Err(PeerContractError::Schema(_))
        ));
    }

    #[test]
    fn zero_ttl_rejected_and_over_budget_ttl_rejected() {
        let zero = valid_json(1).replace("\"ttl_secs\":60}", "\"ttl_secs\":0}");
        assert!(matches!(
            parse_and_validate(&zero, &ResponseChecks { expect_req_id: 7, expect_seq_gt: None, budget: &BUDGET }),
            Err(PeerContractError::Range(f, _)) if f == "plan.ttl_secs"
        ));
        let over = valid_json(1).replace("\"ttl_secs\":60}", "\"ttl_secs\":61}");
        assert!(matches!(
            parse_and_validate(&over, &ResponseChecks { expect_req_id: 7, expect_seq_gt: None, budget: &BUDGET }),
            Err(PeerContractError::Range(f, _)) if f == "plan.ttl_secs"
        ));
    }

    #[test]
    fn empty_reason_rejected() {
        let bad = valid_json(1).replace("\"reason\":\"soc low\"", "\"reason\":\"  \"");
        assert!(matches!(
            parse_and_validate(&bad, &ResponseChecks { expect_req_id: 7, expect_seq_gt: None, budget: &BUDGET }),
            Err(PeerContractError::Range(f, _)) if f == "reason"
        ));
    }

    #[test]
    fn malformed_json_rejected() {
        assert!(matches!(
            parse_and_validate(
                "{not json",
                &ResponseChecks {
                    expect_req_id: 7,
                    expect_seq_gt: None,
                    budget: &BUDGET
                }
            ),
            Err(PeerContractError::Schema(_))
        ));
    }

    // ===== SPEC-SERIES-2026-08-27/E WP-E.1：清洗单源判据面 =====
    // 判据 C1：围栏/别名/碎裂/垃圾/超大六例 + 清洗不扩权反例。

    fn checks_first_frame() -> ResponseChecks<'static> {
        ResponseChecks {
            expect_req_id: 7,
            expect_seq_gt: None,
            budget: &BUDGET,
        }
    }

    /// C1-a：```json 围栏包裹的合法应答 ⇒ 过（旧实现 Schema 误拒）。
    #[test]
    fn e1_fenced_json_accepted() {
        let fenced = alloc::format!("```json\n{}\n```", valid_json(3));
        let r = parse_and_validate(&fenced, &checks_first_frame()).expect("围栏应剥除后过校验");
        assert_eq!(r.plan.power_kw, -3.0);
        assert_eq!(r.seq, 3);
    }

    /// C1-b：xml 围栏 + 前后说明文字 ⇒ 过（首个平衡对象提取）。
    #[test]
    fn e1_xml_fence_and_prose_accepted() {
        let wrapped = alloc::format!(
            "<answer>模型输出如下</answer>\n{}\n以上供参考",
            valid_json(1)
        );
        parse_and_validate(&wrapped, &checks_first_frame()).expect("围栏+说明文字应过校验");
    }

    /// C1-c：键别名形态——数字字段加引号（真实小模型常见偏离）⇒ 归一后过。
    #[test]
    fn e1_string_number_coercion_accepted() {
        let aliased = r#"{"v":"1","req_id":"7","seq":"5","plan":{"power_kw":"-3.0","target_device":"pcs","ttl_secs":"60"},"reason":"soc low"}"#;
        let r = parse_and_validate(aliased, &checks_first_frame()).expect("字符串数字应归一");
        assert_eq!(r.plan.power_kw, -3.0);
        assert_eq!(r.seq, 5);
        assert_eq!(r.plan.ttl_secs, 60);
    }

    /// C1-d：嵌套碎裂（内层对象截断）⇒ Bad 不变（Schema 拒绝）。
    #[test]
    fn e1_nested_broken_still_bad() {
        let broken = r#"{"v":1,"req_id":7,"seq":9,"plan":{"power_kw":-3.0,"target_device"#;
        assert!(matches!(
            parse_and_validate(broken, &checks_first_frame()),
            Err(PeerContractError::Schema(_))
        ));
    }

    /// C1-e：纯垃圾（无平衡对象）⇒ Bad 不变。
    #[test]
    fn e1_pure_garbage_still_bad() {
        assert!(matches!(
            parse_and_validate("complete nonsense, no json here", &checks_first_frame()),
            Err(PeerContractError::Schema(_))
        ));
    }

    /// C1-f：超大 payload（256KiB 垃圾）⇒ Bad 且不 panic（不扩权放行）。
    #[test]
    fn e1_huge_payload_bad_without_panic() {
        let mut huge = String::from("{\"v\":1,\"req_id\":7,\"seq\":1,\"junk\":\"");
        huge.push_str(&"x".repeat(256 * 1024));
        huge.push_str("\"}");
        let out = parse_and_validate(&huge, &checks_first_frame());
        assert!(matches!(out, Err(PeerContractError::Schema(_))));
    }

    /// 清洗不扩权：围栏包裹的**越权**提案照旧被数值域拦截（Range，非放行）。
    #[test]
    fn e1_normalization_does_not_grant_authority() {
        let over = valid_json(1).replace("-3.0", "-5.5");
        let fenced = alloc::format!("```json\n{over}\n```");
        assert!(matches!(
            parse_and_validate(&fenced, &checks_first_frame()),
            Err(PeerContractError::Range(f, _)) if f == "plan.power_kw"
        ));
    }

    /// 清洗不扩权：围栏包裹的未知键照旧被 schema 闭集拒绝。
    #[test]
    fn e1_normalization_preserves_closed_schema() {
        let extra = valid_json(1).replace("\"reason\":\"soc low\"", "\"reason\":\"x\",\"extra\":1");
        let fenced = alloc::format!("```json\n{extra}\n```");
        assert!(matches!(
            parse_and_validate(&fenced, &checks_first_frame()),
            Err(PeerContractError::Schema(_))
        ));
    }

    /// 语义钉子：说明文字含多个对象时取**首个**平衡对象（normalize 既有
    /// 语义，不择优——避免清洗层引入语义判断）。
    #[test]
    fn e1_first_balanced_object_wins() {
        let decoy = r#"contradiction {"v":9} then real:"#;
        let with_real = alloc::format!("{decoy}{}", valid_json(1));
        assert!(matches!(
            parse_and_validate(&with_real, &checks_first_frame()),
            Err(PeerContractError::Schema(_))
        ));
    }

    #[test]
    fn audit_truncation_byte_safe() {
        assert_eq!(truncate_for_audit("short", 8), "short");
        let long = "x".repeat(100);
        let t = truncate_for_audit(&long, 48);
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().count(), 49);
    }
}
