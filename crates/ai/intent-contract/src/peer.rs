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

// ============================================================================
// 蓝图 70.10（WP-70.10.1/70.10.2）：**结构化全计划**站端通道 v2
// ============================================================================
//
// v1（上方）只承载单值 `power_kw`：目标侧拿到的是 host 取第 1 步再钳制的
// 有损投影。v2 把**完整计划**搬过通道：请求携带目标侧事实引用（快照身份/
// 目录代次/时域/逐资产规格/目标价格），应答携带全部资源 × 全部时段的
// 分配、求解状态与模型/输入摘要。
//
// 纪律（与 v1 同律，逐条加严）：
// - `deny_unknown_fields`：未知键整体拒绝；
// - 版本闭集（仅 2）；身份**逐字段绑定**（快照/目录/时域必须与请求一致）；
// - 分配条数 ≤ [`PEER_PLAN_MAX_ASSIGNMENTS`]（与 decision-contract inline
//   上限同源），每资产恰好 `step_count` 步（轨迹长度不符即拒）；
// - 数值有限、非零资产、步序 ∈ [1, step_count]；**预算越界即拒**——
//   v2 禁止 host 钳制后仍声称原求解证明（钳制 = 计划不再是被认证的那份）。

use alloc::vec::Vec;

/// v2 契约版本。
pub const PEER_CONTRACT_VERSION_V2: u64 = 2;
/// v2 请求 JSON 尺寸上限（结构化输入：逐资产规格 + 价格序列）。
pub const PEER_REQUEST_MAX_LEN_V2: usize = 4096;
/// v2 应答 JSON 尺寸上限（64 项分配 × ~90B + 头部）。
pub const PEER_RESPONSE_MAX_LEN_V2: usize = 8192;
/// 计划级分配条数上限（= decision-contract inline assignment 上限 64；
/// 蓝图 08 §9 容量表：完整候选 inline 最多 64 assignments，64 上限与 65
/// 拒绝分别测试；3 资源 × 4 时段 = 12 为最低功能演示）。
/// 与单批上限分离：**每批** intent 上限 = 8（DEC-70-13 ②，传输容量墙
/// ≈11 留 3 余量——落点 = control-dispatch 编译器
/// `MAX_INTENTS_PER_BATCH` 单源）；计划超 8 分配由目标侧编译拆批、
/// 每批独立认证（无跨批免认证）。
pub const PEER_PLAN_MAX_ASSIGNMENTS: usize = 64;

/// 逐资产规格（目标侧事实 → host 模型输入；**不是** host 自持 fixture）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerAssetSpec {
    pub asset_id: u64,
    /// 铭牌容量 kWh（>0；目录铭牌同代）。
    pub capacity_kwh: f64,
    pub eta_c: f64,
    pub eta_d: f64,
    /// 快照 SOC × 容量的初始能量 kWh。
    pub initial_kwh: f64,
    /// 可用 SOC 窗（0..1 分数；目录铭牌）。
    pub soc_min: f64,
    pub soc_max: f64,
    /// 功率窗 kW（包络求交后）。
    pub p_min_kw: f64,
    pub p_max_kw: f64,
    pub ramp_up_kw_s: f64,
    pub ramp_down_kw_s: f64,
    /// 分配绑定（device/point/unit；0 = 未绑定，由 dispatch 解析）。
    pub device_id: u16,
    pub point_id: u32,
    pub unit_code: u16,
}

/// 结构化决策请求 v2（target → host）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRequestV2 {
    pub v: u64,
    pub req_id: u64,
    pub ts_ms: u64,
    /// 冻结快照身份（hex 32；绑定的唯一代次）。
    pub snapshot_id_hex: String,
    /// 快照 provenance root（hex 64）。
    pub provenance_hex: String,
    /// 目录合同版本（同代）。
    pub catalog_version: u64,
    /// 计划时域（起点 ms / 步长 ns / 步数）。
    pub horizon_start_ms: u64,
    pub step_ns: u64,
    pub step_count: u32,
    /// 逐资产规格（≥1）。
    pub assets: Vec<PeerAssetSpec>,
    /// 并网点资产（0 = 无）。
    pub grid_asset_id: u64,
    /// 目标：`min_cost` | `max_revenue`（闭集）。
    pub objective_kind: String,
    /// 每步电价（元/kWh；len == step_count）。
    pub prices_per_kwh: Vec<f64>,
    /// 预算（功率幅值/TTL 上界；host 不得越界提案）。
    pub budget: PeerBudget,
}

impl PeerRequestV2 {
    /// 编码 + 闭合校验（host 侧入口与 target 侧构造共用）。
    pub fn encode(&self) -> Result<String, PeerContractError> {
        validate_request_v2(self)?;
        let s = serde_json::to_string(self)
            .map_err(|e| PeerContractError::Serialization(e.to_string()))?;
        if s.len() > PEER_REQUEST_MAX_LEN_V2 {
            return Err(PeerContractError::TooLarge(s.len()));
        }
        Ok(s)
    }
}

/// 请求 v2 闭合校验（schema 之外的结构/数值判据）。
pub fn validate_request_v2(req: &PeerRequestV2) -> Result<(), PeerContractError> {
    if req.v != PEER_CONTRACT_VERSION_V2 {
        return Err(PeerContractError::Version(req.v));
    }
    if !is_hex(&req.snapshot_id_hex, 32) {
        return Err(PeerContractError::Range(
            String::from("snapshot_id_hex"),
            String::from("expect 32 lowercase hex"),
        ));
    }
    if !is_hex(&req.provenance_hex, 64) {
        return Err(PeerContractError::Range(
            String::from("provenance_hex"),
            String::from("expect 64 lowercase hex"),
        ));
    }
    if req.catalog_version == 0 || req.step_count == 0 || req.step_ns == 0 {
        return Err(PeerContractError::Range(
            String::from("horizon"),
            String::from("zero catalog/step_count/step_ns"),
        ));
    }
    if req.assets.is_empty() {
        return Err(PeerContractError::Range(
            String::from("assets"),
            String::from("empty"),
        ));
    }
    if req.assets.len() as u64 * u64::from(req.step_count) > PEER_PLAN_MAX_ASSIGNMENTS as u64 {
        return Err(PeerContractError::Range(
            String::from("assets"),
            String::from("assets × steps exceeds plan cap"),
        ));
    }
    if req.prices_per_kwh.len() != req.step_count as usize {
        return Err(PeerContractError::Range(
            String::from("prices_per_kwh"),
            String::from("len != step_count"),
        ));
    }
    if req.prices_per_kwh.iter().any(|p| !p.is_finite()) {
        return Err(PeerContractError::Range(
            String::from("prices_per_kwh"),
            String::from("non-finite"),
        ));
    }
    match req.objective_kind.as_str() {
        "min_cost" | "max_revenue" => {}
        _ => {
            return Err(PeerContractError::Range(
                String::from("objective_kind"),
                String::from("not in closed set"),
            ))
        }
    }
    if !req.budget.max_power_kw.is_finite() || req.budget.max_power_kw <= 0.0 {
        return Err(PeerContractError::Range(
            String::from("budget.max_power_kw"),
            String::from("non-positive"),
        ));
    }
    for a in &req.assets {
        if a.asset_id == 0 {
            return Err(PeerContractError::Range(
                String::from("assets[].asset_id"),
                String::from("zero"),
            ));
        }
        let finite = [
            a.capacity_kwh,
            a.eta_c,
            a.eta_d,
            a.initial_kwh,
            a.soc_min,
            a.soc_max,
            a.p_min_kw,
            a.p_max_kw,
            a.ramp_up_kw_s,
            a.ramp_down_kw_s,
        ];
        if finite.iter().any(|v| !v.is_finite()) {
            return Err(PeerContractError::Range(
                String::from("assets[]"),
                String::from("non-finite parameter"),
            ));
        }
        if a.capacity_kwh <= 0.0
            || a.p_min_kw > a.p_max_kw
            || a.soc_min < 0.0
            || a.soc_max > 1.0
            || a.soc_min >= a.soc_max
            || a.ramp_up_kw_s < 0.0
            || a.ramp_down_kw_s < 0.0
        {
            return Err(PeerContractError::Range(
                String::from("assets[]"),
                String::from("inconsistent parameters"),
            ));
        }
    }
    Ok(())
}

/// 计划绑定（target 期望的同代事实；逐字段比对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanBinds {
    pub snapshot_id: [u8; 16],
    pub catalog_version: u64,
    pub horizon_start_ms: u64,
    pub step_ns: u64,
    pub step_count: u32,
}

/// 单条分配（资源 × 步）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerAssignment {
    pub asset_id: u64,
    pub device_id: u16,
    pub point_id: u32,
    pub unit_code: u16,
    /// 1-based 步序（∈ [1, step_count]）。
    pub step_index: u32,
    pub power_kw: f64,
}

/// 结构化计划体（v2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerPlanV2 {
    /// 绑定字段（必须与请求/目标侧在途输入逐字段相等）。
    pub snapshot_id_hex: String,
    pub catalog_version: u64,
    pub horizon_start_ms: u64,
    pub step_ns: u64,
    pub step_count: u32,
    /// `optimal` | `suboptimal`（闭集；非成功求解不得应答）。
    pub solver_status: String,
    /// 模型摘要（hex 64）——求解证明的可复验引用。
    pub model_digest_hex: String,
    /// 输入摘要（hex 64）。
    pub input_digest_hex: String,
    pub objective_value: f64,
    pub relative_gap: f64,
    pub elapsed_ms: u64,
    /// 全量分配（≤ [`PEER_PLAN_MAX_ASSIGNMENTS`]）。
    pub assignments: Vec<PeerAssignment>,
    pub target_device: String,
    pub ttl_secs: u32,
}

/// 结构化计划应答（v2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerResponseV2 {
    pub v: u64,
    pub req_id: u64,
    pub seq: u64,
    pub plan: PeerPlanV2,
    pub reason: String,
}

impl PeerResponseV2 {
    /// 编码（host 侧；尺寸闭合）。
    pub fn encode(&self) -> Result<String, PeerContractError> {
        let s = serde_json::to_string(self)
            .map_err(|e| PeerContractError::Serialization(e.to_string()))?;
        if s.len() > PEER_RESPONSE_MAX_LEN_V2 {
            return Err(PeerContractError::TooLarge(s.len()));
        }
        Ok(s)
    }
}

/// 解析 + 全量校验一条 v2 应答（target 侧；**不受信输入**）。
///
/// **不走 LLM 归一化**（与 v1 的关键差异，复查实锤）：v1 的
/// `normalize_llm_json` 会把"看起来像数字的字符串"去引号（为 LLM 输出
/// 容错而设）——v2 的 hex 摘要/身份字段恰恰可能是全数字串
/// （如 `snapshot_id_hex = "1111…"`），归一会把它变成 JSON 数字，
/// 使**合法计划被 schema 拒绝**（实测：64 位十六进制全数字 ⇒
/// `invalid type: floating point …, expected a string`）。v2 是机器对机器
/// 的结构化通道，故解析**严格**：不做任何形态改写（也不接受围栏/多余
/// 文本——那是 LLM 通道的容错面，不属于结构化计划通道）。
///
/// 判据（短路）：schema → 版本闭集 → req_id 配对 → seq 单调 → 绑定逐字段
/// 相等 → 求解状态闭集 → 摘要 hex → 分配结构（条数/步序/资产非零/唯一性/
/// 轨迹完整）→ 数值有限 → 预算域 → TTL 域。
pub fn parse_and_validate_v2(
    json: &str,
    checks: &ResponseChecks<'_>,
    binds: &PlanBinds,
) -> Result<PeerResponseV2, PeerContractError> {
    let resp: PeerResponseV2 =
        serde_json::from_str(json).map_err(|e| PeerContractError::Schema(e.to_string()))?;
    validate_response_v2(&resp, checks, binds)?;
    Ok(resp)
}

/// 对已解析 v2 应答执行全量校验（单源；`parse_and_validate_v2` 与
/// 装配期预检共用）。
pub fn validate_response_v2(
    resp: &PeerResponseV2,
    checks: &ResponseChecks<'_>,
    binds: &PlanBinds,
) -> Result<(), PeerContractError> {
    if resp.v != PEER_CONTRACT_VERSION_V2 {
        return Err(PeerContractError::Version(resp.v));
    }
    if resp.req_id != checks.expect_req_id {
        return Err(PeerContractError::Correlation {
            expected: checks.expect_req_id,
            got: resp.req_id,
        });
    }
    if let Some(last) = checks.expect_seq_gt {
        if resp.seq <= last {
            return Err(PeerContractError::Sequence {
                last: Some(last),
                got: resp.seq,
            });
        }
    }
    if resp.reason.trim().is_empty() {
        return Err(PeerContractError::Range(
            String::from("reason"),
            String::from("empty"),
        ));
    }
    let p = &resp.plan;
    // —— 身份绑定：逐字段相等（不得"事后改写身份使不匹配候选通过"）——
    let expect_sid = hex16(&binds.snapshot_id);
    if p.snapshot_id_hex != expect_sid {
        return Err(PeerContractError::Range(
            String::from("plan.snapshot_id_hex"),
            String::from("not the bound generation"),
        ));
    }
    if p.catalog_version != binds.catalog_version {
        return Err(PeerContractError::Range(
            String::from("plan.catalog_version"),
            String::from("mismatch"),
        ));
    }
    if p.horizon_start_ms != binds.horizon_start_ms
        || p.step_ns != binds.step_ns
        || p.step_count != binds.step_count
    {
        return Err(PeerContractError::Range(
            String::from("plan.horizon"),
            String::from("mismatch"),
        ));
    }
    match p.solver_status.as_str() {
        "optimal" | "suboptimal" => {}
        _ => {
            return Err(PeerContractError::Range(
                String::from("plan.solver_status"),
                String::from("not in closed set"),
            ))
        }
    }
    if !is_hex(&p.model_digest_hex, 64) || !is_hex(&p.input_digest_hex, 64) {
        return Err(PeerContractError::Range(
            String::from("plan.digest_hex"),
            String::from("expect 64 lowercase hex"),
        ));
    }
    if !p.objective_value.is_finite() || !p.relative_gap.is_finite() || p.relative_gap < 0.0 {
        return Err(PeerContractError::Range(
            String::from("plan.objective"),
            String::from("non-finite or negative gap"),
        ));
    }
    if p.target_device.trim().is_empty() {
        return Err(PeerContractError::Range(
            String::from("plan.target_device"),
            String::from("empty"),
        ));
    }
    if p.assignments.is_empty() || p.assignments.len() > PEER_PLAN_MAX_ASSIGNMENTS {
        return Err(PeerContractError::Range(
            String::from("plan.assignments"),
            String::from("empty or exceeds cap"),
        ));
    }
    // 轨迹完整性：每资产恰好 step_count 步、步序唯一且 ∈ [1, step_count]。
    let mut seen: Vec<(u64, u32)> = Vec::with_capacity(p.assignments.len());
    let mut counts: Vec<(u64, u32)> = Vec::new();
    for a in &p.assignments {
        if a.asset_id == 0 {
            return Err(PeerContractError::Range(
                String::from("plan.assignments[].asset_id"),
                String::from("zero"),
            ));
        }
        if a.step_index == 0 || a.step_index > p.step_count {
            return Err(PeerContractError::Range(
                String::from("plan.assignments[].step_index"),
                String::from("out of [1, step_count]"),
            ));
        }
        if !a.power_kw.is_finite() {
            return Err(PeerContractError::Range(
                String::from("plan.assignments[].power_kw"),
                String::from("non-finite"),
            ));
        }
        // 预算域：越界即拒（**不钳制**——钳制后计划已不是被求解的那份）。
        if a.power_kw.abs() > checks.budget.max_power_kw {
            return Err(PeerContractError::Range(
                String::from("plan.assignments[].power_kw"),
                String::from("exceeds budget"),
            ));
        }
        if seen.contains(&(a.asset_id, a.step_index)) {
            return Err(PeerContractError::Range(
                String::from("plan.assignments"),
                String::from("duplicate asset/step"),
            ));
        }
        seen.push((a.asset_id, a.step_index));
        match counts.iter_mut().find(|(id, _)| *id == a.asset_id) {
            Some((_, n)) => *n += 1,
            None => counts.push((a.asset_id, 1)),
        }
    }
    if counts.iter().any(|(_, n)| *n != p.step_count) {
        return Err(PeerContractError::Range(
            String::from("plan.assignments"),
            String::from("incomplete trajectory (per-asset steps != step_count)"),
        ));
    }
    if p.ttl_secs == 0 || p.ttl_secs > checks.budget.ttl_secs {
        return Err(PeerContractError::Range(
            String::from("plan.ttl_secs"),
            String::from("out of [1, budget.ttl_secs]"),
        ));
    }
    Ok(())
}

/// 小写 hex 长度判定（严格：长度 + 字符集）。
fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 16B → 32 小写 hex（绑定比对单源）。
pub fn hex16(id: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in id {
        s.push_str(&alloc::format!("{b:02x}"));
    }
    s
}

/// 32B → 64 小写 hex（摘要字段单源）。
pub fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&alloc::format!("{b:02x}"));
    }
    s
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
    #![allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）

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

    // ========================================================================
    // 蓝图 70.10 WP-70.10.1/70.10.2：结构化全计划 v2 校验（正例 + 逐类负例）
    // ========================================================================

    fn binds() -> PlanBinds {
        PlanBinds {
            snapshot_id: [0x5A; 16],
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 60_000_000_000,
            step_count: 4,
        }
    }

    fn budget_v2() -> PeerBudget {
        PeerBudget {
            max_power_kw: 500.0,
            ttl_secs: 60,
        }
    }

    fn checks_v2(seq_gt: Option<u64>) -> ResponseChecks<'static> {
        // 预算为常量借用（'static 生命周期便于测试构造）。
        static BUDGET: PeerBudget = PeerBudget {
            max_power_kw: 500.0,
            ttl_secs: 60,
        };
        ResponseChecks {
            expect_req_id: 7,
            expect_seq_gt: seq_gt,
            budget: &BUDGET,
        }
    }

    fn assignment(asset: u64, step: u32, power: f64) -> PeerAssignment {
        PeerAssignment {
            asset_id: asset,
            device_id: 2,
            point_id: 10,
            unit_code: 6,
            step_index: step,
            power_kw: power,
        }
    }

    fn plan_v2_full() -> PeerPlanV2 {
        let mut assignments = Vec::new();
        for asset in [1u64, 10] {
            for step in 1..=4u32 {
                assignments.push(assignment(asset, step, -20.0 + step as f64));
            }
        }
        PeerPlanV2 {
            snapshot_id_hex: hex16(&[0x5A; 16]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 60_000_000_000,
            step_count: 4,
            solver_status: String::from("optimal"),
            model_digest_hex: hex32(&[0x11; 32]),
            input_digest_hex: hex32(&[0x22; 32]),
            objective_value: -12.5,
            relative_gap: 0.0,
            elapsed_ms: 12,
            assignments,
            target_device: String::from("pcs"),
            ttl_secs: 60,
        }
    }

    fn resp_v2(plan: PeerPlanV2) -> PeerResponseV2 {
        PeerResponseV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: 7,
            seq: 1,
            plan,
            reason: String::from("backend=planner status=optimal"),
        }
    }

    fn json_v2(plan: PeerPlanV2) -> String {
        serde_json::to_string(&resp_v2(plan)).expect("encode")
    }

    /// 正例：2 资产 × 4 时段全轨迹通过（逐字段绑定一致）。
    #[test]
    fn v2_full_plan_accepted_and_bound() {
        let json = json_v2(plan_v2_full());
        let r = parse_and_validate_v2(&json, &checks_v2(None), &binds()).expect("accept");
        assert_eq!(r.plan.assignments.len(), 8);
        assert_eq!(r.plan.solver_status, "optimal");
    }

    /// 跨代快照（身份不再逐字段相等）→ 拒。
    #[test]
    fn v2_cross_generation_snapshot_rejected() {
        let mut plan = plan_v2_full();
        plan.snapshot_id_hex = hex16(&[0x77; 16]);
        assert!(parse_and_validate_v2(&json_v2(plan), &checks_v2(None), &binds()).is_err());
    }

    /// 目录版本 / 时域不符 → 拒（时域错位是最危险的静默错绑）。
    #[test]
    fn v2_catalog_and_horizon_mismatch_rejected() {
        let mut p1 = plan_v2_full();
        p1.catalog_version = 4;
        assert!(parse_and_validate_v2(&json_v2(p1), &checks_v2(None), &binds()).is_err());
        let mut p2 = plan_v2_full();
        p2.horizon_start_ms += 1;
        assert!(parse_and_validate_v2(&json_v2(p2), &checks_v2(None), &binds()).is_err());
        let mut p3 = plan_v2_full();
        p3.step_count = 3;
        assert!(parse_and_validate_v2(&json_v2(p3), &checks_v2(None), &binds()).is_err());
    }

    /// 轨迹不完整（某资产缺步）→ 拒（不得"取 first 或补默认值"）。
    #[test]
    fn v2_incomplete_trajectory_rejected() {
        let mut plan = plan_v2_full();
        plan.assignments
            .retain(|a| !(a.asset_id == 10 && a.step_index == 3));
        assert!(parse_and_validate_v2(&json_v2(plan), &checks_v2(None), &binds()).is_err());
    }

    /// 越预算即拒（**不钳制**——钳制后计划已不是被求解的那份）。
    #[test]
    fn v2_over_budget_assignment_rejected_not_clamped() {
        let mut plan = plan_v2_full();
        plan.assignments[0].power_kw = 501.0;
        match parse_and_validate_v2(&json_v2(plan), &checks_v2(None), &binds()) {
            Err(PeerContractError::Range(f, _)) => assert_eq!(f, "plan.assignments[].power_kw"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 重复 (asset, step) 与越界步序 → 拒。
    #[test]
    fn v2_duplicate_and_out_of_range_step_rejected() {
        let mut p1 = plan_v2_full();
        p1.assignments[1].step_index = 1;
        assert!(parse_and_validate_v2(&json_v2(p1), &checks_v2(None), &binds()).is_err());
        let mut p2 = plan_v2_full();
        p2.assignments[0].step_index = 5;
        assert!(parse_and_validate_v2(&json_v2(p2), &checks_v2(None), &binds()).is_err());
    }

    /// 求解状态闭集 / 摘要形态 / 非有限数值 → 拒。
    #[test]
    fn v2_status_digest_and_finiteness_rejected() {
        let mut p1 = plan_v2_full();
        p1.solver_status = String::from("infeasible");
        assert!(parse_and_validate_v2(&json_v2(p1), &checks_v2(None), &binds()).is_err());
        let mut p2 = plan_v2_full();
        p2.model_digest_hex = String::from("zz");
        assert!(parse_and_validate_v2(&json_v2(p2), &checks_v2(None), &binds()).is_err());
        let mut p3 = plan_v2_full();
        p3.assignments[0].power_kw = f64::NAN;
        assert!(parse_and_validate_v2(&json_v2(p3), &checks_v2(None), &binds()).is_err());
        let mut p4 = plan_v2_full();
        p4.relative_gap = -1.0;
        assert!(parse_and_validate_v2(&json_v2(p4), &checks_v2(None), &binds()).is_err());
    }

    /// 分配条数上限（> 64）→ 拒；seq 重放 → 拒；req_id 不配对 → 拒。
    #[test]
    fn v2_cap_replay_and_correlation_rejected() {
        let mut plan = plan_v2_full();
        // 65 项：把某资产轨迹拉长到 65 步（同时也破坏轨迹完整性）。
        plan.step_count = 65;
        plan.assignments = (1..=65).map(|s| assignment(1, s, 1.0)).collect();
        assert!(parse_and_validate_v2(&json_v2(plan), &checks_v2(None), &binds()).is_err());
        // seq 重放（须严格递增）。
        let json = json_v2(plan_v2_full());
        assert!(matches!(
            parse_and_validate_v2(&json, &checks_v2(Some(1)), &binds()),
            Err(PeerContractError::Sequence { .. })
        ));
        // req_id 不配对。
        let bad = json.replace("\"req_id\":7", "\"req_id\":8");
        assert!(matches!(
            parse_and_validate_v2(&bad, &checks_v2(None), &binds()),
            Err(PeerContractError::Correlation { .. })
        ));
    }

    /// 计划级 inline 上限 = 64 的**边界**（蓝图 08 §9：64 上限与 65 拒绝
    /// 分别测试）。2026-09-18 R10 复查：上一轮把计划级上限误降到 8
    /// （与 DEC-70-13 ② 的**单批** 8 混同——单批上限落点在编译器
    /// `MAX_INTENTS_PER_BATCH`），本测红锁回归。
    #[test]
    fn v2_plan_cap_boundary_64_accepted_65_rejected() {
        // 16 资产 × 4 步 = 64（轨迹完整：每资产恰 4 步）→ 受理。
        let mut full = plan_v2_full();
        full.assignments.clear();
        for asset in 1..=16u64 {
            for step in 1..=4u32 {
                full.assignments
                    .push(assignment(asset, step, -20.0 + step as f64));
            }
        }
        assert_eq!(full.assignments.len(), 64);
        let json = json_v2(full.clone());
        let r = parse_and_validate_v2(&json, &checks_v2(None), &binds()).expect("64 必须受理");
        assert_eq!(r.plan.assignments.len(), 64);
        // 1 资产 × 65 步 = 65（轨迹完整：恰 65 步）→ 拒（超计划级上限）。
        let mut over = plan_v2_full();
        over.step_count = 65;
        over.assignments = (1..=65u32).map(|s| assignment(1, s, 1.0)).collect();
        assert!(
            parse_and_validate_v2(&json_v2(over), &checks_v2(None), &binds()).is_err(),
            "65 分配必须拒"
        );
    }

    /// 3 资源 × 4 时段 = 12 分配（蓝图 08 §208/§234 最低功能演示形）：
    /// 请求与应答两侧都必须放行——单批 8 由目标侧编译拆批承担
    /// （12 → 2 批），不是 peer wire 拒绝的理由。
    #[test]
    fn v2_demo_shape_three_assets_four_steps_passes_wire() {
        let spec = PeerAssetSpec {
            asset_id: 9,
            capacity_kwh: 50.0,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: 15.0,
            soc_min: 0.05,
            soc_max: 0.95,
            p_min_kw: -500.0,
            p_max_kw: 500.0,
            ramp_up_kw_s: 3.0,
            ramp_down_kw_s: 3.0,
            device_id: 9,
            point_id: 1014,
            unit_code: 6,
        };
        let mut req = PeerRequestV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: 7,
            ts_ms: 1_700_000_000_000,
            snapshot_id_hex: hex16(&[0x5A; 16]),
            provenance_hex: hex32(&[0x3D; 32]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 7_000_000_000,
            step_count: 4,
            assets: vec![spec.clone()],
            grid_asset_id: 0,
            objective_kind: String::from("min_cost"),
            prices_per_kwh: vec![0.5, 0.6, 0.4, 0.45],
            budget: budget_v2(),
        };
        // 请求侧：3 资产 × 4 步 = 12 ≤ 64 → encode 通过。
        req.assets = vec![
            PeerAssetSpec {
                asset_id: 1,
                ..spec.clone()
            },
            PeerAssetSpec {
                asset_id: 2,
                ..spec.clone()
            },
            spec,
        ];
        assert!(req.encode().is_ok(), "3×4=12 分配请求必须放行");
        // 应答侧：3 资产 × 4 步全轨迹通过（binds 与 fixture 同步 60s 步长
        // ——本测锁**条数**边界，时域窗由 engine 侧测试承载）。
        let mut demo = plan_v2_full();
        demo.assignments.clear();
        for asset in [1u64, 2, 9] {
            for step in 1..=4u32 {
                demo.assignments
                    .push(assignment(asset, step, -20.0 + step as f64));
            }
        }
        assert_eq!(demo.assignments.len(), 12);
        let r =
            parse_and_validate_v2(&json_v2(demo), &checks_v2(None), &binds()).expect("12 必须受理");
        assert_eq!(r.plan.assignments.len(), 12);
    }

    /// 请求 v2 闭合校验：自持 fixture 缺失（无资产/价格长度不符/越上限）
    /// → 拒（host 不得凭 host 自持配置补值）。
    #[test]
    fn v2_request_closed_validation() {
        let spec = PeerAssetSpec {
            asset_id: 1,
            capacity_kwh: 100.0,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: 60.0,
            soc_min: 0.05,
            soc_max: 0.95,
            p_min_kw: -500.0,
            p_max_kw: 500.0,
            ramp_up_kw_s: 3.0,
            ramp_down_kw_s: 3.0,
            device_id: 2,
            point_id: 10,
            unit_code: 6,
        };
        let req = PeerRequestV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: 7,
            ts_ms: 1_700_000_000_000,
            snapshot_id_hex: hex16(&[0x5A; 16]),
            provenance_hex: hex32(&[0x3D; 32]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 60_000_000_000,
            step_count: 4,
            assets: vec![spec.clone()],
            grid_asset_id: 0,
            objective_kind: String::from("min_cost"),
            prices_per_kwh: vec![0.5; 4],
            budget: budget_v2(),
        };
        assert!(req.encode().is_ok());
        let mut no_assets = req.clone();
        no_assets.assets.clear();
        assert!(no_assets.encode().is_err());
        let mut bad_prices = req.clone();
        bad_prices.prices_per_kwh.pop();
        assert!(bad_prices.encode().is_err());
        let mut too_many = req.clone();
        too_many.step_count = 32;
        too_many.prices_per_kwh = vec![0.5; 32];
        too_many.assets = vec![spec.clone(), spec.clone(), spec];
        assert!(too_many.encode().is_err(), "3×32 > 64 必须拒");
    }

    /// 2026-09-18 R10 三资源：3 资产 v2 请求 JSON 实测尺寸——必须
    /// ≤ PEER_REQUEST_MAX_LEN_V2 且 > 1024（旧 AI_REQUEST_MAX=1024 时代的
    /// 实弹失败面：超限即 fragment 恒败零请求）。尺寸漂移（新增字段）
    /// 越界时本测红。
    #[test]
    fn v2_three_asset_request_size_fits_relay_budget() {
        let spec = PeerAssetSpec {
            asset_id: 1,
            capacity_kwh: 100.0,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: 60.0,
            soc_min: 0.05,
            soc_max: 0.95,
            p_min_kw: -5.0,
            p_max_kw: 5.0,
            ramp_up_kw_s: 3.0,
            ramp_down_kw_s: 3.0,
            device_id: 1,
            point_id: 10,
            unit_code: 6,
        };
        let req = PeerRequestV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: 7,
            ts_ms: 1_700_000_000_000,
            snapshot_id_hex: hex16(&[0x5A; 16]),
            provenance_hex: hex32(&[0x3D; 32]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 7_000_000_000,
            step_count: 4,
            assets: vec![
                PeerAssetSpec {
                    asset_id: 1,
                    ..spec.clone()
                },
                PeerAssetSpec {
                    asset_id: 2,
                    ..spec.clone()
                },
                PeerAssetSpec {
                    asset_id: 3,
                    ..spec
                },
            ],
            grid_asset_id: 10,
            objective_kind: String::from("min_cost"),
            prices_per_kwh: vec![0.5, 0.6, 0.4, 0.45],
            budget: budget_v2(),
        };
        let json = req.encode().expect("encode");
        assert!(
            json.len() <= PEER_REQUEST_MAX_LEN_V2,
            "3 资产请求 {}B 超 wire 上限",
            json.len()
        );
        assert!(
            json.len() > 1024,
            "尺寸前提失效（{}B ≤1024——本测需重校准）",
            json.len()
        );
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
