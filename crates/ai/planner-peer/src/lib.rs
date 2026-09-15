//! 蓝图 68.5（v0.325.0）WP-68.5.1：host planner 分区第一形态。
//!
//! ADR-050 裁决 2 的部署落点：生产 planner 运行在**受控站内计算分区
//! （host）**，经既有 RemoteBrain peer 通道（F26 合同，MQTT QoS1）应答
//! 单值功率提案；目标侧决策链消费。本 crate 是可测核心（配置解析 →
//! 真 HiGHS 求解 → PeerResponse 组装）；传输面在 [`mqtt`] + bin。
//!
//! # 第一形态边界（执行卡 WP-68.5.0）
//!
//! - 提案被 F26 schema 压扁为单值 `power_kw`（正=放电，负=充电——与
//!   目标侧 `ProposedControlPlan` 同号）；结构化 CertifiedPlan 通道为
//!   后续卷候选；
//! - 上行仅 6 字段遥测投影：`soc_pct` 进模型初值，`digest_hex` 派生
//!   provenance/snapshot 证据链；ObjectiveSpec/模型规格 host 侧自持
//!   （`--config` toml）；
//! - 本 crate **不持有** protocol writer / Gateway key / seL4 capability
//!   （依赖面即证明：intent-contract/decision-contract/planner/
//!   solver-core/objective-engine/resource-registry + std）。
//!
//! # 偏离登记（执行卡附加裁决 5）
//!
//! `CompiledObjective` 经 objective-engine `test-util` fixture 缝构造：
//! 生产编译管线（签名 RuleBundle + TrustedSnapshot + FactSet）的输入是
//! **目标侧证据链产物**，host 第一形态无此输入源；digest 仍由
//! `digest_compiled` 只算不传（构造纪律与生产同）。ADR-050 求解门
//! （ToleranceProfile L2 只收 Optimal）全生效。
//!
//! # 诚实性
//!
//! - 求解失败/不可行/超时 ⇒ **不应答**（目标侧超时降级，fail-operational
//!   既有语义）+ `PLANNER-SKIP` 词形——绝不伪造应答；
//! - `reason` 前缀词形 `backend=planner status=<optimal|suboptimal>
//!   model=<64hex>`（WP-68.5.2 审计诚实化的对端约定，schema 不动）；
//! - 求解功率钳制到请求预算域内（包络构造已按预算收紧，此处兜底数值
//!   噪声——契约校验在目标侧仍是权威）。

use std::string::String;
use std::vec::Vec;

use eneros_decision_contract::objective::{
    Constraint, ConstraintKind, ObjectiveKind, ObjectiveSource,
};
use eneros_decision_contract::{DecisionRequest, ObjectiveSpec, ReasonCode, SnapshotId};
use eneros_intent_contract::peer::{PeerRequest, PeerResponse, PeerTelemetry};
use eneros_objective_engine::compiler::CompiledObjectiveParts;
use eneros_objective_engine::CompiledObjective;
use eneros_planner::builder::{BatterySpec, GridSpec, PlanInput};
use eneros_planner::{AssetBinding, Planner};
use eneros_resource_registry::capability::{Availability, CapabilityEnvelope, DeratingReason};

/// peer 通道 reason 前缀词形（WP-68.5.2 对端约定；目标侧
/// `remote_brain::backend` 单源解析本词形——两侧漂移即审计不可区分）。
pub const BACKEND_TAG: &str = "backend=planner";

// ============================================================================
// host 侧自持配置（--config toml）
// ============================================================================

/// 电池模型规格（host 自持；不依赖上行全量遥测）。
#[derive(Debug, Clone, PartialEq)]
pub struct BatteryConfig {
    /// 额定容量 kWh（>0）。
    pub capacity_kwh: f64,
    /// 充/放电效率 (0,1]。
    pub eta_c: f64,
    pub eta_d: f64,
    /// SOC 硬窗（[0,1]；Min ≤ Max）。
    pub soc_min: f64,
    pub soc_max: f64,
    /// 功率幅值上界 kW（>0；与请求预算取 min 进包络）。
    pub power_max_kw: f64,
}

/// 目标与地平线。
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveConfig {
    /// "min_cost" | "max_revenue"。
    pub kind: String,
    /// 每步电价 元/kWh（len == steps；有限）。
    pub prices_per_kwh: Vec<f64>,
    /// 步长纳秒（>0）。
    pub step_ns: u64,
    /// 步数（[1, 24]）。
    pub steps: u32,
}

/// planner-peer 服务配置（toml 全文解析产物）。
#[derive(Debug, Clone, PartialEq)]
pub struct PeerPlannerConfig {
    pub battery: BatteryConfig,
    /// 并网点额定 kW（>0；对称包络）。
    pub grid_rating_kw: f64,
    pub objective: ObjectiveConfig,
    /// 提案目标设备（进 PeerResponse.plan.target_device；授权对账按目标
    /// 侧点表裁决——缺省 pcs 与目标侧绑定一致）。
    pub target_device: String,
    /// host 求解预算毫秒（DecisionRequest.budget_ms；planner 以此设
    /// HiGHS time_limit 并留下游余量）。
    pub budget_ms: u32,
}

/// 配置错误（首错即返，全量拒绝不做部分装配）。
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn get_f64(t: &toml::Table, path: &str, key: &str, lo: f64, hi: f64) -> Result<f64, ConfigError> {
    let v = t
        .get(path)
        .ok_or_else(|| ConfigError(format!("missing [{path}]")))?;
    let raw = v
        .get(key)
        .ok_or_else(|| ConfigError(format!("missing {path}.{key}")))?;
    let x = raw
        .as_float()
        .ok_or_else(|| ConfigError(format!("{path}.{key} not float")))?;
    if !x.is_finite() || x <= lo || x > hi {
        return Err(ConfigError(format!("{path}.{key}={x} out of ({lo},{hi}]")));
    }
    Ok(x)
}

fn get_u64(t: &toml::Table, path: &str, key: &str, lo: u64, hi: u64) -> Result<u64, ConfigError> {
    let v = t
        .get(path)
        .ok_or_else(|| ConfigError(format!("missing [{path}]")))?;
    let raw = v
        .get(key)
        .ok_or_else(|| ConfigError(format!("missing {path}.{key}")))?;
    let x = raw
        .as_integer()
        .ok_or_else(|| ConfigError(format!("{path}.{key} not integer")))?;
    if x < 0 || (x as u64) < lo || (x as u64) > hi {
        return Err(ConfigError(format!("{path}.{key}={x} out of [{lo},{hi}]")));
    }
    Ok(x as u64)
}

fn get_str(t: &toml::Table, path: &str, key: &str) -> Result<String, ConfigError> {
    let v = t
        .get(path)
        .ok_or_else(|| ConfigError(format!("missing [{path}]")))?;
    let s = v
        .get(key)
        .ok_or_else(|| ConfigError(format!("missing {path}.{key}")))?;
    s.as_str()
        .map(String::from)
        .ok_or_else(|| ConfigError(format!("{path}.{key} not string")))
}

impl PeerPlannerConfig {
    /// 从 toml 全文解析（未知键在消费处自然忽略——本档为 host 自持配置，
    /// 非 target 签名 bundle 面；值域在此全检，解析失败不启动服务）。
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let root: toml::Value =
            toml::from_str(text).map_err(|e| ConfigError(format!("toml: {e}")))?;
        let t = root
            .as_table()
            .ok_or_else(|| ConfigError(String::from("root not table")))?;
        let battery = BatteryConfig {
            capacity_kwh: get_f64(t, "battery", "capacity_kwh", 0.0, 1e6)?,
            eta_c: get_f64(t, "battery", "eta_c", 0.0, 1.0)?,
            eta_d: get_f64(t, "battery", "eta_d", 0.0, 1.0)?,
            soc_min: get_f64(t, "battery", "soc_min", -0.0, 1.0)?,
            soc_max: get_f64(t, "battery", "soc_max", -0.0, 1.0)?,
            power_max_kw: get_f64(t, "battery", "power_max_kw", 0.0, 1e4)?,
        };
        if battery.soc_min > battery.soc_max {
            return Err(ConfigError(String::from("battery.soc_min > soc_max")));
        }
        let grid_rating_kw = get_f64(t, "grid", "rating_kw", 0.0, 1e6)?;
        let steps = get_u64(t, "objective", "steps", 1, 24)? as u32;
        let step_ns = get_u64(t, "objective", "step_ns", 1, 86_400_000_000_000)?;
        let kind = get_str(t, "objective", "kind")?;
        if kind != "min_cost" && kind != "max_revenue" {
            return Err(ConfigError(format!(
                "objective.kind={kind} 闭集外（min_cost|max_revenue）"
            )));
        }
        let prices = t
            .get("objective")
            .and_then(|o| o.get("prices_per_kwh"))
            .and_then(toml::Value::as_array)
            .ok_or_else(|| ConfigError(String::from("missing objective.prices_per_kwh")))?;
        let mut prices_per_kwh = Vec::with_capacity(prices.len());
        for p in prices {
            let x = p
                .as_float()
                .ok_or_else(|| ConfigError(String::from("price not float")))?;
            if !x.is_finite() || x < 0.0 {
                return Err(ConfigError(format!("price {x} 非法（须有限 ≥0）")));
            }
            prices_per_kwh.push(x);
        }
        if prices_per_kwh.len() != steps as usize {
            return Err(ConfigError(format!(
                "prices len {} != steps {steps}",
                prices_per_kwh.len()
            )));
        }
        let target_device = get_str(t, "plan", "target_device")?;
        if target_device.trim().is_empty() {
            return Err(ConfigError(String::from("plan.target_device empty")));
        }
        // 上限 15s 与 broker keepalive（30s）联动：求解同步阻塞主循环，
        // 超过 ping 半窗（15s）即可能错过 keepalive 节拍（68 复查：
        // 60s 档在求解期 0 ping → broker 按 1.5×keepalive 踢链，自愈靠
        // 重连但每次请求浪费——配置域直接拒自害档）。
        let budget_ms = get_u64(t, "plan", "budget_ms", 100, 15_000)? as u32;
        Ok(Self {
            battery,
            grid_rating_kw,
            objective: ObjectiveConfig {
                kind,
                prices_per_kwh,
                step_ns,
                steps,
            },
            target_device,
            budget_ms,
        })
    }
}

// ============================================================================
// 求解（真 HiGHS；ADR-050 后端裁决 1）
// ============================================================================

/// 求解失败（一切拒绝可观测；bin 打 PLANNER-SKIP 后不应答）。
#[derive(Debug, Clone, PartialEq)]
pub struct SolveError(pub String);

/// 求解元数据（reason 词形 + host 日志对账行素材；诚实标注面）。
#[derive(Debug, Clone, PartialEq)]
pub struct SolveMeta {
    /// 解状态词形（"optimal" | "suboptimal"——L2 门常态只产 optimal）。
    pub status_word: &'static str,
    /// 模型 digest hex（64 字符小写；目标侧经 `backend` 解析回 32B 进
    /// CandidatePlan.model_digest——审计对账键）。
    pub model_digest_hex: String,
    /// 求解耗时毫秒（host 日志对账）。
    pub elapsed_ms: u64,
    /// 目标值（元；host 日志对账）。
    pub objective_value: f64,
}

/// 处理一条 peer 请求：LP 建模 → 真 HiGHS 求解 → (power_kw, 元数据)。
///
/// `now_ms` = host 墙钟毫秒（DecisionRequest/plan 签发时刻）。
pub fn solve_request(
    cfg: &PeerPlannerConfig,
    planner: &mut Planner,
    backend: &mut eneros_solver_core::highs::HighsSolver,
    req: &PeerRequest,
    now_ms: u64,
) -> Result<(f64, SolveMeta), SolveError> {
    if !req.telemetry.soc_pct.is_finite() || !(0.0..=1.0).contains(&req.telemetry.soc_pct) {
        return Err(SolveError(format!("soc {} 域外", req.telemetry.soc_pct)));
    }
    // 包络功率上界 = min(host 规格, 请求预算)——越预算提案在目标侧必拒，
    // 此处在源头收紧（契约校验权威仍在目标侧）。
    let p = cfg.battery.power_max_kw.min(req.budget.max_power_kw);
    if p <= 0.0 {
        return Err(SolveError(String::from("预算域非正")));
    }
    // 证据链派生：digest_hex(64 hex = 32B) → provenance_root 全量；
    // snapshot_id 取前 16B（bind 契约只要求 host 侧 spec/plan 自洽——
    // F26 投影无独立快照面，登记执行卡）。
    let root = eneros_hex::decode_vec(&req.telemetry.digest_hex)
        .ok_or_else(|| SolveError(String::from("digest_hex 非 hex")))?;
    if root.len() != 32 {
        return Err(SolveError(format!("digest_hex {}B ≠ 32B", root.len())));
    }
    let mut sid = [0u8; 16];
    sid.copy_from_slice(&root[..16]);
    let snapshot_id = SnapshotId(sid);

    // SOC 窗取 host 规格与请求遥测的合流：初值 = soc×容量（钳进规格窗）。
    let cap = cfg.battery.capacity_kwh;
    let initial_kwh = (req.telemetry.soc_pct * cap).clamp(0.0, cap);

    let spec: ObjectiveSpec = ObjectiveSpec::builder(snapshot_id)
        .constraints(
            vec![
                Constraint {
                    kind: ConstraintKind::MinSoc,
                    bound: cfg.battery.soc_min,
                },
                Constraint {
                    kind: ConstraintKind::MaxSoc,
                    bound: cfg.battery.soc_max,
                },
            ],
            Vec::new(),
        )
        .objective(
            if cfg.objective.kind == "min_cost" {
                ObjectiveKind::MinCost
            } else {
                ObjectiveKind::MaxRevenue
            },
            1,
            ObjectiveSource::RuleBundle,
            ReasonCode::PriceArbitrage,
        )
        .window(0, u64::MAX)
        .build()
        .map_err(|e| SolveError(format!("spec: {e:?}")))?;

    // WP-64.3.2.1 fixture 缝（偏离登记见模块头）：digest 只算不传。
    let compiled = CompiledObjective::compile_fixture(CompiledObjectiveParts {
        spec: spec.clone(),
        prices_per_kwh: cfg.objective.prices_per_kwh.clone(),
        min_on_steps: 0,
        min_off_steps: 0,
        registry_generation: 1,
        bundle_version: 1,
        bundle_digest: [0x68; 32], // host 自持配置无外部 bundle——域标识常量
        horizon_step_ns: cfg.objective.step_ns,
        horizon_step_count: cfg.objective.steps,
    });

    // 请求 id 派生（16B 非零；host 侧 spec/plan 绑定自洽即可）。
    let mut request_id = [b'p'; 16];
    request_id[..8].copy_from_slice(&req.req_id.to_le_bytes());
    let dr = DecisionRequest::new(request_id, spec, 1, cfg.budget_ms, 1)
        .map_err(|e| SolveError(format!("decision-request: {e:?}")))?;

    let envelopes = vec![
        (
            1u64,
            CapabilityEnvelope {
                asset_id: 1,
                valid_from_ns: 0,
                valid_until_ns: u64::MAX,
                p_min_kw: -p,
                p_max_kw: p,
                q_min_kvar: 0.0,
                q_max_kvar: 0.0,
                energy_min_kwh: 0.0,
                energy_max_kwh: cap,
                ramp_up_kw_s: p * 100.0,
                ramp_down_kw_s: p * 100.0,
                min_on_ns: 0,
                min_off_ns: 0,
                availability: Availability::Full,
                derating_reason: DeratingReason::None,
                source_snapshot_id: sid,
                tightenings: Vec::new(),
                generation: 1,
            },
        ),
        (
            10u64,
            CapabilityEnvelope {
                asset_id: 10,
                p_min_kw: -cfg.grid_rating_kw,
                p_max_kw: cfg.grid_rating_kw,
                ..envelopes_pcc_template(sid)
            },
        ),
    ];
    let input = PlanInput {
        objective: &compiled,
        snapshot_id: sid,
        provenance_root: root.try_into().expect("32B"),
        registry_generation: 1,
        now_ms,
        site_node_id: 1,
        envelopes,
        batteries: vec![BatterySpec {
            asset_id: 1,
            capacity_kwh: cap,
            eta_c: cfg.battery.eta_c,
            eta_d: cfg.battery.eta_d,
            initial_kwh,
        }],
        renewables: Vec::new(),
        loads: Vec::new(),
        grid: Some(GridSpec { asset_id: 10 }),
        dispatchable: Vec::new(),
        // 绑定面 host 占位（solve_document 不消费 bindings；CandidatePlan
        // 装配不经本路径——提案单值出域）。
        bindings: vec![
            AssetBinding {
                asset_id: 1,
                device_id: 0,
                point_id: 0,
                unit_code: 0,
            },
            AssetBinding {
                asset_id: 10,
                device_id: 0,
                point_id: 0,
                unit_code: 0,
            },
        ],
    };

    let out = planner
        .solve_document(backend, &dr, &input)
        .map_err(|e| SolveError(format!("solve: {e:?}")))?;
    let status_word = match out.solver_status {
        eneros_decision_contract::SolveStatusKind::Optimal => "optimal",
        eneros_decision_contract::SolveStatusKind::Suboptimal => "suboptimal",
    };
    // 单值提案 = 电池资源第 1 步净功率（正=放电——与目标侧同号）。
    let power_kw = out
        .resources
        .iter()
        .find(|r| r.asset_id == 1)
        .ok_or_else(|| SolveError(String::from("解缺电池资源投影")))?
        .net_kw
        .first()
        .copied()
        .ok_or_else(|| SolveError(String::from("解空地平线")))?;
    if !power_kw.is_finite() {
        return Err(SolveError(String::from("解非有限")));
    }
    // 数值噪声兜底钳制（包络已在源头收紧；契约权威在目标侧）。
    let budget = req.budget.max_power_kw;
    let power_kw = power_kw.clamp(-budget, budget);
    let meta = SolveMeta {
        status_word,
        model_digest_hex: eneros_hex::encode_lower(&out.model_digest),
        elapsed_ms: out.elapsed_ms,
        objective_value: out.objective_value,
    };
    Ok((power_kw, meta))
}

fn envelopes_pcc_template(sid: [u8; 16]) -> CapabilityEnvelope {
    let mut e = CapabilityEnvelope {
        asset_id: 0,
        valid_from_ns: 0,
        valid_until_ns: u64::MAX,
        p_min_kw: 0.0,
        p_max_kw: 0.0,
        q_min_kvar: 0.0,
        q_max_kvar: 0.0,
        energy_min_kwh: 0.0,
        energy_max_kwh: 0.0,
        ramp_up_kw_s: 1e6,
        ramp_down_kw_s: 1e6,
        min_on_ns: 0,
        min_off_ns: 0,
        availability: Availability::Full,
        derating_reason: DeratingReason::None,
        source_snapshot_id: sid,
        tightenings: Vec::new(),
        generation: 1,
    };
    e.asset_id = 10;
    e
}

// ============================================================================
// 应答组装（WP-68.5.2 reason 词形约定）
// ============================================================================

/// 组装 PeerResponse（schema 闭集逐字段；`reason` 前缀 = BACKEND_TAG）。
pub fn build_response(
    req_id: u64,
    seq: u64,
    power_kw: f64,
    ttl_secs: u32,
    target_device: &str,
    meta: &SolveMeta,
) -> PeerResponse {
    PeerResponse {
        v: eneros_intent_contract::peer::PEER_CONTRACT_VERSION,
        req_id,
        seq,
        plan: eneros_intent_contract::peer::PeerPlan {
            power_kw,
            target_device: String::from(target_device),
            ttl_secs,
        },
        reason: format!(
            "{BACKEND_TAG} status={} model={} obj={:.4}",
            meta.status_word, meta.model_digest_hex, meta.objective_value
        ),
    }
}

// ============================================================================
// 蓝图 70.10 WP-70.10.2：**结构化全计划**求解（无第一步投影、无钳制）
// ============================================================================

/// v2 求解：**消费请求携带的目标侧事实**（快照身份/目录代次/时域/逐资产
/// 规格/价格），返回**全部资源 × 全部时段**的分配。
///
/// 与 v1 的差异（蓝图 70.10 的核心）：
/// - 不再从 host 自持配置取电池规格/价格/envelope——一切来自请求；
/// - 不再取 `net_kw.first()`（有损投影）；
/// - **不钳制**：越预算/越包络即 `SolveError`（不应答）——钳制后计划已
///   不是被求解证明覆盖的那一份，v2 拒绝"钳制后仍声称原证明"。
pub fn solve_request_v2(
    planner: &mut Planner,
    backend: &mut eneros_solver_core::highs::HighsSolver,
    req: &eneros_intent_contract::peer::PeerRequestV2,
    now_ms: u64,
) -> Result<(eneros_intent_contract::peer::PeerPlanV2, SolveMeta), SolveError> {
    use eneros_intent_contract::peer::{
        validate_request_v2, PeerAssignment, PeerPlanV2, PEER_CONTRACT_VERSION_V2,
    };
    validate_request_v2(req).map_err(|e| SolveError(format!("request: {e:?}")))?;

    let sid = eneros_hex::decode_vec(&req.snapshot_id_hex)
        .ok_or_else(|| SolveError(String::from("snapshot_id_hex 非 hex")))?;
    if sid.len() != 16 {
        return Err(SolveError(format!("snapshot_id {}B ≠ 16B", sid.len())));
    }
    let mut snapshot_id = [0u8; 16];
    snapshot_id.copy_from_slice(&sid);
    let root = eneros_hex::decode_vec(&req.provenance_hex)
        .ok_or_else(|| SolveError(String::from("provenance_hex 非 hex")))?;
    if root.len() != 32 {
        return Err(SolveError(format!("provenance {}B ≠ 32B", root.len())));
    }
    let mut provenance_root = [0u8; 32];
    provenance_root.copy_from_slice(&root);

    // —— 目标文档：由请求事实派生（硬约束 = 逐资产 SOC 窗）——
    let mut constraints: Vec<Constraint> = Vec::new();
    for a in &req.assets {
        constraints.push(Constraint {
            kind: ConstraintKind::MinSoc,
            bound: a.soc_min,
        });
        constraints.push(Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: a.soc_max,
        });
    }
    let spec = ObjectiveSpec::builder(SnapshotId(snapshot_id))
        .constraints(constraints, Vec::new())
        .objective(
            if req.objective_kind == "min_cost" {
                ObjectiveKind::MinCost
            } else {
                ObjectiveKind::MaxRevenue
            },
            1,
            ObjectiveSource::Contract,
            ReasonCode::PriceArbitrage,
        )
        .window(req.horizon_start_ms, u64::MAX)
        .build()
        .map_err(|e| SolveError(format!("spec: {e:?}")))?;

    let compiled = CompiledObjective::compile_fixture(CompiledObjectiveParts {
        spec: spec.clone(),
        prices_per_kwh: req.prices_per_kwh.clone(),
        min_on_steps: 0,
        min_off_steps: 0,
        registry_generation: req.catalog_version,
        bundle_version: 1,
        // 请求实际携带的证据摘要（不再是 host 常量占位）。
        bundle_digest: input_digest_v2(req),
        horizon_step_ns: req.step_ns,
        horizon_step_count: req.step_count,
    });

    let mut request_id = [b'p'; 16];
    request_id[..8].copy_from_slice(&req.req_id.to_le_bytes());
    let dr = DecisionRequest::new(
        request_id,
        spec,
        req.catalog_version,
        5_000,
        u64::from(req.step_count),
    )
    .map_err(|e| SolveError(format!("decision-request: {e:?}")))?;

    let mut envelopes: Vec<(u64, CapabilityEnvelope)> = Vec::new();
    let mut batteries: Vec<BatterySpec> = Vec::new();
    let mut bindings: Vec<AssetBinding> = Vec::new();
    for a in &req.assets {
        envelopes.push((
            a.asset_id,
            CapabilityEnvelope {
                asset_id: a.asset_id,
                valid_from_ns: 0,
                valid_until_ns: u64::MAX,
                p_min_kw: a.p_min_kw,
                p_max_kw: a.p_max_kw,
                q_min_kvar: 0.0,
                q_max_kvar: 0.0,
                energy_min_kwh: a.capacity_kwh * a.soc_min,
                energy_max_kwh: a.capacity_kwh * a.soc_max,
                ramp_up_kw_s: a.ramp_up_kw_s,
                ramp_down_kw_s: a.ramp_down_kw_s,
                min_on_ns: 0,
                min_off_ns: 0,
                availability: Availability::Full,
                derating_reason: DeratingReason::None,
                source_snapshot_id: snapshot_id,
                tightenings: Vec::new(),
                generation: req.catalog_version,
            },
        ));
        batteries.push(BatterySpec {
            asset_id: a.asset_id,
            capacity_kwh: a.capacity_kwh,
            eta_c: a.eta_c,
            eta_d: a.eta_d,
            initial_kwh: a.initial_kwh,
        });
        bindings.push(AssetBinding {
            asset_id: a.asset_id,
            device_id: a.device_id,
            point_id: a.point_id,
            unit_code: a.unit_code,
        });
    }
    if req.grid_asset_id != 0 && !req.assets.iter().any(|a| a.asset_id == req.grid_asset_id) {
        envelopes.push((
            req.grid_asset_id,
            CapabilityEnvelope {
                asset_id: req.grid_asset_id,
                p_min_kw: -req.budget.max_power_kw,
                p_max_kw: req.budget.max_power_kw,
                ..envelopes_pcc_template(snapshot_id)
            },
        ));
        bindings.push(AssetBinding {
            asset_id: req.grid_asset_id,
            device_id: 0,
            point_id: 0,
            unit_code: 0,
        });
    }

    let input = PlanInput {
        objective: &compiled,
        snapshot_id,
        provenance_root,
        registry_generation: req.catalog_version,
        now_ms,
        site_node_id: 1,
        envelopes,
        batteries,
        renewables: Vec::new(),
        loads: Vec::new(),
        grid: (req.grid_asset_id != 0).then_some(GridSpec {
            asset_id: req.grid_asset_id,
        }),
        dispatchable: Vec::new(),
        bindings,
    };
    let out = planner
        .solve_document(backend, &dr, &input)
        .map_err(|e| SolveError(format!("solve: {e:?}")))?;
    let status_word = match out.solver_status {
        eneros_decision_contract::SolveStatusKind::Optimal => "optimal",
        eneros_decision_contract::SolveStatusKind::Suboptimal => "suboptimal",
    };

    // —— 全量分配装配（资源 × 步；**不取 first**）——
    let mut assignments: Vec<PeerAssignment> = Vec::new();
    for a in &req.assets {
        let res = out
            .resources
            .iter()
            .find(|r| r.asset_id == a.asset_id)
            .ok_or_else(|| SolveError(format!("解缺资源 {}", a.asset_id)))?;
        if res.net_kw.len() != req.step_count as usize {
            return Err(SolveError(format!(
                "资源 {} 轨迹 {} 步 ≠ 时域 {} 步",
                a.asset_id,
                res.net_kw.len(),
                req.step_count
            )));
        }
        for (i, p) in res.net_kw.iter().enumerate() {
            if !p.is_finite() {
                return Err(SolveError(String::from("解非有限")));
            }
            assignments.push(PeerAssignment {
                asset_id: a.asset_id,
                device_id: a.device_id,
                point_id: a.point_id,
                unit_code: a.unit_code,
                step_index: (i as u32) + 1,
                power_kw: *p,
            });
        }
    }
    // 预算自检（**越界即拒，绝无钳制路径**——钳制后计划已不是被求解证明
    // 覆盖的那一份，见 [08] §7 WP-70.10.2 失败语义）。
    assert_within_budget(&assignments, req.budget.max_power_kw)?;
    if assignments.len() > eneros_intent_contract::peer::PEER_PLAN_MAX_ASSIGNMENTS {
        return Err(SolveError(format!(
            "分配 {} 条越上限 {}",
            assignments.len(),
            eneros_intent_contract::peer::PEER_PLAN_MAX_ASSIGNMENTS
        )));
    }

    let plan = PeerPlanV2 {
        snapshot_id_hex: eneros_intent_contract::peer::hex16(&snapshot_id),
        catalog_version: req.catalog_version,
        horizon_start_ms: req.horizon_start_ms,
        step_ns: req.step_ns,
        step_count: req.step_count,
        solver_status: String::from(status_word),
        model_digest_hex: eneros_hex::encode_lower(&out.model_digest),
        input_digest_hex: eneros_intent_contract::peer::hex32(&input_digest_v2(req)),
        objective_value: out.objective_value,
        relative_gap: out.relative_gap,
        elapsed_ms: out.elapsed_ms,
        assignments,
        target_device: String::from("pcs"),
        ttl_secs: req.budget.ttl_secs,
    };
    // 出站自检：目标侧同链预演（发一条必被拒的应答只会烧对端 bad 计数）。
    let binds = eneros_intent_contract::peer::PlanBinds {
        snapshot_id,
        catalog_version: req.catalog_version,
        horizon_start_ms: req.horizon_start_ms,
        step_ns: req.step_ns,
        step_count: req.step_count,
    };
    eneros_intent_contract::peer::validate_response_v2(
        &eneros_intent_contract::peer::PeerResponseV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: req.req_id,
            seq: 0,
            plan: plan.clone(),
            reason: String::from("self-check"),
        },
        &eneros_intent_contract::peer::ResponseChecks {
            expect_req_id: req.req_id,
            expect_seq_gt: None,
            budget: &req.budget,
        },
        &binds,
    )
    .map_err(|e| SolveError(format!("outbound-self-check: {e:?}")))?;

    let meta = SolveMeta {
        status_word,
        model_digest_hex: eneros_hex::encode_lower(&out.model_digest),
        elapsed_ms: out.elapsed_ms,
        objective_value: out.objective_value,
    };
    Ok((plan, meta))
}

/// 分配预算域自检（纯函数；v2 **不提供**钳制通路）。
///
/// 返回 Err 的调用方语义 = 不应答（目标侧超时降级），绝不"改小一点再发"。
pub fn assert_within_budget(
    assignments: &[eneros_intent_contract::peer::PeerAssignment],
    budget_kw: f64,
) -> Result<(), SolveError> {
    for a in assignments {
        if !a.power_kw.is_finite() {
            return Err(SolveError(format!(
                "资源 {} 步 {} 功率非有限",
                a.asset_id, a.step_index
            )));
        }
        if a.power_kw.abs() > budget_kw {
            return Err(SolveError(format!(
                "资源 {} 步 {} 功率 {:.3} 越预算 {:.3}（v2 不钳制）",
                a.asset_id, a.step_index, a.power_kw, budget_kw
            )));
        }
    }
    Ok(())
}

/// 输入摘要（域分隔；可被目标侧/复验方从请求事实重算）。
pub fn input_digest_v2(req: &eneros_intent_contract::peer::PeerRequestV2) -> [u8; 32] {
    use eneros_decision_contract::keyed::hmac_sha256;
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"eneros.peer-input.v2");
    buf.extend_from_slice(req.snapshot_id_hex.as_bytes());
    buf.extend_from_slice(&req.catalog_version.to_le_bytes());
    buf.extend_from_slice(&req.horizon_start_ms.to_le_bytes());
    buf.extend_from_slice(&req.step_ns.to_le_bytes());
    buf.extend_from_slice(&req.step_count.to_le_bytes());
    for p in &req.prices_per_kwh {
        buf.extend_from_slice(&p.to_le_bytes());
    }
    buf.extend_from_slice(req.objective_kind.as_bytes());
    // 无钥摘要域（输入摘要不是认证材料；域分隔前缀防跨上下文复用）。
    hmac_sha256(&[], &buf)
}

/// 组装 v2 应答（schema 闭集逐字段；`reason` 前缀 = BACKEND_TAG）。
pub fn build_response_v2(
    req_id: u64,
    seq: u64,
    plan: eneros_intent_contract::peer::PeerPlanV2,
    meta: &SolveMeta,
) -> eneros_intent_contract::peer::PeerResponseV2 {
    eneros_intent_contract::peer::PeerResponseV2 {
        v: eneros_intent_contract::peer::PEER_CONTRACT_VERSION_V2,
        req_id,
        seq,
        plan,
        reason: format!(
            "{BACKEND_TAG} status={} model={} obj={:.4}",
            meta.status_word, meta.model_digest_hex, meta.objective_value
        ),
    }
}

/// 请求摘要（host 日志对账行；WP-68.5.4 判据 1 抽查面）。
pub fn request_summary(req: &PeerRequest) -> String {
    let PeerTelemetry {
        soc_pct,
        voltage_v,
        current_a,
        frequency_hz,
        ..
    } = &req.telemetry;
    let req_id = req.req_id;
    let budget = req.budget.max_power_kw;
    let ttl = req.budget.ttl_secs;
    format!(
        "req_id={req_id} soc={soc_pct:.3} v={voltage_v:.0} i={current_a:.1} f={frequency_hz:.1} budget={budget:.1}kW ttl={ttl}s"
    )
}

// ============================================================================
// host 测试（t1 合同一致性 / t2 真 HiGHS / reason 词形）
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use eneros_intent_contract::peer::{
        budget as peer_budget, parse_and_validate, parse_and_validate_v2, PeerAssetSpec,
        PeerRequestV2, PlanBinds, ResponseChecks, PEER_CONTRACT_VERSION_V2,
    };
    use eneros_planner::tolerance::ToleranceProfile;

    use super::*;

    fn checks_for(
        expect_req_id: u64,
        expect_seq_gt: Option<u64>,
        b: &eneros_intent_contract::peer::PeerBudget,
    ) -> ResponseChecks<'_> {
        ResponseChecks {
            expect_req_id,
            expect_seq_gt,
            budget: b,
        }
    }

    const CONFIG: &str = r#"
[battery]
capacity_kwh = 10.0
eta_c = 1.0
eta_d = 1.0
soc_min = 0.1
soc_max = 0.9
power_max_kw = 5.0

[grid]
rating_kw = 20.0

[objective]
kind = "max_revenue"
prices_per_kwh = [1.0, 10.0]
step_ns = 3600000000000
steps = 2

[plan]
target_device = "pcs"
budget_ms = 5000
"#;

    fn cfg() -> PeerPlannerConfig {
        PeerPlannerConfig::parse(CONFIG).expect("config")
    }

    fn req(soc: f64) -> PeerRequest {
        PeerRequest::new(
            7,
            1_000,
            PeerTelemetry {
                soc_pct: soc,
                voltage_v: 380.0,
                current_a: 10.0,
                frequency_hz: 50.0,
                generated_at_ms: 900,
                digest_hex: "ab".repeat(32),
            },
            peer_budget(5.0, 60),
        )
    }

    fn fresh_solve(cfg: &PeerPlannerConfig, r: &PeerRequest) -> (f64, SolveMeta) {
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("HiGHS");
        solve_request(cfg, &mut planner, &mut hb, r, 1_000).expect("solve")
    }

    /// 配置解析：全字段 + 值域负例（闭集外 kind / prices 长度失配）。
    #[test]
    fn config_parse_and_rejects() {
        let c = cfg();
        assert_eq!(c.battery.capacity_kwh, 10.0);
        assert_eq!(c.objective.steps, 2);
        assert_eq!(c.objective.prices_per_kwh, vec![1.0, 10.0]);
        let bad_kind = CONFIG.replace("max_revenue", "max_profit");
        assert!(PeerPlannerConfig::parse(&bad_kind).is_err());
        let bad_len = CONFIG.replace("prices_per_kwh = [1.0, 10.0]", "prices_per_kwh = [1.0]");
        assert!(PeerPlannerConfig::parse(&bad_len).is_err());
        let inverted = CONFIG.replace("soc_min = 0.1", "soc_min = 0.95");
        assert!(PeerPlannerConfig::parse(&inverted).is_err());
    }

    /// t1（红测先行）：本服务应答必过 intent-contract peer 全量校验链
    /// （schema 闭集 / v==1 / seq 严格递增 / 功率域 / TTL 域）——目标侧
    /// `RemoteBrain::on_peer_message` 同一校验函数。
    #[test]
    fn t1_peer_contract_conformance() {
        let c = cfg();
        let (power, meta) = fresh_solve(&c, &req(0.6));
        let r1 = build_response(7, 100, power, 60, &c.target_device, &meta);
        let r2 = build_response(8, 101, power, 60, &c.target_device, &meta);
        let b = peer_budget(5.0, 60);
        let s1 = serde_json::to_string(&r1).expect("encode");
        let s2 = serde_json::to_string(&r2).expect("encode");
        let v1 = parse_and_validate(&s1, &checks_for(7, Some(99), &b)).expect("t1 第一帧过全链");
        assert_eq!(v1.plan.power_kw, power);
        assert_eq!(v1.plan.ttl_secs, 60);
        assert_eq!(v1.plan.target_device, "pcs");
        let _ = parse_and_validate(&s2, &checks_for(8, Some(100), &b)).expect("seq 递增过链");
        // 重放（seq 相等）必拒——服务侧 seq 状态单调的判据面。
        let dup = serde_json::to_string(&build_response(9, 101, power, 60, "pcs", &meta))
            .expect("encode");
        assert!(parse_and_validate(&dup, &checks_for(9, Some(101), &b)).is_err());
    }

    /// t1 伴随：应答 reason 前缀词形 = backend=planner（WP-68.5.2 对端
    /// 约定；目标侧审计可区分性的源头）。
    #[test]
    fn t1b_reason_carries_backend_tag() {
        let c = cfg();
        let (power, meta) = fresh_solve(&c, &req(0.6));
        let r = build_response(1, 1, power, 60, "pcs", &meta);
        assert!(r.reason.starts_with(BACKEND_TAG), "{}", r.reason);
        assert!(r.reason.contains("status=optimal"), "{}", r.reason);
        assert_eq!(meta.model_digest_hex.len(), 64);
    }

    // ========================================================================
    // 蓝图 70.10 WP-70.10.2：v2 全计划（真 HiGHS；无投影无钳制）
    // ========================================================================

    fn spec_v2(asset_id: u64, cap: f64, init: f64, p_max: f64) -> PeerAssetSpec {
        PeerAssetSpec {
            asset_id,
            capacity_kwh: cap,
            eta_c: 0.95,
            eta_d: 0.95,
            initial_kwh: init,
            soc_min: 0.05,
            soc_max: 0.95,
            p_min_kw: -p_max,
            p_max_kw: p_max,
            ramp_up_kw_s: 1e6,
            ramp_down_kw_s: 1e6,
            device_id: 2,
            point_id: 10,
            unit_code: 6,
        }
    }

    fn req_v2(assets: Vec<PeerAssetSpec>, steps: u32, prices: Vec<f64>) -> PeerRequestV2 {
        PeerRequestV2 {
            v: PEER_CONTRACT_VERSION_V2,
            req_id: 7,
            ts_ms: 1_700_000_000_000,
            snapshot_id_hex: eneros_intent_contract::peer::hex16(&[0x5A; 16]),
            provenance_hex: eneros_intent_contract::peer::hex32(&[0x3D; 32]),
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 3_600_000_000_000,
            step_count: steps,
            assets,
            grid_asset_id: 10,
            objective_kind: String::from("max_revenue"),
            prices_per_kwh: prices,
            budget: peer_budget(20.0, 60),
        }
    }

    /// t3（红测先行）：3 资源 × 4 时段全轨迹贯穿——非零条数 = 3×4 = 12、
    /// 每资产 4 步齐、过目标侧 v2 全量校验链（含逐字段绑定）。
    /// 变异靶：回到 `net_kw.first()` 投影（只出 1 条）→ 本测红。
    #[test]
    /// DEC-70-13 批上限（2026-09-15 收紧 64→8）后的 t3 重构：
    /// ① 超批（3 资产×4 步=12>8）→ 求解侧契约拒绝（上限单源 =
    ///    PEER_PLAN_MAX_ASSIGNMENTS）；
    /// ② 合规多资产（2 资产×4 步=8=上限值）→ 全量分配 + 目标侧同链校验。
    fn t3_v2_plan_cap_enforced_over_batch_limit() {
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("HiGHS");
        let req = req_v2(
            vec![
                spec_v2(1, 10.0, 5.0, 5.0),
                spec_v2(2, 8.0, 4.0, 4.0),
                spec_v2(3, 6.0, 3.0, 3.0),
            ],
            4,
            vec![1.0, 1.0, 10.0, 10.0],
        );
        let err = solve_request_v2(&mut planner, &mut hb, &req, 1_700_000_000_000)
            .expect_err("3 资产×4 步=12>8 必须被批上限拒绝（DEC-70-13）");
        assert!(
            format!("{err:?}").contains("exceeds plan cap") || format!("{err:?}").contains("Range")
        );
    }

    #[test]
    fn t3_v2_full_plan_two_assets_four_steps() {
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("HiGHS");
        let req = req_v2(
            vec![spec_v2(1, 10.0, 5.0, 5.0), spec_v2(2, 8.0, 4.0, 4.0)],
            4,
            vec![1.0, 1.0, 10.0, 10.0],
        );
        // now_ms 与请求时域同刻（生产：请求构造与求解同 tick）。
        let (plan, meta) =
            solve_request_v2(&mut planner, &mut hb, &req, 1_700_000_000_000).expect("solve v2");
        assert_eq!(meta.status_word, "optimal");
        assert_eq!(
            plan.assignments.len(),
            8,
            "2 资源 × 4 时段全量（=DEC-70-13 批上限）"
        );
        for asset in [1u64, 2] {
            let n = plan
                .assignments
                .iter()
                .filter(|a| a.asset_id == asset)
                .count();
            assert_eq!(n, 4, "资产 {asset} 轨迹必须完整");
            let steps: Vec<u32> = plan
                .assignments
                .iter()
                .filter(|a| a.asset_id == asset)
                .map(|a| a.step_index)
                .collect();
            assert_eq!(steps, vec![1, 2, 3, 4], "步序连续");
        }
        // 目标侧同一校验链（逐字段绑定 + 轨迹完整性 + 预算域）。
        let binds = PlanBinds {
            snapshot_id: [0x5A; 16],
            catalog_version: 3,
            horizon_start_ms: 1_700_000_000_000,
            step_ns: 3_600_000_000_000,
            step_count: 4,
        };
        let resp = build_response_v2(7, 5, plan.clone(), &meta);
        let json = resp.encode().expect("encode v2");
        let back = parse_and_validate_v2(&json, &checks_for(7, Some(4), &req.budget), &binds)
            .expect("目标侧全链通过");
        assert_eq!(back.plan.assignments.len(), 8);
        // 输入摘要可复算（复验方从请求事实重算即得）。
        assert_eq!(
            plan.input_digest_hex,
            eneros_intent_contract::peer::hex32(&input_digest_v2(&req))
        );
    }

    /// t3b：越预算**不钳制**（纯函数级，直击判据面）：越界/非有限一律
    /// 返回 Err——调用方语义即"不应答"，没有"改小一点再发"的通路。
    ///
    /// 变异靶：把判据换成 `clamp`（或删除 Err 分支）→ 本测红。
    #[test]
    fn t3b_v2_budget_guard_rejects_and_never_clamps() {
        use eneros_intent_contract::peer::PeerAssignment;
        let a = |step: u32, power: f64| PeerAssignment {
            asset_id: 1,
            device_id: 2,
            point_id: 10,
            unit_code: 6,
            step_index: step,
            power_kw: power,
        };
        assert!(assert_within_budget(&[a(1, -5.0), a(2, 20.0)], 20.0).is_ok());
        match assert_within_budget(&[a(1, 20.5)], 20.0) {
            Err(SolveError(ref s)) => assert!(s.contains("越预算"), "{s}"),
            other => panic!("越预算必须 Err（不得钳制）: {other:?}"),
        }
        assert!(assert_within_budget(&[a(1, -20.5)], 20.0).is_err());
        assert!(assert_within_budget(&[a(1, f64::NAN)], 20.0).is_err());
    }

    /// t2（红测先行）：真 HiGHS 求解——非 mock：状态 Optimal（L2 门）、
    /// model_digest 非零、确定性复现、功率落预算域且数值符合手算套利
    /// 方向（价升 ⇒ 后步放电为正）。
    #[test]
    fn t2_highs_real_solve() {
        let c = cfg();
        let r = req(0.5); // 5kWh 初值 = 容量一半
        let (power, meta) = fresh_solve(&c, &r);
        assert_eq!(meta.status_word, "optimal");
        assert!(
            meta.model_digest_hex != "00".repeat(32),
            "model_digest 非零"
        );
        assert!(power.abs() <= 5.0, "功率落预算域：{power}");
        // 手算（golden_two_step 同构）：终值 SOC ≥1kWh + 价升 ⇒ 第 1 步
        // 低价充电（净负）、第 2 步满放 +5；单值提案取第 1 步净功率。
        assert!(power < 0.0, "价升场景第 1 步应充电：{power}");
        let (power2, meta2) = fresh_solve(&c, &r);
        assert_eq!(power, power2, "同输入确定性");
        assert_eq!(meta.model_digest_hex, meta2.model_digest_hex);
        assert!(meta.elapsed_ms < 5_000, "host 求解预算内");
    }

    /// t2 变异靶：请求预算收紧 ⇒ 包络同步收紧（解不得越新预算）。
    #[test]
    fn t2b_budget_clamp_flows_into_envelope() {
        let c = cfg();
        let mut r = req(0.5);
        r.budget.max_power_kw = 2.0;
        let (power, _) = fresh_solve(&c, &r);
        assert!(power.abs() <= 2.0 + 1e-9, "包络随预算收紧：{power}");
    }

    /// SOC 域外请求拒绝（不应答语义）。
    #[test]
    fn soc_domain_violation_rejected() {
        let c = cfg();
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("HiGHS");
        let mut r = req(0.5);
        r.telemetry.soc_pct = 1.5;
        assert!(solve_request(&c, &mut planner, &mut hb, &r, 1_000).is_err());
        r.telemetry.soc_pct = f64::NAN;
        assert!(solve_request(&c, &mut planner, &mut hb, &r, 1_000).is_err());
    }

    /// t0（依赖面红线自证伴侣）：digest 派生链——非法/短 digest 拒绝。
    #[test]
    fn digest_derivation_validated() {
        let c = cfg();
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut hb = eneros_solver_core::highs::HighsSolver::new().expect("HiGHS");
        let mut r = req(0.5);
        r.telemetry.digest_hex = "zz".repeat(32);
        assert!(solve_request(&c, &mut planner, &mut hb, &r, 1_000).is_err());
        r.telemetry.digest_hex = "ab".repeat(8);
        assert!(solve_request(&c, &mut planner, &mut hb, &r, 1_000).is_err());
    }
}
