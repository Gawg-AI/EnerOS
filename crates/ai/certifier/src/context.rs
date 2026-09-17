//! 蓝图 70.9 WP-70.9.2——**可信输入 → 认证上下文**装配（纯函数，无 I/O）。
//!
//! # 为什么单独成模块
//!
//! `CertifierModule::certify` 只接受 [`EvalContext`]：它必须**独立**从
//! 可信来源重建以下事实，而不是接受请求方塞进来的值：
//!
//! | 输入 | 来源（本模块唯一取法） |
//! |---|---|
//! | `snapshot` | 调用方交来的 `TrustedSnapshot`（由 svc-telemetry 冻结 blob 解码——身份在 `EvalContext::bind` 与计划逐字段比对） |
//! | `envelopes` | `ResourceRegistry::evaluate`（目录铭牌 ∩ 快照观测 ∩ 合同，**含快照身份与收紧证据**） |
//! | `batteries` | 目录铭牌（容量/效率）+ 快照 SOC 观测（初始能量） |
//! | `spec` | 目录控制合同（功率窗/爬坡）+ 安全覆盖（SOC 窗）——**硬约束非空**，缺任一即拒绝 |
//! | horizon / catalog / tolerance | 计划绑定字段与目录版本组 |
//!
//! 缺铭牌、缺合同、缺 SOC 观测、维度不符、目录版本不符 → 具名拒绝
//! （fail-closed）；**不从 host fixture 或默认值补**。

use alloc::vec::Vec;

use eneros_decision_contract::{
    CandidatePlan, Constraint, ConstraintKind, ObjectiveKind, ObjectiveSource, ObjectiveSpec,
    ReasonCode, SnapshotId,
};
use eneros_resource_registry::capability::CapabilityEnvelope;
use eneros_resource_registry::registry::{registry_from_catalog, ResourceRegistry};
use eneros_semantic_catalog::SemanticCatalog;
use eneros_snapshot_service::TrustedSnapshot;

use crate::EvalBattery;

/// 上下文装配拒绝（结构化；调用方零证书）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextReject {
    /// 计划无分配到可控资产。
    NoControllableAssets,
    /// 资产缺物理参数铭牌。
    MissingNameplate(u64),
    /// 资产缺控制合同。
    MissingContract(u64),
    /// 包络求交失败（原因字符串来自 registry）。
    Envelope(u64, alloc::string::String),
    /// 快照缺 SOC 观测（电池初始能量不可得）。
    MissingSoc(u32),
    /// 目录版本与计划绑定不符。
    CatalogVersion { plan: u64, catalog: u64 },
    /// 计划时域非法（零步长/零步数）。
    BadHorizon,
    /// 目标文档构造失败。
    Spec(alloc::string::String),
}

/// 认证上下文输入（owned；在 [`crate::CertifierModule::certify`] 前构造）。
pub struct CertifiedContextInputs {
    /// 逐资产能力包络（来自 `ResourceRegistry::evaluate`）。
    pub envelopes: Vec<(u64, CapabilityEnvelope)>,
    /// 电池参数（铭牌容量/效率 + 快照 SOC 初始能量）。
    pub batteries: Vec<EvalBattery>,
    /// 目标文档（目录派生；快照绑定 = 计划快照）。
    pub spec: ObjectiveSpec,
    /// 步长（ns）。
    pub horizon_step_ns: u64,
    /// 步数。
    pub horizon_step_count: u32,
    /// 时域起点（ms）。
    pub horizon_start_ms: u64,
    /// 目录合同版本。
    pub catalog_version: u64,
    /// 并网点资产（**仅当计划确实包含该资产**时非零——站点级约束必须
    /// 在计划的 PCC 轨迹上裁决；计划不含并网点时传 0，站点约束由
    /// [`crate::eval`] 以"无并网点无法校验"显式拒绝）。
    pub grid_asset_id: u64,
}

/// 由目录 + 冻结快照 + 计划装配认证上下文输入（纯函数）。
///
/// `registry` 应由 [`registry_from_catalog`] 在**同代目录**上装配（调用方
/// 保持目录所有权用于 `evaluate` 的 `&dyn SemanticCatalog`）。
pub fn context_inputs(
    plan: &CandidatePlan,
    snapshot: &TrustedSnapshot,
    catalog: &dyn SemanticCatalog,
    registry: &ResourceRegistry,
) -> Result<CertifiedContextInputs, ContextReject> {
    let vs = catalog.version_set();
    if vs.contract_version != plan.resource_catalog_version() {
        return Err(ContextReject::CatalogVersion {
            plan: plan.resource_catalog_version(),
            catalog: vs.contract_version,
        });
    }
    if plan.step_count() == 0 {
        return Err(ContextReject::BadHorizon);
    }
    // 有效步长单源（迁移窗单时段 ⇒ TTL 等效步长；与 `EvalContext::bind`
    // 同一判据，禁止两处各算）。
    let step_ns = crate::effective_step_ns(plan);
    // —— 资产集合（计划的分配面；去重保序）——
    let mut assets: Vec<u64> = Vec::new();
    for a in plan.assignments() {
        if !assets.contains(&a.asset_id) {
            assets.push(a.asset_id);
        }
    }
    if assets.is_empty() {
        return Err(ContextReject::NoControllableAssets);
    }
    let at_ns = snapshot.produced_at_ns;
    let resilience = eneros_resource_registry::ResilienceInputs::default();
    let mut envelopes: Vec<(u64, CapabilityEnvelope)> = Vec::new();
    let mut batteries: Vec<EvalBattery> = Vec::new();
    let mut hard: Vec<Constraint> = Vec::new();
    let mut grid_asset_id = 0u64;
    for asset_id in assets {
        let Some(np) = catalog.nameplate(asset_id) else {
            return Err(ContextReject::MissingNameplate(asset_id));
        };
        let Some(contract) = catalog.asset_control_contract(asset_id, vs.contract_version) else {
            return Err(ContextReject::MissingContract(asset_id));
        };
        let env = registry
            .evaluate(asset_id, at_ns, catalog, snapshot, &resilience)
            .map_err(|e| ContextReject::Envelope(asset_id, alloc::format!("{e}")))?;
        // 电池参数：容量/效率来自铭牌；初始能量 = 容量 × 快照 SOC。
        let soc = snapshot
            .asset_soc(catalog, asset_id)
            .ok_or(ContextReject::MissingSoc(
                catalog.asset_soc_point(asset_id).unwrap_or(0),
            ))?;
        batteries.push(EvalBattery {
            asset_id,
            capacity_kwh: np.capacity_kwh,
            eta_c: np.eta_c,
            eta_d: np.eta_d,
            initial_kwh: np.capacity_kwh * soc,
        });
        envelopes.push((asset_id, env));
        // —— 硬约束：储能可用 SOC 窗（**铭牌**单源）——
        //
        // 资产级功率/爬坡**不**入 spec：它们的权威面是包络
        // （`ResourceRegistry::evaluate` 已把合同范围求交进 `p_min/p_max`
        // 与 `ramp_*`，见 [`CertifierModule`] 第 3 节包络检查）；certifier
        // 对 `spec` 里的功率/爬坡约束按**站点（PCC）**语义裁决——重复
        // 塞入会被当成站点约束而去查并网点序列（误拒）。
        //
        // SOC 窗来自铭牌 `soc_min/soc_max`（储能可用 SOC 运行窗的物理
        // 来源）。**不得**用覆盖表（coverage = 遥测测量量程）顶替：那是
        // 量程而不是运行安全窗，用它会让 MinSoc/MaxSoc 退化为 [0,1]
        // 恒真（复查订正：原实现即此错，硬约束形同虚设）。
        let _ = contract;
        hard.push(Constraint {
            kind: ConstraintKind::MinSoc,
            bound: np.soc_min,
        });
        hard.push(Constraint {
            kind: ConstraintKind::MaxSoc,
            bound: np.soc_max,
        });
    }
    // 并网资产：站点级约束的锚——**必须**是本计划轨迹内的资产（否则站点
    // 约束会被拿去查一条不存在的序列，误报缺分配）。
    for a in catalog.asset_ids() {
        if !plan.assignments().iter().any(|x| x.asset_id == a) {
            continue;
        }
        if catalog
            .asset(a, vs.asset_version)
            .map(|x| x.kind == eneros_config_bundle::schema::catalog_kinds::ASSET_GRID_CONNECTION)
            .unwrap_or(false)
        {
            grid_asset_id = a;
            break;
        }
    }
    if hard.is_empty() {
        return Err(ContextReject::Spec(alloc::string::String::from(
            "no-hard-constraints",
        )));
    }
    // —— 目标文档（目录派生；快照绑定 = 计划快照）——
    let valid_from_ms = plan.horizon_start_ms();
    let valid_until_ms = plan.horizon_start_ms().saturating_add(plan.ttl_ms().max(1));
    let spec = ObjectiveSpec::builder(SnapshotId(plan.snapshot_id().0))
        .constraints(hard, Vec::new())
        .objective(
            ObjectiveKind::SelfConsume,
            1,
            ObjectiveSource::Contract,
            ReasonCode::PriceArbitrage,
        )
        .window(valid_from_ms, valid_until_ms)
        .build()
        .map_err(|e| ContextReject::Spec(alloc::format!("{e:?}")))?;
    let _ = grid_asset_id;
    Ok(CertifiedContextInputs {
        envelopes,
        batteries,
        spec,
        horizon_step_ns: step_ns,
        horizon_step_count: plan.step_count(),
        horizon_start_ms: plan.horizon_start_ms(),
        catalog_version: plan.resource_catalog_version(),
        grid_asset_id,
    })
}

/// 便捷装配：目录 → 注册表 → 上下文输入（装配方最常用组合）。
pub fn context_inputs_from_catalog(
    plan: &CandidatePlan,
    snapshot: &TrustedSnapshot,
    catalog: &dyn SemanticCatalog,
) -> Result<(CertifiedContextInputs, ResourceRegistry), ContextReject> {
    let (registry, _report) = registry_from_catalog(catalog, 64);
    let inputs = context_inputs(plan, snapshot, catalog, &registry)?;
    Ok((inputs, registry))
}
