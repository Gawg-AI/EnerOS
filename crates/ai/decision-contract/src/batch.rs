//! `CommandBatch` — 有序命令批（[06] §3.3：构造函数只接受 `&CertifiedPlan`
//! 与 `&ResourceRegistry`）。
//!
//! `AuthorizedCommand` 不在本 crate：批内的每个 [`CommandIntent`] 仍须经
//! 唯一 ControlGateway 裁决才成为 AuthorizedCommand（逐意图，.40 前批大小
//! 生产=1）。

use alloc::vec::Vec;

use sha2::Digest as _;

use crate::certificate::CertifiedPlan;
use crate::error::ContractError;
use crate::ids::PlanId;
use crate::intent::CommandIntent;

/// 资源注册表只读视图（decision-contract 最小面；.35 卷 resource-registry
/// 落完整 ResourceRegistry——本 trait 是其合同前驱）。
pub trait ResourceRegistry {
    /// 资产是否存在且可控。
    fn asset_exists(&self, asset_id: u64) -> bool;
    /// 资产的 (device_id, point_id, unit_code) 控制绑定。
    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)>;
    /// 资源目录版本（与 CandidatePlan.resource_catalog_version 对照）。
    fn catalog_version(&self) -> u64;
}

/// 批大小上界（迁移期单 PCS=1；61 Phase 5 Saga 卷放宽并加 Saga journal）。
pub const MAX_BATCH_SIZE: usize = 64;

/// 批规范 wire 标识（batch_digest 编码前缀；[10] §2.2 batch 级字段）。
pub const BATCH_MAGIC: [u8; 4] = *b"CBAT";

/// fail-closed 空注册表（装配方未注入真实 Registry 时命令批编译拒绝：
/// `asset_exists` 恒 false、绑定恒 None——[08] §4.8「planner 输入只能
/// 查询 Registry」，无注册表 = 无计划编译）。
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyResourceRegistry;

impl ResourceRegistry for EmptyResourceRegistry {
    fn asset_exists(&self, _asset_id: u64) -> bool {
        false
    }

    fn asset_control_binding(&self, _asset_id: u64) -> Option<(u16, u32, u16)> {
        None
    }

    fn catalog_version(&self) -> u64 {
        0
    }
}

/// 有序命令批（意图序 = 编译器裁决序；逐意图经 Gateway 授权）。
///
/// [10] §2.2 批级编译元数据：依赖 DAG（意图索引对）、跨设备并行上限、
/// 批 deadline 与 compiler/batch 双摘要——同 CertifiedPlan + Catalog +
/// Compiler 版本必然产出相同 `batch_digest`（唯一不变量）。
#[derive(Debug, Clone, PartialEq)]
pub struct CommandBatch {
    batch_id: [u8; 16],
    /// 绑定证书（三向：batch → cert → plan）。
    plan_id: PlanId,
    plan_sha256: [u8; 32],
    certificate_id: crate::ids::CertificateId,
    intents: Vec<CommandIntent>,
    catalog_version: u64,
    built_at_ms: u64,
    /// 依赖边（before, after)——均为 intents 索引；无环（构造期校验）。
    dependency_edges: Vec<(u32, u32)>,
    /// 跨设备最大并行度（≥1；同设备恒串行——依赖边保证）。
    max_parallel: u16,
    /// 批 deadline（毫秒基准；0 = 未指定——由证书窗管辖）。
    deadline_ms: u64,
    /// 编译器摘要（SHA-256(compiler version ‖ policy)）。
    compiler_digest: [u8; 32],
    /// 批摘要（SHA-256([`CommandBatch::encode_v1`])——构造期计算）。
    batch_digest: [u8; 32],
}

/// 批编译字段袋（crate 内核：builder/`new` 共用校验通道，WP-64.3.1）。
pub(crate) struct CompiledBatchParts<'a> {
    pub batch_id: [u8; 16],
    pub cert: &'a CertifiedPlan,
    pub registry: &'a dyn ResourceRegistry,
    pub intents: Vec<CommandIntent>,
    pub built_at_ms: u64,
    pub dependency_edges: Vec<(u32, u32)>,
    pub max_parallel: u16,
    pub deadline_ms: u64,
    pub compiler_digest: [u8; 32],
}

/// 编译元数据组（依赖边/max_parallel/deadline/compiler_digest）。
pub type CompileMeta = (Vec<(u32, u32)>, u16, u64, [u8; 32]);

/// CommandBatch 语义组构造器（WP-64.3.1：identity 组于
/// [`CommandBatch::compile`] 声明；intents/built_at/compile_meta 三组
/// 必填）。
pub struct CommandBatchBuilder<'a> {
    batch_id: [u8; 16],
    cert: &'a CertifiedPlan,
    registry: &'a dyn ResourceRegistry,
    intents: Option<Vec<CommandIntent>>,
    built_at_ms: Option<u64>,
    compile_meta: Option<CompileMeta>,
}

impl CommandBatchBuilder<'_> {
    /// 意图组（非空、≤[`MAX_BATCH_SIZE`]、三向绑定过证）。
    pub fn intents(mut self, v: Vec<CommandIntent>) -> Self {
        self.intents = Some(v);
        self
    }

    /// 批构造时刻（证书有效期判据输入）。
    pub fn built_at(mut self, ms: u64) -> Self {
        self.built_at_ms = Some(ms);
        self
    }

    /// 编译元数据组（依赖边/max_parallel/deadline/compiler_digest——
    /// 全部入 `batch_digest` 码）。
    pub fn compile_meta(
        mut self,
        dependency_edges: Vec<(u32, u32)>,
        max_parallel: u16,
        deadline_ms: u64,
        compiler_digest: [u8; 32],
    ) -> Self {
        self.compile_meta = Some((dependency_edges, max_parallel, deadline_ms, compiler_digest));
        self
    }

    /// 组装并校验。
    pub fn finish(self) -> Result<CommandBatch, ContractError> {
        let intents = self.intents.ok_or(ContractError::MissingField("intents"))?;
        let built_at_ms = self
            .built_at_ms
            .ok_or(ContractError::MissingField("built_at_ms"))?;
        let (dependency_edges, max_parallel, deadline_ms, compiler_digest) = self
            .compile_meta
            .ok_or(ContractError::MissingField("compile_meta"))?;
        CommandBatch::compile_core(CompiledBatchParts {
            batch_id: self.batch_id,
            cert: self.cert,
            registry: self.registry,
            intents,
            built_at_ms,
            dependency_edges,
            max_parallel,
            deadline_ms,
            compiler_digest,
        })
    }
}

impl CommandBatch {
    /// 唯一构造入口：**必须**持证书与资源注册表视图。
    ///
    /// 校验（[06] §3.6 负例全集）：
    /// - 每个意图 plan_id/plan_sha256 与证书一致（`CertificatePlanMismatch`）；
    /// - 证书有效（未过期）；
    /// - 资产存在且控制绑定 (device, point, unit) 与意图一致
    ///   （`ScopeMismatch`）；
    /// - 目录版本与计划一致；
    /// - 批非空且不超界；intent_id 唯一。
    pub fn new(
        batch_id: [u8; 16],
        cert: &CertifiedPlan,
        registry: &dyn ResourceRegistry,
        intents: Vec<CommandIntent>,
        built_at_ms: u64,
    ) -> Result<Self, ContractError> {
        Self::compile_core(CompiledBatchParts {
            batch_id,
            cert,
            registry,
            intents,
            built_at_ms,
            dependency_edges: alloc::vec::Vec::new(),
            max_parallel: 1,
            deadline_ms: 0,
            compiler_digest: [0u8; 32],
        })
    }

    /// 编译器入口（[10] §2.2；WP-64.3.1 builder 化）：在
    /// [`CommandBatch::new`] 全部校验之上追加批级编译元数据校验（边索引
    /// 界内、无自环、无环、max_parallel ≥1）并计算 `batch_digest`。
    ///
    /// `deadline_ms = 0` 表示未指定（批窗由证书有效期管辖）。
    pub fn compile<'a>(
        batch_id: [u8; 16],
        cert: &'a CertifiedPlan,
        registry: &'a dyn ResourceRegistry,
    ) -> CommandBatchBuilder<'a> {
        CommandBatchBuilder {
            batch_id,
            cert,
            registry,
            intents: None,
            built_at_ms: None,
            compile_meta: None,
        }
    }

    pub(crate) fn compile_core(
        CompiledBatchParts {
            batch_id,
            cert,
            registry,
            intents,
            built_at_ms,
            dependency_edges,
            max_parallel,
            deadline_ms,
            compiler_digest,
        }: CompiledBatchParts<'_>,
    ) -> Result<Self, ContractError> {
        if intents.is_empty() {
            return Err(ContractError::MissingField("intents"));
        }
        if intents.len() > MAX_BATCH_SIZE {
            return Err(ContractError::LimitExceeded("batch"));
        }
        if !cert.is_valid_at(built_at_ms) {
            return Err(ContractError::InvalidWindow);
        }
        if registry.catalog_version() != cert.constraint_catalog_version() {
            return Err(ContractError::ScopeMismatch("catalog_version"));
        }
        if max_parallel == 0 {
            return Err(ContractError::OutOfRange("max_parallel"));
        }
        // 依赖边：索引界内 + 无自环（环检测在绑定校验后统一做——对全
        // 部意图索引，含编译器显式边）。
        for &(a, b) in &dependency_edges {
            if a as usize >= intents.len() || b as usize >= intents.len() {
                return Err(ContractError::ScopeMismatch("edge.index"));
            }
            if a == b {
                return Err(ContractError::Malformed("edge.self-loop"));
            }
        }
        let mut seen: Vec<[u8; 16]> = Vec::with_capacity(intents.len());
        for i in &intents {
            // 三向绑定。
            if i.plan_id() != cert.plan_id() || i.plan_sha256() != cert.plan_sha256() {
                return Err(ContractError::CertificatePlanMismatch);
            }
            if !seen.contains(&i.intent_id().0) {
                seen.push(i.intent_id().0);
            } else {
                return Err(ContractError::Malformed("duplicate_intent_id"));
            }
            // 资源注册表 scope 校验。
            if !registry.asset_exists(i.asset_id()) {
                return Err(ContractError::ScopeMismatch("asset"));
            }
            match registry.asset_control_binding(i.asset_id()) {
                Some((device, point, unit)) => {
                    if device != i.device_id() || point != i.point_id() || unit != i.unit_code() {
                        return Err(ContractError::ScopeMismatch("binding"));
                    }
                }
                None => return Err(ContractError::ScopeMismatch("no-binding")),
            }
            // 意图窗口 ⊆ 证书窗口。
            if i.valid_until_ms() > cert.valid_until_ms() {
                return Err(ContractError::InvalidWindow);
            }
        }
        Self::assert_acyclic(intents.len(), &dependency_edges)?;
        let mut batch = Self {
            batch_id,
            plan_id: *cert.plan_id(),
            plan_sha256: *cert.plan_sha256(),
            certificate_id: *cert.certificate_id(),
            intents,
            catalog_version: registry.catalog_version(),
            built_at_ms,
            dependency_edges,
            max_parallel,
            deadline_ms,
            compiler_digest,
            batch_digest: [0u8; 32],
        };
        batch.batch_digest = sha2::Sha256::digest(batch.encode_v1().as_slice()).into();
        Ok(batch)
    }

    /// DAG 无环断言（Kahn 拓扑计数；残余即有环）。
    fn assert_acyclic(n: usize, edges: &[(u32, u32)]) -> Result<(), ContractError> {
        let mut indegree = alloc::vec![0usize; n];
        let mut adj: alloc::collections::BTreeMap<u32, alloc::vec::Vec<u32>> =
            alloc::collections::BTreeMap::new();
        for &(a, b) in edges {
            indegree[b as usize] += 1;
            adj.entry(a).or_default().push(b);
        }
        let mut queue: alloc::collections::BTreeSet<u32> = (0..n as u32)
            .filter(|i| indegree[*i as usize] == 0)
            .collect();
        let mut visited = 0usize;
        while let Some(v) = queue.pop_first() {
            visited += 1;
            if let Some(next) = adj.get(&v) {
                for &w in next {
                    indegree[w as usize] -= 1;
                    if indegree[w as usize] == 0 {
                        queue.insert(w);
                    }
                }
            }
        }
        if visited != n {
            return Err(ContractError::Malformed("dependency-cycle"));
        }
        Ok(())
    }

    pub fn batch_id(&self) -> &[u8; 16] {
        &self.batch_id
    }
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn plan_sha256(&self) -> &[u8; 32] {
        &self.plan_sha256
    }
    pub fn certificate_id(&self) -> &crate::ids::CertificateId {
        &self.certificate_id
    }
    pub fn intents(&self) -> &[CommandIntent] {
        &self.intents
    }
    pub fn catalog_version(&self) -> u64 {
        self.catalog_version
    }
    pub fn built_at_ms(&self) -> u64 {
        self.built_at_ms
    }
    pub fn dependency_edges(&self) -> &[(u32, u32)] {
        &self.dependency_edges
    }
    pub fn max_parallel(&self) -> u16 {
        self.max_parallel
    }
    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
    pub fn compiler_digest(&self) -> &[u8; 32] {
        &self.compiler_digest
    }
    pub fn batch_digest(&self) -> &[u8; 32] {
        &self.batch_digest
    }

    /// 批规范编码（batch_digest 输入；字段序固定，reserved 全零）。
    ///
    /// 意图段逐条 [`CommandIntent::encode_v1`]（96B 定长，含 plan 绑定
    /// 三元组）；批级字段（边/并行/deadline/compiler 摘要）全部入码——
    /// 任何元数据漂移都改变 batch_digest（mutation 可检）。
    pub fn encode_v1(&self) -> Vec<u8> {
        use crate::wire::{put_u16, put_u32, put_u64};
        let mut out = Vec::with_capacity(64 + self.intents.len() * crate::intent::INTENT_WIRE_LEN);
        out.extend_from_slice(&BATCH_MAGIC);
        put_u16(&mut out, crate::CONTRACT_WIRE_V1);
        put_u16(&mut out, 0); // flags/reserved
        out.extend_from_slice(&self.batch_id);
        out.extend_from_slice(&self.plan_id.0);
        out.extend_from_slice(&self.plan_sha256);
        out.extend_from_slice(&self.certificate_id.0);
        put_u64(&mut out, self.catalog_version);
        put_u64(&mut out, self.built_at_ms);
        put_u32(&mut out, self.intents.len() as u32);
        for i in &self.intents {
            out.extend_from_slice(&i.encode_v1());
        }
        put_u32(&mut out, self.dependency_edges.len() as u32);
        for &(a, b) in &self.dependency_edges {
            put_u32(&mut out, a);
            put_u32(&mut out, b);
        }
        put_u16(&mut out, self.max_parallel);
        put_u64(&mut out, self.deadline_ms);
        out.extend_from_slice(&self.compiler_digest);
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    #[cfg(feature = "transition-certifier")]
    use super::*;
    #[cfg(feature = "transition-certifier")]
    use crate::certificate::CertifierSeal;
    #[cfg(feature = "transition-certifier")]
    use crate::ids::{CertificateId, IntentId, SnapshotId};
    #[cfg(feature = "transition-certifier")]
    use crate::intent::{CompensationHint, Precondition};

    // 以下夹具仅被 transition-certifier 门控的测试使用（默认 feature
    // 下不存在，避免 dead-code 警告）。

    #[cfg(feature = "transition-certifier")]
    struct SinglePcs;

    #[cfg(feature = "transition-certifier")]
    impl ResourceRegistry for SinglePcs {
        fn asset_exists(&self, asset_id: u64) -> bool {
            asset_id == 1
        }
        fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
            if asset_id == 1 {
                Some((1, 10, 6))
            } else {
                None
            }
        }
        fn catalog_version(&self) -> u64 {
            1
        }
    }

    #[cfg(feature = "transition-certifier")]
    fn cert() -> CertifiedPlan {
        CertifiedPlan::new_sealed(
            CertifierSeal::issue_transition(),
            crate::certificate::CertifiedPlanFields {
                certificate_id: CertificateId([9u8; 16]),
                plan_id: PlanId([1u8; 16]),
                plan_sha256: [0xAB; 32],
                snapshot_id: SnapshotId([2u8; 16]),
                certifier_version: 1,
                constraint_catalog_version: 1, // catalog version
                tolerance_profile_id: 1,
                certified_at_ms: 1_000,
                valid_until_ms: 61_000,
                hard_constraint_count: 1,
                proof_digest: [0xCD; 32],
            },
        )
        .unwrap()
    }

    #[cfg(feature = "transition-certifier")]
    fn intent() -> CommandIntent {
        CommandIntent::builder(IntentId([3u8; 16]), PlanId([1u8; 16]), [0xAB; 32])
            .target(1, 1, 10, -50.0, 6)
            .window(1_000, 60_000)
            .policy(Precondition::None, CompensationHint::None)
            .build()
            .unwrap()
    }

    #[test]
    #[cfg(feature = "transition-certifier")]
    fn batch_requires_cert_and_registry() {
        let c = cert();
        let b = CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![intent()], 2_000).unwrap();
        assert_eq!(b.intents().len(), 1);
        assert_eq!(b.certificate_id(), c.certificate_id());
    }

    #[test]
    #[cfg(feature = "transition-certifier")]
    fn batch_scope_and_binding_negatives() {
        let c = cert();
        // 意图绑定别的 plan digest → 拒绝。
        let i = intent();
        let bad = CommandIntent::builder(*i.intent_id(), *i.plan_id(), [0xEE; 32])
            .target(1, 1, 10, -50.0, 6)
            .window(1_000, 60_000)
            .policy(Precondition::None, CompensationHint::None)
            .build()
            .unwrap();
        assert_eq!(
            CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![bad], 2_000),
            Err(ContractError::CertificatePlanMismatch)
        );
        // 意图指向注册表之外的绑定（点/单位不符）。
        let wrong_point = CommandIntent::builder(*i.intent_id(), *i.plan_id(), [0xAB; 32])
            .target(1, 1, 99, -50.0, 6)
            .window(1_000, 60_000)
            .policy(Precondition::None, CompensationHint::None)
            .build()
            .unwrap();
        assert_eq!(
            CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![wrong_point], 2_000),
            Err(ContractError::ScopeMismatch("binding"))
        );
        // 证书过期 → 拒绝。
        assert_eq!(
            CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![intent()], 61_001),
            Err(ContractError::InvalidWindow)
        );
        // 空批 → 拒绝。
        assert_eq!(
            CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![], 2_000),
            Err(ContractError::MissingField("intents"))
        );
    }

    // ---- [10] §2.2 批级编译元数据（.41 PlanCompiler 卷）----

    #[cfg(feature = "transition-certifier")]
    fn two_intents() -> (CommandIntent, CommandIntent) {
        let a = CommandIntent::builder(IntentId([3u8; 16]), PlanId([1u8; 16]), [0xAB; 32])
            .target(1, 1, 10, -50.0, 6)
            .window(1_000, 60_000)
            .policy(Precondition::None, CompensationHint::None)
            .build()
            .unwrap();
        let b = CommandIntent::builder(IntentId([4u8; 16]), PlanId([1u8; 16]), [0xAB; 32])
            .target(1, 1, 10, 30.0, 6)
            .window(2_000, 60_000)
            .policy(Precondition::None, CompensationHint::None)
            .build()
            .unwrap();
        (a, b)
    }

    #[test]
    #[cfg(feature = "transition-certifier")]
    fn compiled_batch_digest_deterministic_and_metadata_sensitive() {
        let c = cert();
        let (a, b) = two_intents();
        let edges = alloc::vec![(0u32, 1u32)];
        let cd: [u8; 32] = [7; 32];
        let b1 = CommandBatch::compile([7u8; 16], &c, &SinglePcs)
            .intents(vec![a, b])
            .built_at(2_000)
            .compile_meta(edges.clone(), 2, 60_000, cd)
            .finish()
            .unwrap();
        let b2 = CommandBatch::compile([7u8; 16], &c, &SinglePcs)
            .intents(vec![a, b])
            .built_at(2_000)
            .compile_meta(edges, 2, 60_000, cd)
            .finish()
            .unwrap();
        // 同输入同 digest（确定性不变量）。
        assert_eq!(b1.batch_digest(), b2.batch_digest());
        assert_eq!(b1.encode_v1(), b2.encode_v1());
        // 任一元数据漂移改变 digest（mutation 可检）。
        let b3 = CommandBatch::compile([7u8; 16], &c, &SinglePcs)
            .intents(vec![a, b])
            .built_at(2_000)
            .compile_meta(alloc::vec::Vec::new(), 2, 60_000, cd)
            .finish()
            .unwrap();
        assert_ne!(b1.batch_digest(), b3.batch_digest(), "边集入码");
        let b4 = CommandBatch::compile([7u8; 16], &c, &SinglePcs)
            .intents(vec![a, b])
            .built_at(2_000)
            .compile_meta(alloc::vec![(0u32, 1u32)], 4, 60_000, cd)
            .finish()
            .unwrap();
        assert_ne!(b1.batch_digest(), b4.batch_digest(), "max_parallel 入码");
    }

    #[test]
    #[cfg(feature = "transition-certifier")]
    fn compiled_batch_edge_and_cycle_negatives() {
        let c = cert();
        let (a, b) = two_intents();
        // 边索引越界。
        assert_eq!(
            CommandBatch::compile([7u8; 16], &c, &SinglePcs)
                .intents(vec![a, b])
                .built_at(2_000)
                .compile_meta(alloc::vec![(0u32, 2u32)], 1, 0, [0; 32])
                .finish(),
            Err(ContractError::ScopeMismatch("edge.index"))
        );
        // 自环。
        assert_eq!(
            CommandBatch::compile([7u8; 16], &c, &SinglePcs)
                .intents(vec![a, b])
                .built_at(2_000)
                .compile_meta(alloc::vec![(1u32, 1u32)], 1, 0, [0; 32])
                .finish(),
            Err(ContractError::Malformed("edge.self-loop"))
        );
        // DAG 有环（0→1→0）。
        assert_eq!(
            CommandBatch::compile([7u8; 16], &c, &SinglePcs)
                .intents(vec![a, b])
                .built_at(2_000)
                .compile_meta(alloc::vec![(0u32, 1u32), (1u32, 0u32)], 1, 0, [0; 32])
                .finish(),
            Err(ContractError::Malformed("dependency-cycle"))
        );
        // max_parallel=0。
        assert_eq!(
            CommandBatch::compile([7u8; 16], &c, &SinglePcs)
                .intents(vec![a])
                .built_at(2_000)
                .compile_meta(alloc::vec::Vec::new(), 0, 0, [0; 32])
                .finish(),
            Err(ContractError::OutOfRange("max_parallel"))
        );
    }
}
