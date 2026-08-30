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
    /// 全局资产 ID（0 = 未绑定——不满足控制资格）。
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

impl CandidatePlan {
    /// 校验构造器（唯一入口）。
    ///
    /// 拒绝：全零 ID/快照、非有限数值、负 gap/残差、空分配、分配超上限、
    /// 未绑定输入摘要（dispatch 契约沿用）、ttl=0、越界 step。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: PlanId,
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
        model_digest: [u8; 32],
        input_digest: [u8; 32],
        assignments: Vec<Assignment>,
        issued_at_ms: u64,
        ttl_ms: u64,
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
        Self::new(
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
        )
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn plan() -> CandidatePlan {
        let mut sid = [0u8; 16];
        sid[..8].copy_from_slice(&0xA5A5u64.to_le_bytes());
        CandidatePlan::new(
            PlanId::new([1u8; 16]).unwrap(),
            SnapshotId(sid),
            [2u8; 32],
            1,
            1_000,
            1_000,
            1,
            SolveStatusKind::Suboptimal,
            12.5,
            0.01,
            0.001,
            30,
            [3u8; 32],
            [4u8; 32],
            vec![Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 10,
                power_kw: -50.0,
                unit_code: 6,
                step_index: 1,
            }],
            2_000,
            60_000,
        )
        .unwrap()
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
            CandidatePlan::new(
                PlanId([0u8; 16]),
                SnapshotId(sid),
                [0; 32],
                1,
                0,
                0,
                1,
                SolveStatusKind::Optimal,
                0.0,
                0.0,
                0.0,
                0,
                [0; 32],
                [1; 32],
                vec![Assignment {
                    asset_id: 1,
                    device_id: 1,
                    point_id: 1,
                    power_kw: 1.0,
                    unit_code: 6,
                    step_index: 1
                }],
                0,
                1,
            ),
            Err(ContractError::ZeroId)
        );
        // NaN 目标值。
        let p = plan();
        let c = CandidatePlan::new(
            *p.id(),
            p.snapshot_id(),
            *p.objective_digest(),
            1,
            0,
            0,
            1,
            SolveStatusKind::Optimal,
            f64::NAN,
            0.0,
            0.0,
            0,
            *p.model_digest(),
            *p.input_digest(),
            vec![Assignment {
                asset_id: 1,
                device_id: 1,
                point_id: 1,
                power_kw: 1.0,
                unit_code: 6,
                step_index: 1,
            }],
            0,
            1,
        );
        assert_eq!(c, Err(ContractError::OutOfRange("objective_value")));
        // 空分配。
        let c = CandidatePlan::new(
            *p.id(),
            p.snapshot_id(),
            *p.objective_digest(),
            1,
            0,
            0,
            1,
            SolveStatusKind::Optimal,
            0.0,
            0.0,
            0.0,
            0,
            *p.model_digest(),
            *p.input_digest(),
            vec![],
            0,
            1,
        );
        assert_eq!(c, Err(ContractError::MissingField("assignments")));
        // 未绑定输入摘要（dispatch 契约：拒绝模糊输入）。
        let c = CandidatePlan::new(
            *p.id(),
            p.snapshot_id(),
            *p.objective_digest(),
            1,
            0,
            0,
            1,
            SolveStatusKind::Optimal,
            0.0,
            0.0,
            0.0,
            0,
            *p.model_digest(),
            [0u8; 32],
            p.assignments().to_vec(),
            0,
            1,
        );
        assert_eq!(c, Err(ContractError::MissingField("input_digest")));
        // ttl=0。
        let c = CandidatePlan::new(
            *p.id(),
            p.snapshot_id(),
            *p.objective_digest(),
            1,
            0,
            0,
            1,
            SolveStatusKind::Optimal,
            0.0,
            0.0,
            0.0,
            0,
            *p.model_digest(),
            *p.input_digest(),
            p.assignments().to_vec(),
            0,
            0,
        );
        assert_eq!(c, Err(ContractError::OutOfRange("ttl_ms")));
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
