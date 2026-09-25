//! 蓝图 61 Phase 5 WP-5.1（[10] §2）：`PlanCompiler` —— CertifiedPlan
//! → 有序、可补偿、多设备 [`CommandBatch`] 的确定性编译器。
//!
//! 唯一不变量（[10] §2.1）：同一 `CertifiedPlan`、Catalog 与 Compiler
//! 版本必然生成相同 `batch_digest`；每个 [`CommandIntent`] 都能追溯到一个
//! 认证 assignment（intent_id = SHA-256(plan_sha ‖ asset ‖ step) 派生）和
//! 控制合同（编译期逐 assignment 查 [`PlanContractView`]，未知合同/无回读/
//! 越合同值域 ⇒ 拒绝并指名 assignment）。
//!
//! 编译规则（[10] §2.3）：
//! - schedule setpoint → typed 设备命令；单位转换走 `UnitCatalog`
//!   （SI 比例换算，量纲不兼容拒绝，不猜）；
//! - 小于分辨率的变化按 policy 合并，保留「未动作」证据
//!   （[`SkippedCommand`]）；
//! - 先执行使系统更安全/释放约束的动作（|目标| ≤ |当前| ⇒ 方向类 0），
//!   再执行增加出力/风险的动作——同方向类内 (asset, class, step) 全序；
//! - 同设备串行（依赖边链）；跨设备并行上限 = `policy.max_parallel`；
//! - 每命令有效期 = 计划时段窗 ∩ 证书窗，短于 `min_window_ms` 拒绝；
//! - 不可逆动作（`reversible == 0`）：补偿提示强制 None（= compensation
//!   id 0）并登记「需人工批准」；compiler 不修改计划「以更安全」，无法
//!   编译则拒绝并指名 assignment。
//!
//! 装配纪律：本模块不知道 Catalog 实现——合同事实经
//! [`PlanContractView`] seam 注入（生产 = control-services 装配从
//! SemanticCatalog 构建；空视图 fail-closed 全拒）。deviation 登记：
//! 能力包络时效不在 [`eneros_decision_contract::ResourceRegistry`] 合同
//! 面（唯一绑定权威是 `catalog_version` 对照）；目标侧多设备投递面待
//! WP-5.2 Saga 卷（本卷超界意图装配前整体拒绝，不半执行）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_decision_contract::{
    CertifiedPlan, CommandBatch, CommandIntent, CommandIntentParts, CompensationHint,
    ContractError, IntentId, PlanId, ResourceRegistry,
};
use eneros_telemetry_model::unit_catalog;
use sha2::Digest as _;

/// 控制合同编译面（[10] §2.2 输入「ResourceRegistry/ControlContract
/// generation」的合同半边；绑定半边 = `ResourceRegistry`）。
///
/// 值域/单位以绑定单位（合同单位）为准；`readback_point_id == 0` 的合同
/// 无法编译（[10] §2.4：无 readback 拒绝）。
pub trait PlanContractView {
    /// 资产控制合同；None = 未知合同（拒绝）。
    fn plan_contract(&self, asset_id: u64) -> Option<PlanContract>;
}

/// 单资产编译合同（`semantic_catalog::ControlContract` 的编译子集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanContract {
    /// 合同类（1=switch 2=setpoint；`config_bundle::schema::catalog`）。
    pub class: u16,
    /// 回读点（0 = 无回读 → 拒绝）。
    pub readback_point_id: u32,
    /// 可逆（0 = 不可逆：补偿强制 None + 需人工批准）。
    pub reversible: u8,
    /// 值域下界（绑定单位）。
    pub min: f64,
    /// 值域上界（绑定单位）。
    pub max: f64,
    /// 静置窗（毫秒；登记进意图前置检查语义，投递侧消费）。
    pub settle_ms: u32,
    /// 合同声明可补偿（true 且有已确认基线 ⇒ 补偿 = 回设基线值；
    /// 基线未知 ⇒ None——不发明补偿值，收敛交 Saga/对账）。
    pub compensable: bool,
}

/// 映射合同视图（装配/测试公用：`BTreeMap<u64, PlanContract>` 直用）。
#[derive(Debug, Clone, Default)]
pub struct MapContractView {
    contracts: BTreeMap<u64, PlanContract>,
}

impl MapContractView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, asset_id: u64, contract: PlanContract) -> &mut Self {
        self.contracts.insert(asset_id, contract);
        self
    }
}

impl PlanContractView for MapContractView {
    fn plan_contract(&self, asset_id: u64) -> Option<PlanContract> {
        self.contracts.get(&asset_id).copied()
    }
}

/// 当前 AppliedConfirmed 设备状态视图（[10] §2.2 输入；分辨率合并参照）。
pub trait AppliedStateView {
    /// 资产当前已确认功率（绑定单位；None = 无已确认基线）。
    fn applied_power_kw(&self, asset_id: u64) -> Option<f64>;
}

impl<F> AppliedStateView for F
where
    F: Fn(u64) -> Option<f64>,
{
    fn applied_power_kw(&self, asset_id: u64) -> Option<f64> {
        self(asset_id)
    }
}

/// fail-closed 空状态视图（全部资产无基线 → 不做分辨率合并）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAppliedState;

impl AppliedStateView for NoAppliedState {
    fn applied_power_kw(&self, _asset_id: u64) -> Option<f64> {
        None
    }
}

/// 单批 intent 上限（DEC-70-13 ②，2026-09-15 裁决：分配 = intent 1:1，
/// 传输容量墙 ≈11 留 3 余量给控制写/回读通道）。计划级 inline 上限 =
/// 64（蓝图 08 §9 / `PEER_PLAN_MAX_ASSIGNMENTS`）；超 8 分配的计划在
/// 编译期按资产链边界拆批，每批独立时域窗内推进（无跨批免认证）。
pub const MAX_INTENTS_PER_BATCH: usize = 8;

/// 编译器策略（[10] §2.2 输入「compiler policy」；全部参与
/// compiler_digest——策略漂移 = 摘要漂移 = mutation 可检）。
#[derive(Debug, Clone, PartialEq)]
pub struct CompilerPolicy {
    /// 编译器版本（行为变更必须递增）。
    pub compiler_version: u32,
    /// 单批命令上限（≤ `MAX_BATCH_SIZE`；生产默认 = [`MAX_INTENTS_PER_BATCH`]）。
    pub max_commands_per_batch: usize,
    /// 跨设备最大并行度（≥1；迁移期单设备 = 1 串行）。
    pub max_parallel: u16,
    /// 分辨率合并阈值（绑定单位；0 = 不合并）。
    pub merge_below_kw: f64,
    /// 最短命令有效期（毫秒；过短批拒绝）。
    pub min_window_ms: u64,
}

impl Default for CompilerPolicy {
    fn default() -> Self {
        Self {
            compiler_version: 2,
            max_commands_per_batch: MAX_INTENTS_PER_BATCH,
            max_parallel: 1,
            merge_below_kw: 0.0,
            min_window_ms: 1_000,
        }
    }
}

/// 未动作证据（[10] §2.3：合并保持证据，不静默丢弃）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkippedCommand {
    pub asset_id: u64,
    pub step_index: u32,
    /// 计划值（绑定单位，换算后）。
    pub power_kw: f64,
    pub reason: SkipReason,
}

/// 跳过原因（闭枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// 低于分辨率（|计划 − 已确认| < merge_below_kw）。
    BelowResolution,
    /// 步窗已整体过期（重编译时早于当前时刻的历史步——跳过不重发；
    /// 其执行证据在 journal，键不存在 = 永不重投）。
    WindowExpired,
}

/// 编译产物（[10] §2.2 输出侧；批序 = 执行序）。
#[derive(Debug, Clone)]
pub struct CompileOutcome {
    pub batches: Vec<CommandBatch>,
    pub skipped: Vec<SkippedCommand>,
    /// 不可逆意图（需人工批准/串行；[10] §2.3）。
    pub manual_approval: Vec<IntentId>,
    pub compiler_digest: [u8; 32],
}

/// 编译拒绝（[10] §2.3：拒绝并指名 assignment）。
#[derive(Debug, Clone, PartialEq)]
pub enum CompileReject {
    /// 计划与证书不绑定（digest/plan_id 不一致）。
    PlanCertMismatch,
    /// 计划在编译时刻已过期。
    ExpiredPlan,
    /// 未知合同（指名资产）。
    UnknownContract { asset_id: u64 },
    /// 无回读点（指名资产）。
    NoReadback { asset_id: u64 },
    /// 单位量纲不兼容（指名资产）。
    UnitIncompatible { asset_id: u64 },
    /// 值越合同域（指名 asset+step；不钳位——compiler 不修改计划）。
    ValueOutOfRange { asset_id: u64, step_index: u32 },
    /// 命令有效期不足（过短批拒绝）。
    WindowTooShort,
    /// 单资产依赖链超单批上限（链完整性不可让——fail-closed）。
    ChainExceedsBatch { asset_id: u64, len: usize },
    /// 合同批构造拒绝（decision-contract 校验面）。
    BatchReject(ContractError),
}

impl core::fmt::Display for CompileReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PlanCertMismatch => write!(f, "plan-cert mismatch"),
            Self::ExpiredPlan => write!(f, "plan expired"),
            Self::UnknownContract { asset_id } => write!(f, "unknown contract: asset {asset_id}"),
            Self::NoReadback { asset_id } => write!(f, "no readback: asset {asset_id}"),
            Self::UnitIncompatible { asset_id } => {
                write!(f, "unit incompatible: asset {asset_id}")
            }
            Self::ValueOutOfRange {
                asset_id,
                step_index,
            } => write!(
                f,
                "value out of contract range: asset {asset_id} step {step_index}"
            ),
            Self::WindowTooShort => write!(f, "intent window below policy minimum"),
            Self::ChainExceedsBatch { asset_id, len } => {
                write!(f, "asset {asset_id} chain len {len} exceeds batch cap")
            }
            Self::BatchReject(e) => write!(f, "batch: {e:?}"),
        }
    }
}

/// 编译中间命令（排序/分批内部表示）。
struct CompiledCommand {
    intent: CommandIntent,
    asset_id: u64,
    step_index: u32,
    /// 方向类（0 = 更安全/释放约束；1 = 增加出力/风险）。
    direction_class: u8,
}

/// 确定性编译入口（[10] §2 唯一不变量的生产者）。
///
/// `now_ms`：单调毫秒（计划/证书过期裁决与批 built_at 基准）。
/// `default_asset_id`：unbound assignment（asset_id == 0）的绑定完成点
/// （[06] §3.3：后端侧转换全零绑定，由 dispatch 编译期经 profile 注入；
/// 生产 = `profile.device_id`——与 PlannerView 装配同源；0 = 严格模式，
/// unbound 即拒绝）。
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn compile(
    cert: &CertifiedPlan,
    plan: &eneros_decision_contract::CandidatePlan,
    registry: &dyn ResourceRegistry,
    contracts: &dyn PlanContractView,
    applied: &dyn AppliedStateView,
    policy: &CompilerPolicy,
    now_ms: u64,
    default_asset_id: u64,
) -> Result<CompileOutcome, CompileReject> {
    // 0. 计划-证书绑定（compiler 不信任调用方配对）。
    // 62 v0.315.0 WP-62.1.2：派生单源 = decision-contract
    // `plan_wire_sha256`（原手写 SHA-256 删除）。
    let plan_sha: [u8; 32] = eneros_decision_contract::CandidatePlan::plan_wire_sha256(plan);
    if plan_sha != *cert.plan_sha256() || plan.id() != cert.plan_id() {
        return Err(CompileReject::PlanCertMismatch);
    }
    if !plan.is_valid_at(now_ms) || !cert.is_valid_at(now_ms) {
        return Err(CompileReject::ExpiredPlan);
    }
    let compiler_digest: [u8; 32] =
        sha2::Sha256::digest(compiler_digest_input(policy).as_slice()).into();

    // 1. 逐 assignment 编译（确定性序：排序后处理）。重复 (asset, step)
    // 在构造点即拒（WP-64.3.1.2 轨迹不变量；原 DuplicateStep 第二遍
    // 检查删除——CandidatePlan 构造/解码单通道已挡）。
    let mut assignments: Vec<_> = plan.assignments().to_vec();
    assignments.sort_by_key(|a| (a.asset_id, a.step_index));
    let mut ordered: Vec<CompiledCommand> = Vec::with_capacity(assignments.len());
    let mut skipped: Vec<SkippedCommand> = Vec::new();
    let mut manual_approval: Vec<IntentId> = Vec::new();
    for a in &assignments {
        // unbound 绑定完成（[06] §3.3；迁移 adapter 形态：assignment
        // 全零——asset/unit 一并由 dispatch profile 完成）。
        let unbound = a.asset_id == eneros_decision_contract::candidate::UNQUALIFIED_ASSET_ID;
        let asset_id = if unbound {
            default_asset_id
        } else {
            a.asset_id
        };
        if asset_id == 0 {
            return Err(CompileReject::UnknownContract { asset_id: 0 });
        }
        // 绑定（registry 唯一权威）+ 合同。
        let Some((device, point, unit)) = registry.asset_control_binding(asset_id) else {
            return Err(CompileReject::UnknownContract { asset_id });
        };
        let Some(c) = contracts.plan_contract(asset_id) else {
            return Err(CompileReject::UnknownContract { asset_id });
        };
        if c.readback_point_id == 0 {
            return Err(CompileReject::NoReadback { asset_id });
        }
        // 单位换算走 UnitCatalog（SI 比例；量纲不兼容拒绝，不猜）。
        // 单位码 0 = 未携带（生产者未声明）→ 取 registry 绑定单位（无换算）。
        // 原判据按 `unbound`（资产占位）分支：引擎在认证前解析占位资产后
        // 单位码仍是 0，故判据改按**单位码**本身（语义更直接，行为对旧
        // 未绑定形态逐字节不变）。
        let value_unit = if a.unit_code == 0 { unit } else { a.unit_code };
        let _ = unbound;
        let value = match (
            unit_catalog::from_code(value_unit),
            unit_catalog::from_code(unit),
        ) {
            (Some(from), Some(to)) if from.compatible_with(to) => {
                a.power_kw * from.si_scale_factor() / to.si_scale_factor()
            }
            _ => {
                return Err(CompileReject::UnitIncompatible {
                    asset_id: a.asset_id,
                })
            }
        };
        if !value.is_finite() || value < c.min || value > c.max {
            return Err(CompileReject::ValueOutOfRange {
                asset_id,
                step_index: a.step_index,
            });
        }
        // 分辨率合并（保留未动作证据）。
        if policy.merge_below_kw > 0.0 {
            if let Some(cur) = applied.applied_power_kw(asset_id) {
                if (value - cur).abs() < policy.merge_below_kw {
                    skipped.push(SkippedCommand {
                        asset_id,
                        step_index: a.step_index,
                        power_kw: value,
                        reason: SkipReason::BelowResolution,
                    });
                    continue;
                }
            }
        }
        // 时段窗：[horizon + (step-1)·step, horizon + step·step) ∩ 证书窗。
        // `step_ns == 0` = 迁移期单时段（adapter 语义）：窗 = 证书全窗
        // （覆盖预计执行窗——与 Phase 0 意图窗 [started, started+ttl] 一致）。
        let (valid_from, valid_until) = if plan.step_ns() == 0 {
            (cert.certified_at_ms(), cert.valid_until_ms())
        } else {
            let step_len_ms = plan.step_len_ms();
            let step_ms = u64::from(a.step_index.saturating_sub(1)).saturating_mul(step_len_ms);
            let raw_from = plan.horizon_start_ms().saturating_add(step_ms);
            let raw_to = raw_from.saturating_add(step_len_ms);
            (
                raw_from.max(cert.certified_at_ms()),
                raw_to.min(cert.valid_until_ms()),
            )
        };
        let span = valid_until.saturating_sub(valid_from);
        if valid_until <= valid_from || span < policy.min_window_ms {
            // 历史步（步窗已整体过去）→ 跳过留证，不阻塞后续步编译
            // （跨 tick 重编译时前序步必然走到这里；键不入编译 = 永不
            // 重投）。未来步窗过短 → 计划缺陷，拒绝。
            if valid_until <= now_ms {
                skipped.push(SkippedCommand {
                    asset_id,
                    step_index: a.step_index,
                    power_kw: value,
                    reason: SkipReason::WindowExpired,
                });
                continue;
            }
            return Err(CompileReject::WindowTooShort);
        }
        // 不可逆动作：补偿强制 None + 人工批准登记（[10] §2.3）。
        // 可补偿合同：补偿值 = 计划前最近已确认基线（不发明数值；
        // 基线未知 → None，收敛交 Saga/对账）。
        let (compensation, needs_manual) = if c.reversible == 0 {
            (CompensationHint::None, true)
        } else if c.compensable {
            let hint = applied
                .applied_power_kw(asset_id)
                .map(CompensationHint::RestorePowerKw)
                .unwrap_or(CompensationHint::None);
            (hint, false)
        } else {
            (CompensationHint::None, false)
        };
        let intent_id = intent_id_of(&plan_sha, asset_id, a.step_index);
        if needs_manual {
            manual_approval.push(intent_id);
        }
        // 方向类：|目标| ≤ |当前| ⇒ 先执行（更安全/释放约束）；无基线
        // 保守归类 1（零值目标仍算释放）。
        let direction_class = match applied.applied_power_kw(asset_id) {
            Some(cur) => u8::from(value.abs() > cur.abs()),
            None => u8::from(value != 0.0),
        };
        // 蓝图 65 WP-65.2：命名字段 Parts 构造（builder 退役）——
        // device/point/value/unit 在字段名上自证（同型相邻位换位面收口；
        // 行为锁 = 既有链路测试六件）。
        let intent = CommandIntent::from_parts(CommandIntentParts {
            intent_id,
            plan_id: *cert.plan_id(),
            plan_sha256: plan_sha,
            asset_id,
            device_id: device,
            point_id: point,
            power_kw: value,
            unit_code: unit,
            valid_from_ms: valid_from,
            valid_until_ms: valid_until,
            precondition: eneros_decision_contract::Precondition::None,
            compensation,
        })
        .map_err(CompileReject::BatchReject)?;
        ordered.push(CompiledCommand {
            intent,
            asset_id,
            step_index: a.step_index,
            direction_class,
        });
    }
    if ordered.is_empty() {
        // 全部合并/无 assignment：无命令批可编译——计划级拒绝（空批在
        // contract 层本就拒绝；这里指名「无动作」语义由调用方用 skipped
        // 呈现）。
        return Err(CompileReject::BatchReject(ContractError::MissingField(
            "intents",
        )));
    }

    // 2. 全序：方向类（先安全/释放约束）→ 资产 → 时段（确定性全序）。
    ordered.sort_by_key(|cmd| (cmd.direction_class, cmd.asset_id, cmd.step_index));

    // 3. 依赖边：同资产连续段串行链（DAG；跨设备不连边——并行上限管）。
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(ordered.len());
    for i in 1..ordered.len() {
        if ordered[i].asset_id == ordered[i - 1].asset_id {
            edges.push(((i - 1) as u32, i as u32));
        }
    }

    // 4. 分批（时间窗/故障域）：只能在资产链边界切割——链完整性不可让；
    // 单链超上限拒绝（fail-closed，[10] §2.4）。
    let cap = policy
        .max_commands_per_batch
        .min(eneros_decision_contract::MAX_BATCH_SIZE);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for i in 1..=ordered.len() {
        let boundary = i == ordered.len() || ordered[i].asset_id != ordered[start].asset_id;
        if boundary {
            let len = i - start;
            if len > cap {
                return Err(CompileReject::ChainExceedsBatch {
                    asset_id: ordered[start].asset_id,
                    len,
                });
            }
            runs.push((start, i));
            start = i;
        }
    }
    // 贪心装箱：连续 runs 合入同批直至上限。
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    let mut cs = 0usize;
    let mut ce = 0usize;
    for &(rs, re) in &runs {
        if ce == 0 {
            cs = rs;
            ce = re;
        } else if re - cs <= cap {
            ce = re;
        } else {
            chunks.push((cs, ce));
            cs = rs;
            ce = re;
        }
    }
    if ce > cs {
        chunks.push((cs, ce));
    }

    // 5. 逐批构造（批级元数据入 batch_digest）。
    let mut batches = Vec::with_capacity(chunks.len());
    for (batch_index, &(s, e)) in chunks.iter().enumerate() {
        let intents: Vec<CommandIntent> = ordered[s..e].iter().map(|c| c.intent).collect();
        let edges: Vec<(u32, u32)> = edges
            .iter()
            .filter(|&&(a, b)| (a as usize) >= s && (b as usize) < e)
            .map(|&(a, b)| (a - s as u32, b - s as u32))
            .collect();
        let batch_id = batch_id_of(&compiler_digest, &plan_sha, batch_index);
        let deadline = intents
            .iter()
            .map(|i| i.valid_until_ms())
            .max()
            .unwrap_or_default();
        let batch = CommandBatch::compile(batch_id, cert, registry)
            .intents(intents)
            .built_at(now_ms)
            .compile_meta(edges, policy.max_parallel, deadline, compiler_digest)
            .finish()
            .map_err(CompileReject::BatchReject)?;
        batches.push(batch);
    }

    Ok(CompileOutcome {
        batches,
        skipped,
        manual_approval,
        compiler_digest,
    })
}

/// intent_id 派生（确定性；可追溯 assignment：SHA-256(plan_sha ‖ asset ‖ step)）。
fn intent_id_of(plan_sha: &[u8; 32], asset_id: u64, step_index: u32) -> IntentId {
    let mut buf = Vec::with_capacity(48);
    buf.extend_from_slice(b"eneros.intent-id.v1");
    buf.extend_from_slice(plan_sha);
    buf.extend_from_slice(&asset_id.to_le_bytes());
    buf.extend_from_slice(&step_index.to_le_bytes());
    let d: [u8; 32] = sha2::Sha256::digest(&buf).into();
    IntentId(d[..16].try_into().expect("16B"))
}

/// batch_id 派生（确定性；compiler/plan/序号入码）。
fn batch_id_of(compiler_digest: &[u8; 32], plan_sha: &[u8; 32], index: usize) -> [u8; 16] {
    let mut buf = Vec::with_capacity(80);
    buf.extend_from_slice(b"eneros.batch-id.v1");
    buf.extend_from_slice(compiler_digest);
    buf.extend_from_slice(plan_sha);
    buf.extend_from_slice(&(index as u64).to_le_bytes());
    let d: [u8; 32] = sha2::Sha256::digest(&buf).into();
    d[..16].try_into().expect("16B")
}

/// compiler_digest 输入（策略全字段规范化——策略漂移可检）。
fn compiler_digest_input(policy: &CompilerPolicy) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(b"eneros.plan-compiler.v1");
    buf.extend_from_slice(&policy.compiler_version.to_le_bytes());
    buf.extend_from_slice(&(policy.max_commands_per_batch as u64).to_le_bytes());
    buf.extend_from_slice(&policy.max_parallel.to_le_bytes());
    buf.extend_from_slice(&policy.merge_below_kw.to_le_bytes());
    buf.extend_from_slice(&policy.min_window_ms.to_le_bytes());
    buf
}

/// 迁移期计划幂等键（engine 用）。
///
/// - `legacy_single == true`（adapter 单时段形态：`step_ns == 0` 且单一
///   assignment——由计划形态判定，跨 tick 重编译稳定）：correlation =
///   PlanId 字节（Phase 0 单 PCS 记录兼容）；
/// - 其余（多步/多资产）：correlation = SHA-256(intent_id) 派生——
///   intent_id 本身由 (plan_sha, asset, step) 确定性派生，因此同计划
///   重放、跨 tick 重编译（含前序步过期/合并跳过）键恒稳定。
pub fn plan_command_correlation(
    plan_id: &PlanId,
    intent_id: &IntentId,
    legacy_single: bool,
) -> [u8; 16] {
    if legacy_single {
        return plan_id.0;
    }
    let mut buf = Vec::with_capacity(56);
    buf.extend_from_slice(b"eneros.plan-command.v2");
    buf.extend_from_slice(&intent_id.0);
    let d: [u8; 32] = sha2::Sha256::digest(&buf).into();
    d[..16].try_into().expect("16B")
}

/// 计划是否为迁移期单命令形态（engine 判定入参；adapter 契约：
/// `step_ns == 0` + 单 assignment）。
pub fn is_legacy_single_command_plan(plan: &eneros_decision_contract::CandidatePlan) -> bool {
    plan.step_ns() == 0 && plan.assignments().len() == 1
}

/// R14-B：journal 身份对单源——(correlation, semantic digest) 是幂等
/// 真值的身份契约，两值必须同点派生（原 run_typed 未决普查/重放判定
/// 两处手写配对、恢复路径 correlation/digest 拆两函数分算，任何一处
/// 公式漂移即 replay 检测静默失效——2026-09-24 收编）。correlation-only
/// / digest-only 站点仍直用两原子（[`plan_command_correlation`] /
/// `semantic_command_digest`），免白付第二次 SHA-256。
pub fn intent_journal_identity(
    plan_id: &PlanId,
    intent: &eneros_decision_contract::CommandIntent,
    legacy_single: bool,
) -> ([u8; 16], [u8; 32]) {
    let correlation = plan_command_correlation(plan_id, intent.intent_id(), legacy_single);
    let digest = eneros_protocol_abstract::write::semantic_command_digest(
        u32::from(intent.device_id()),
        intent.point_id(),
        eneros_telemetry_model::command::CommandValue::ActivePowerKw(intent.power_kw()),
    );
    (correlation, digest)
}
