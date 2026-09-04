//! `CandidatePlan` — 候选计划（[02] §7.1 最低字段集）。
//!
//! 唯一生产者 = RuleEngine 或 SolverPlanner（迁移期 = 旧 proposal 经严格
//! adapter）。不能自称安全或授权；`Infeasible/Unbounded/Timeout/Error` 与
//! 变量缺失**不可构造**（[02] §7.1：只有 Optimal 或界内 Feasible/Suboptimal
//! 可进入认证）。

use alloc::vec::Vec;

use crate::error::ContractError;
use crate::ids::{PlanId, SnapshotId};
use crate::wire::{get_u16, put_u16, put_u32, put_u64, MAGIC_CAND};

/// 求解状态（wire 数字码；Phase 0 语义沿用——非成功状态拒绝成候选）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveStatusKind {
    /// 最优。
    Optimal,
    /// 次优（界内）。
    Suboptimal,
}

impl SolveStatusKind {
    pub const fn code(self) -> u8 {
        match self {
            Self::Optimal => 0,
            Self::Suboptimal => 1,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Optimal),
            1 => Some(Self::Suboptimal),
            _ => None,
        }
    }
}

/// 单资源分配（单 PCS 迁移期 inline 形态；海量集合 .29 起 BlobRef）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assignment {
    /// 全局资产 ID（[`UNQUALIFIED_ASSET_ID`] = 未绑定——不满足控制资格）。
    pub asset_id: u64,
    /// 站内设备 ID。
    pub device_id: u16,
    /// 站内点 ID。
    pub point_id: u32,
    /// 有功功率 kW（正=放电；迁移期唯一命令语义）。
    pub power_kw: f64,
    /// 单位码（UnitCatalog；0=未知不满足控制资格）。
    pub unit_code: u16,
    /// 时段序号（多时段计划 ≥1；单时段=1）。
    pub step_index: u32,
}

impl Assignment {
    fn wire_len() -> usize {
        8 + 2 + 4 + 8 + 2 + 4
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.asset_id.to_le_bytes());
        out.extend_from_slice(&self.device_id.to_le_bytes());
        out.extend_from_slice(&self.point_id.to_le_bytes());
        out.extend_from_slice(&self.power_kw.to_le_bytes());
        out.extend_from_slice(&self.unit_code.to_le_bytes());
        out.extend_from_slice(&self.step_index.to_le_bytes());
    }

    fn decode(buf: &[u8], pos: &mut usize) -> Result<Self, ContractError> {
        let n = Self::wire_len();
        if *pos + n > buf.len() {
            return Err(ContractError::Malformed("assignment"));
        }
        let rec = &buf[*pos..*pos + n];
        let a = Self {
            asset_id: u64::from_le_bytes(rec[0..8].try_into().unwrap()),
            device_id: u16::from_le_bytes(rec[8..10].try_into().unwrap()),
            point_id: u32::from_le_bytes(rec[10..14].try_into().unwrap()),
            power_kw: f64::from_le_bytes(rec[14..22].try_into().unwrap()),
            unit_code: u16::from_le_bytes(rec[22..24].try_into().unwrap()),
            step_index: u32::from_le_bytes(rec[24..28].try_into().unwrap()),
        };
        *pos += n;
        Ok(a)
    }
}

/// inline 分配上限（超过必须走 BlobRef——[06] §4.2 大集合不入 IPC）。
pub const MAX_INLINE_ASSIGNMENTS: usize = 64;

/// 资格魔数单源（WP-64.3.1.4）：0 = 未绑定——不满足控制资格。
///
/// 此前同一不变量两种编码（Assignment 文档魔数 + `CommandIntent::new`
/// 拒收判据 + dispatch adapter unbound 判定各自字面 `0`）——收口于此。
pub const UNQUALIFIED_ASSET_ID: u64 = 0;

/// CandidatePlan 四语义组构造器（WP-64.3.1.1）。
///
/// 组界：identity（[`CandidatePlan::builder`] 声明）/ solve / horizons /
/// digests / validity / assignments——调用方只声明所变组；全部字段必填
/// （默认值是测试夹具的事，不是构造器的事）。`build()` 执行全部契约校验
/// 含轨迹不变量。
pub struct CandidatePlanBuilder {
    id: PlanId,
    snapshot_id: SnapshotId,
    objective_digest: [u8; 32],
    resource_catalog_version: u64,
    solve: Option<(SolveStatusKind, f64, f64, f64, u64)>,
    horizons: Option<(u64, u64, u32)>,
    digests: Option<([u8; 32], [u8; 32])>,
    validity: Option<(u64, u64)>,
    assignments: Option<Vec<Assignment>>,
}

impl CandidatePlanBuilder {
    /// 求解报告组（status/objective_value/gap/residual/elapsed）。
    pub fn solve(
        mut self,
        status: SolveStatusKind,
        objective_value: f64,
        relative_gap: f64,
        max_primal_residual: f64,
        elapsed_ms: u64,
    ) -> Self {
        self.solve = Some((
            status,
            objective_value,
            relative_gap,
            max_primal_residual,
            elapsed_ms,
        ));
        self
    }

    /// 地平线组（start_ms/step_ns/step_count）。
    pub fn horizons(mut self, start_ms: u64, step_ns: u64, step_count: u32) -> Self {
        self.horizons = Some((start_ms, step_ns, step_count));
        self
    }

    /// 摘要组（model/input——model_digest 为求解器标识摘要，input_digest
    /// 为输入 provenance）。
    pub fn digests(mut self, model: [u8; 32], input: [u8; 32]) -> Self {
        self.digests = Some((model, input));
        self
    }

    /// 有效期组（issued_at_ms/ttl_ms）。
    pub fn validity(mut self, issued_at_ms: u64, ttl_ms: u64) -> Self {
        self.validity = Some((issued_at_ms, ttl_ms));
        self
    }

    /// 分配组（非空、≤[`MAX_INLINE_ASSIGNMENTS`]、满足轨迹不变量）。
    pub fn assignments(mut self, v: Vec<Assignment>) -> Self {
        self.assignments = Some(v);
        self
    }

    /// 组装并校验（缺组 = `MissingField`；判据全清单见
    /// [`CandidatePlanParts`] 文档）。
    pub fn build(self) -> Result<CandidatePlan, ContractError> {
        let (solver_status, objective_value, relative_gap, max_primal_residual, elapsed_ms) =
            self.solve.ok_or(ContractError::MissingField("solve"))?;
        let (horizon_start_ms, step_ns, step_count) = self
            .horizons
            .ok_or(ContractError::MissingField("horizons"))?;
        let (model_digest, input_digest) =
            self.digests.ok_or(ContractError::MissingField("digests"))?;
        let (issued_at_ms, ttl_ms) = self
            .validity
            .ok_or(ContractError::MissingField("validity"))?;
        let assignments = self
            .assignments
            .ok_or(ContractError::MissingField("assignments"))?;
        CandidatePlan::from_parts(CandidatePlanParts {
            id: self.id,
            snapshot_id: self.snapshot_id,
            objective_digest: self.objective_digest,
            resource_catalog_version: self.resource_catalog_version,
            horizon_start_ms,
            step_ns,
            step_count,
            solver_status,
            objective_value,
            relative_gap,
            max_primal_residual,
            elapsed_ms,
            model_digest,
            input_digest,
            assignments,
            issued_at_ms,
            ttl_ms,
        })
    }
}

/// 候选计划（公开字段只读；构造经校验构造器）。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidatePlan {
    id: PlanId,
    /// 快照 ID（全零 = 未绑定快照——不满足控制资格）。
    snapshot_id: SnapshotId,
    objective_digest: [u8; 32],
    resource_catalog_version: u64,
    horizon_start_ms: u64,
    step_ns: u64,
    step_count: u32,
    solver_status: SolveStatusKind,
    objective_value: f64,
    relative_gap: f64,
    max_primal_residual: f64,
    elapsed_ms: u64,
    /// 模型/算法摘要（SolverModel/启发式标识）。
    model_digest: [u8; 32],
    /// 输入快照 provenance（TrustedTelemetry.provenance_digest——Phase 0
    /// 语义保留：dispatch 拒绝未绑定输入）。
    input_digest: [u8; 32],
    assignments: Vec<Assignment>,
    /// 有效期（毫秒基准，迁移期沿用单调 ms 时钟）。
    issued_at_ms: u64,
    ttl_ms: u64,
}

/// wire 解码字段袋（crate 内构造通道：`decode_v1` → 校验的唯一入口，
/// WP-64.3.1.1——17 位置参构造器收窄后的内部组合形态）。
pub(crate) struct CandidatePlanParts {
    pub id: PlanId,
    pub snapshot_id: SnapshotId,
    pub objective_digest: [u8; 32],
    pub resource_catalog_version: u64,
    pub horizon_start_ms: u64,
    pub step_ns: u64,
    pub step_count: u32,
    pub solver_status: SolveStatusKind,
    pub objective_value: f64,
    pub relative_gap: f64,
    pub max_primal_residual: f64,
    pub elapsed_ms: u64,
    pub model_digest: [u8; 32],
    pub input_digest: [u8; 32],
    pub assignments: Vec<Assignment>,
    pub issued_at_ms: u64,
    pub ttl_ms: u64,
}

impl CandidatePlan {
    /// 校验组装（唯一构造通道）。
    ///
    /// 拒绝：全零 ID/快照、非有限数值、负 gap/残差、空分配、分配超上限、
    /// 未绑定输入摘要（dispatch 契约沿用）、ttl=0、越界 step；以及轨迹
    /// 不变量（WP-64.3.1.2，构造点强制）：(asset, step) 唯一 + 每个
    /// 出现资产的 1..=step_count 全覆盖。
    pub(crate) fn from_parts(
        CandidatePlanParts {
            id,
            snapshot_id,
            objective_digest,
            resource_catalog_version,
            horizon_start_ms,
            step_ns,
            step_count,
            solver_status,
            objective_value,
            relative_gap,
            max_primal_residual,
            elapsed_ms,
            model_digest,
            input_digest,
            assignments,
            issued_at_ms,
            ttl_ms,
        }: CandidatePlanParts,
    ) -> Result<Self, ContractError> {
        if id.0 == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        if snapshot_id.is_unbound() {
            return Err(ContractError::MissingField("snapshot_id"));
        }
        if input_digest == [0u8; 32] {
            return Err(ContractError::MissingField("input_digest"));
        }
        for (name, v) in [
            ("objective_value", objective_value),
            ("relative_gap", relative_gap),
            ("max_primal_residual", max_primal_residual),
        ] {
            if !v.is_finite() {
                return Err(ContractError::OutOfRange(name));
            }
        }
        if relative_gap < 0.0 || max_primal_residual < 0.0 {
            return Err(ContractError::OutOfRange("negative_gap_or_residual"));
        }
        if assignments.is_empty() {
            return Err(ContractError::MissingField("assignments"));
        }
        if assignments.len() > MAX_INLINE_ASSIGNMENTS {
            return Err(ContractError::LimitExceeded("assignments"));
        }
        for a in &assignments {
            if !a.power_kw.is_finite() {
                return Err(ContractError::OutOfRange("assignment.power_kw"));
            }
            if a.step_index == 0 || a.step_index > step_count.max(1) {
                return Err(ContractError::OutOfRange("assignment.step_index"));
            }
        }
        if ttl_ms == 0 {
            return Err(ContractError::OutOfRange("ttl_ms"));
        }
        if step_count == 0 {
            return Err(ContractError::OutOfRange("step_count"));
        }
        // 轨迹不变量（构造点强制；certifier 独立复检按 ADR-051 保留）：
        // (a) (asset_id, step_index) 唯一；(b) 每个出现资产 1..=step_count
        // 全覆盖（step_index 已逐条界内 ⇒ 集合大小 = step_count 即恰好
        // 全覆盖）。
        let mut seen: alloc::collections::BTreeSet<(u64, u32)> = Default::default();
        let mut per_asset: alloc::collections::BTreeMap<u64, alloc::collections::BTreeSet<u32>> =
            Default::default();
        for a in &assignments {
            if !seen.insert((a.asset_id, a.step_index)) {
                return Err(ContractError::Malformed("duplicate_assignment"));
            }
            per_asset
                .entry(a.asset_id)
                .or_default()
                .insert(a.step_index);
        }
        for steps in per_asset.values() {
            if steps.len() != step_count as usize {
                return Err(ContractError::Malformed("assignment_trajectory_gap"));
            }
        }
        Ok(Self {
            id,
            snapshot_id,
            objective_digest,
            resource_catalog_version,
            horizon_start_ms,
            step_ns,
            step_count,
            solver_status,
            objective_value,
            relative_gap,
            max_primal_residual,
            elapsed_ms,
            model_digest,
            input_digest,
            assignments,
            issued_at_ms,
            ttl_ms,
        })
    }

    /// CandidatePlan 构造器（WP-64.3.1.1：四语义组 builder——identity/
    /// solve/horizons/digests/validity/assignments；全部字段必填，无便利
    /// 默认值。wire 解码走 crate 内 [`CandidatePlanParts`] 通道）。
    pub fn builder(
        id: PlanId,
        snapshot_id: SnapshotId,
        objective_digest: [u8; 32],
        resource_catalog_version: u64,
    ) -> CandidatePlanBuilder {
        CandidatePlanBuilder {
            id,
            snapshot_id,
            objective_digest,
            resource_catalog_version,
            solve: None,
            horizons: None,
            digests: None,
            validity: None,
            assignments: None,
        }
    }

    // —— 只读访问面 ——

    pub fn id(&self) -> &PlanId {
        &self.id
    }
    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }
    pub fn objective_digest(&self) -> &[u8; 32] {
        &self.objective_digest
    }
    pub fn input_digest(&self) -> &[u8; 32] {
        &self.input_digest
    }
    pub fn model_digest(&self) -> &[u8; 32] {
        &self.model_digest
    }
    pub fn resource_catalog_version(&self) -> u64 {
        self.resource_catalog_version
    }
    pub fn solver_status(&self) -> SolveStatusKind {
        self.solver_status
    }
    pub fn objective_value(&self) -> f64 {
        self.objective_value
    }
    pub fn relative_gap(&self) -> f64 {
        self.relative_gap
    }
    pub fn max_primal_residual(&self) -> f64 {
        self.max_primal_residual
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
    pub fn horizon_start_ms(&self) -> u64 {
        self.horizon_start_ms
    }
    pub fn step_ns(&self) -> u64 {
        self.step_ns
    }
    pub fn step_count(&self) -> u32 {
        self.step_count
    }
    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }
    pub fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }
    pub fn ttl_ms(&self) -> u64 {
        self.ttl_ms
    }

    /// 有效期判定。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.issued_at_ms.saturating_add(self.ttl_ms)
    }

    // —— wire V1 ——

    /// V1 wire 编码（小端；magic `CAND` + version + 定长域 + 分配区）。
    pub fn encode_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(160 + self.assignments.len() * Assignment::wire_len());
        out.extend_from_slice(&MAGIC_CAND);
        put_u16(&mut out, crate::CONTRACT_WIRE_V1);
        put_u16(&mut out, 0); // flags
        out.extend_from_slice(&self.id.0);
        out.extend_from_slice(&self.snapshot_id.0);
        out.extend_from_slice(&self.objective_digest);
        put_u64(&mut out, self.resource_catalog_version);
        put_u64(&mut out, self.horizon_start_ms);
        put_u64(&mut out, self.step_ns);
        put_u32(&mut out, self.step_count);
        out.push(self.solver_status.code());
        out.push(0); // reserved
        out.extend_from_slice(&self.objective_value.to_le_bytes());
        out.extend_from_slice(&self.relative_gap.to_le_bytes());
        out.extend_from_slice(&self.max_primal_residual.to_le_bytes());
        put_u64(&mut out, self.elapsed_ms);
        out.extend_from_slice(&self.model_digest);
        out.extend_from_slice(&self.input_digest);
        put_u64(&mut out, self.issued_at_ms);
        put_u64(&mut out, self.ttl_ms);
        put_u32(&mut out, self.assignments.len() as u32);
        for a in &self.assignments {
            a.encode(&mut out);
        }
        out
    }

    /// CandidatePlan wire SHA-256（62 v0.315.0 WP-62.1.2 单源派生点）。
    ///
    /// `CoreCommand.plan_sha` 的契约值（engine.rs CoreCommand 文档）与
    /// compiler 计划-证书绑定锚、certifier 证书摘要共用本公式：
    /// 计划 wire 字节的 SHA-256。此前三处各自手写、EngineSagaExecutor
    /// 又以 input_digest 冒充（D-3 漂移）——派生只允许经本函数族。
    pub fn plan_wire_sha256(plan: &CandidatePlan) -> [u8; 32] {
        Self::plan_wire_sha256_of_wire(plan.encode_v1().as_slice())
    }

    /// wire 字节形态的同一摘要（WP-64.3.2.2 派生辅助）：certifier verify
    /// 与测试夹具持 wire 字节（未解码形态）时经此派生——公式与
    /// [`CandidatePlan::plan_wire_sha256`] 单源，禁止调用方再手写
    /// 摘要公式。
    pub fn plan_wire_sha256_of_wire(wire: &[u8]) -> [u8; 32] {
        use sha2::Digest as _;
        sha2::Sha256::digest(wire).into()
    }

    /// V1 wire 解码（先校 magic/版本/计数上限，再读字段——[06] §4.2）。
    pub fn decode_v1(buf: &[u8]) -> Result<Self, ContractError> {
        if buf.len() < 8 || buf[0..4] != MAGIC_CAND {
            return Err(ContractError::Malformed("magic"));
        }
        let version = u16::from_le_bytes(get_u16(buf, 4)?);
        if version != crate::CONTRACT_WIRE_V1 {
            return Err(ContractError::VersionUnsupported {
                found: version,
                supported: crate::CONTRACT_WIRE_V1,
            });
        }
        let flags = u16::from_le_bytes(get_u16(buf, 6)?);
        if flags != 0 {
            return Err(ContractError::Malformed("flags"));
        }
        let mut pos = 8usize;
        let need = |pos: usize, add: usize, len: usize| -> Result<(), ContractError> {
            if pos + add > len {
                return Err(ContractError::Malformed("truncated"));
            }
            Ok(())
        };
        need(pos, 16, buf.len())?;
        let id = PlanId(buf[pos..pos + 16].try_into().unwrap());
        pos += 16;
        need(pos, 16, buf.len())?;
        let snapshot_id = SnapshotId(buf[pos..pos + 16].try_into().unwrap());
        pos += 16;
        need(pos, 32, buf.len())?;
        let mut objective_digest = [0u8; 32];
        objective_digest.copy_from_slice(&buf[pos..pos + 32]);
        pos += 32;
        let rd64 = |pos: &mut usize| -> Result<u64, ContractError> {
            need(*pos, 8, buf.len())?;
            let v = u64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(v)
        };
        let resource_catalog_version = rd64(&mut pos)?;
        let horizon_start_ms = rd64(&mut pos)?;
        let step_ns = rd64(&mut pos)?;
        need(pos, 4, buf.len())?;
        let step_count = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        need(pos, 2, buf.len())?;
        let status_code = buf[pos];
        let reserved = buf[pos + 1];
        if reserved != 0 {
            return Err(ContractError::Malformed("reserved"));
        }
        let solver_status =
            SolveStatusKind::from_code(status_code).ok_or(ContractError::Malformed("status"))?;
        pos += 2;
        need(pos, 24, buf.len())?;
        let objective_value = f64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        let relative_gap = f64::from_le_bytes(buf[pos + 8..pos + 16].try_into().unwrap());
        let max_primal_residual = f64::from_le_bytes(buf[pos + 16..pos + 24].try_into().unwrap());
        pos += 24;
        let elapsed_ms = rd64(&mut pos)?;
        need(pos, 32, buf.len())?;
        let mut model_digest = [0u8; 32];
        model_digest.copy_from_slice(&buf[pos..pos + 32]);
        pos += 32;
        need(pos, 32, buf.len())?;
        let mut input_digest = [0u8; 32];
        input_digest.copy_from_slice(&buf[pos..pos + 32]);
        pos += 32;
        let issued_at_ms = rd64(&mut pos)?;
        let ttl_ms = rd64(&mut pos)?;
        need(pos, 4, buf.len())?;
        let count = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if count > MAX_INLINE_ASSIGNMENTS {
            return Err(ContractError::LimitExceeded("assignments"));
        }
        let mut assignments = Vec::with_capacity(count);
        for _ in 0..count {
            assignments.push(Assignment::decode(buf, &mut pos)?);
        }
        if pos != buf.len() {
            return Err(ContractError::Malformed("trailing"));
        }
        Self::from_parts(CandidatePlanParts {
            id,
            snapshot_id,
            objective_digest,
            resource_catalog_version,
            horizon_start_ms,
            step_ns,
            step_count,
            solver_status,
            objective_value,
            relative_gap,
            max_primal_residual,
            elapsed_ms,
            model_digest,
            input_digest,
            assignments,
            issued_at_ms,
            ttl_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// 62 v0.315.0 WP-62.1.2：派生公式锁——`plan_wire_sha256` 恒等于
    /// `SHA-256(plan.encode_v1())` 本体（唯一允许的派生写法）。
    #[test]
    fn plan_wire_sha256_matches_encode_v1_digest() {
        use sha2::Digest as _;
        let p = plan();
        let via_helper = CandidatePlan::plan_wire_sha256(&p);
        let via_formula: [u8; 32] = sha2::Sha256::digest(p.encode_v1().as_slice()).into();
        assert_eq!(via_helper, via_formula);
        // WP-64.3.2.2：wire 形态辅助与 plan 形态单源。
        assert_eq!(
            CandidatePlan::plan_wire_sha256_of_wire(&p.encode_v1()),
            CandidatePlan::plan_wire_sha256(&p)
        );
    }

    /// WP-64.3.2.2 辅助函数 KAT（SHA-256 空向量 + "abc"）。
    #[test]
    fn plan_wire_sha256_of_wire_known_vectors() {
        assert_eq!(
            hex(CandidatePlan::plan_wire_sha256_of_wire(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(CandidatePlan::plan_wire_sha256_of_wire(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn hex(b: [u8; 32]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn plan() -> CandidatePlan {
        let mut sid = [0u8; 16];
        sid[..8].copy_from_slice(&0xA5A5u64.to_le_bytes());
        CandidatePlan::builder(
            PlanId::new([1u8; 16]).unwrap(),
            SnapshotId(sid),
            [2u8; 32],
            1,
        )
        .solve(SolveStatusKind::Suboptimal, 12.5, 0.01, 0.001, 30)
        .horizons(1_000, 1_000, 1)
        .digests([3u8; 32], [4u8; 32])
        .validity(2_000, 60_000)
        .assignments(vec![Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: -50.0,
            unit_code: 6,
            step_index: 1,
        }])
        .build()
        .unwrap()
    }

    /// 负例基座（WP-64.3.1.6：负例只重声明所变组）。
    fn neg(id: PlanId, sid: SnapshotId) -> CandidatePlanBuilder {
        CandidatePlan::builder(id, sid, [0u8; 32], 1)
            .solve(SolveStatusKind::Optimal, 0.0, 0.0, 0.0, 0)
            .horizons(0, 0, 1)
            .digests([0u8; 32], [1u8; 32])
            .validity(0, 1)
            .assignments(vec![Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 1,
                power_kw: 1.0,
                unit_code: 6,
                step_index: 1,
            }])
    }

    #[test]
    fn roundtrip_v1() {
        let p = plan();
        let wire = p.encode_v1();
        let d = CandidatePlan::decode_v1(&wire).unwrap();
        assert_eq!(d, p);
    }

    #[test]
    fn negative_rejections() {
        // 全零 ID。
        let mut sid = [1u8; 16];
        sid[0] = 0xA5;
        assert_eq!(
            neg(PlanId([0u8; 16]), SnapshotId(sid)).build(),
            Err(ContractError::ZeroId)
        );
        // NaN 目标值（只重声明 solve 组）。
        let p = plan();
        let c = neg(*p.id(), p.snapshot_id())
            .solve(SolveStatusKind::Optimal, f64::NAN, 0.0, 0.0, 0)
            .build();
        assert_eq!(c, Err(ContractError::OutOfRange("objective_value")));
        // 空分配（只重声明 assignments 组）。
        let c = neg(*p.id(), p.snapshot_id()).assignments(vec![]).build();
        assert_eq!(c, Err(ContractError::MissingField("assignments")));
        // 未绑定输入摘要（dispatch 契约：拒绝模糊输入；只重声明 digests 组）。
        let c = neg(*p.id(), p.snapshot_id())
            .digests([0u8; 32], [0u8; 32])
            .build();
        assert_eq!(c, Err(ContractError::MissingField("input_digest")));
        // ttl=0（只重声明 validity 组）。
        let c = neg(*p.id(), p.snapshot_id()).validity(0, 0).build();
        assert_eq!(c, Err(ContractError::OutOfRange("ttl_ms")));
        // ttl=0 ∧ step_count=0 双非法：错误词序与迁移前实现一致
        // （ttl_ms 先报；step_count 检查必须在轨迹不变量之前保持可达
        // ——64.3 复查回归锁）。
        let c = neg(*p.id(), p.snapshot_id())
            .horizons(1_000, 1_000, 0)
            .validity(2_000, 0)
            .build();
        assert_eq!(c, Err(ContractError::OutOfRange("ttl_ms")));
    }

    /// WP-64.3.1.2 轨迹不变量：重复 (asset,step) 对构造点拒绝（先红后
    /// 绿——现状 `new` 放行，红测 r64_duplicate_assignment_currently_
    /// accepted 于实现前实证）。
    #[test]
    fn r64_duplicate_assignment_rejected_at_construction() {
        let a = |step: u32, power: f64| Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: power,
            unit_code: 6,
            step_index: step,
        };
        let err = CandidatePlan::builder(
            PlanId::new([1u8; 16]).unwrap(),
            SnapshotId([7u8; 16]),
            [2u8; 32],
            1,
        )
        .solve(SolveStatusKind::Optimal, 1.0, 0.0, 0.0, 1)
        .horizons(1_000, 1_000, 1)
        .digests([3u8; 32], [4u8; 32])
        .validity(2_000, 60_000)
        .assignments(vec![a(1, 1.0), a(1, 2.0)])
        .build();
        assert_eq!(err, Err(ContractError::Malformed("duplicate_assignment")));
    }

    /// WP-64.3.1.2 轨迹不变量：出现资产 1..=step_count 全覆盖（缺档拒绝
    /// /全覆盖放行）。
    #[test]
    fn r64_trajectory_gap_rejected_and_full_coverage_accepted() {
        let a = |step: u32, power: f64| Assignment {
            asset_id: 1,
            device_id: 1,
            point_id: 10,
            power_kw: power,
            unit_code: 6,
            step_index: step,
        };
        let base = || {
            CandidatePlan::builder(
                PlanId::new([1u8; 16]).unwrap(),
                SnapshotId([7u8; 16]),
                [2u8; 32],
                1,
            )
            .solve(SolveStatusKind::Optimal, 1.0, 0.0, 0.0, 1)
            .horizons(1_000, 1_000, 2)
            .digests([3u8; 32], [4u8; 32])
            .validity(2_000, 60_000)
        };
        // 缺档：asset 1 只覆盖 step 1 / 2 。
        assert_eq!(
            base().assignments(vec![a(1, 1.0)]).build(),
            Err(ContractError::Malformed("assignment_trajectory_gap"))
        );
        // 全覆盖：1..=2 两步齐备。
        assert!(base()
            .assignments(vec![a(1, 1.0), a(2, -2.0)])
            .build()
            .is_ok());
    }

    #[test]
    fn wire_mutations_rejected() {
        let mut wire = plan().encode_v1();
        // 未知求解状态码。
        let status_off = 8 + 16 + 16 + 32 + 8 * 3 + 4;
        assert_eq!(wire[status_off], 1);
        wire[status_off] = 0xFE;
        assert!(matches!(
            CandidatePlan::decode_v1(&wire),
            Err(ContractError::Malformed("status"))
        ));
        // reserved 非零。
        let mut wire = plan().encode_v1();
        wire[status_off + 1] = 9;
        assert!(matches!(
            CandidatePlan::decode_v1(&wire),
            Err(ContractError::Malformed("reserved"))
        ));
        // 截断。
        let wire = plan().encode_v1();
        assert!(matches!(
            CandidatePlan::decode_v1(&wire[..wire.len() - 4]),
            Err(ContractError::Malformed(_))
        ));
        // 尾随字节。
        let mut wire = plan().encode_v1();
        wire.push(0);
        assert!(matches!(
            CandidatePlan::decode_v1(&wire),
            Err(ContractError::Malformed("trailing"))
        ));
    }
}
