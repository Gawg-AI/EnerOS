//! `CertifiedPlan` — 独立认证证书（[02] §7.2 最低字段集）。
//!
//! # 构造权收口（[06] §3.3）
//!
//! 字段全私有；构造器需要 [`CertifierSeal`]——该类型**无公开字段构造**：
//! - 未启用 `certifier` feature 的 crate：枚举面无法构造（`E0451`，
//!   tests/compilefail 锁定）。外部只有 [`CertifiedPlan::decode_v1`]/
//!   [`CertifiedPlan::verify_binding`]。
//! - 启用 `certifier` feature：`CertifierSeal::issue()` 可用 = **唯一发放
//!   面** `eneros-certifier`（`CertifierModule::certify`）。蓝图 70.9 起
//!   dispatch 的过渡发放面（`transition-certifier`）已按 ADR-051 出口条件
//!   **删除**：没有"证书缺失就启用旧 seal"的分支。
//!
//! **诚实边界（复查订正）**：`certifier` 是 Cargo feature，而 feature 在
//! 同一构建图内**合并**——凡依赖 `eneros-certifier`（验证面：ring +
//! verify）的图都会打开它，于是 `issue()` 在该图内语法可达。故"生成器
//! 签不出自己的证书"由**源码扫描门**执法（唯一发放面 + 定义面之外的
//! 任何 crate 不得出现发放构造调用，见 certifier crate
//! `tests/integration.rs::seal_issuance_source_scan_rejects_third_party_callers`），
//! 不是类型系统保证。

use alloc::vec::Vec;

use crate::error::ContractError;
use crate::ids::{CertificateId, PlanId};
use crate::wire::{get_u16, get_u64, put_u16, put_u32, put_u64, MAGIC_CERT};

/// 认证权印（零大小；构造收口 token）。
///
/// 本 crate 外**无法**构造本类型（私有字段 + 无 pub 构造器）——这是
/// CertifiedPlan 构造收口的机制本身，不是占位。
pub struct CertifierSeal {
    _priv: (),
}

impl core::fmt::Debug for CertifierSeal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CertifierSeal")
    }
}

impl Clone for CertifierSeal {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for CertifierSeal {}

#[cfg(feature = "certifier")]
impl CertifierSeal {
    /// 真认证器发放（唯一发放点 = eneros-certifier 的
    /// `CertifierModule::certify`；发放面唯一性由源码扫描门执法——
    /// feature 在同图内合并，见模块头「诚实边界」）。
    ///
    /// 语义约定：调用方必须已用**独立评估器**重算全部硬约束并留证
    /// （proof records）；本函数不接收证据参数——它只是"谁可以造证书"
    /// 的类型闸，证据由 CertifierModule 的评估链结构与 mutation 测试
    /// 锁定（[09] §4.2/§4.7）。
    #[doc(hidden)]
    #[must_use]
    pub fn issue() -> Self {
        Self { _priv: () }
    }
}

/// 独立认证证书（字段全私有；wire V1 往返 + 绑定校验）。
#[derive(Debug, Clone, PartialEq)]
pub struct CertifiedPlan {
    certificate_id: CertificateId,
    plan_id: PlanId,
    /// CandidatePlan wire 的 SHA-256（绑定计划本体——改一位即失配）。
    plan_sha256: [u8; 32],
    snapshot_id: crate::ids::SnapshotId,
    /// 认证器版本（迁移期 = control-chain-transition 语义版本）。
    certifier_version: u32,
    constraint_catalog_version: u64,
    tolerance_profile_id: u32,
    certified_at_ms: u64,
    valid_until_ms: u64,
    hard_constraint_count: u32,
    proof_digest: [u8; 32],
}

/// 认证侧字段袋（WP-64.3.1：seal-gated 构造器共用同一字段形状——
/// 12 位置参签名去重）。
pub struct CertifiedPlanFields {
    pub certificate_id: CertificateId,
    pub plan_id: PlanId,
    pub plan_sha256: [u8; 32],
    pub snapshot_id: crate::ids::SnapshotId,
    pub certifier_version: u32,
    pub constraint_catalog_version: u64,
    pub tolerance_profile_id: u32,
    pub certified_at_ms: u64,
    pub valid_until_ms: u64,
    pub hard_constraint_count: u32,
    pub proof_digest: [u8; 32],
}

impl CertifiedPlan {
    /// 真认证器构造（.38 起；`certifier` feature = 仅 eneros-certifier
    /// 可达）。seal 本身不可跨 crate 构造——构造权仍收口于发放面。
    #[cfg(feature = "certifier")]
    pub fn new_certified(
        seal: CertifierSeal,
        fields: CertifiedPlanFields,
    ) -> Result<Self, ContractError> {
        Self::new_sealed(seal, fields)
    }

    /// 认证侧构造（seal-gated；crate 内唯一入口）。
    pub(crate) fn new_sealed(
        _seal: CertifierSeal,
        CertifiedPlanFields {
            certificate_id,
            plan_id,
            plan_sha256,
            snapshot_id,
            certifier_version,
            constraint_catalog_version,
            tolerance_profile_id,
            certified_at_ms,
            valid_until_ms,
            hard_constraint_count,
            proof_digest,
        }: CertifiedPlanFields,
    ) -> Result<Self, ContractError> {
        // 零 ID = 缺字段（与 CandidatePlan 同纪律；decode 路径同门复验）。
        if plan_id.0 == [0u8; 16] || certificate_id.0 == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        if plan_sha256 == [0u8; 32] || proof_digest == [0u8; 32] {
            return Err(ContractError::MissingField("digest"));
        }
        if valid_until_ms <= certified_at_ms {
            return Err(ContractError::InvalidWindow);
        }
        if snapshot_id.is_unbound() {
            return Err(ContractError::MissingField("snapshot_id"));
        }
        Ok(Self {
            certificate_id,
            plan_id,
            plan_sha256,
            snapshot_id,
            certifier_version,
            constraint_catalog_version,
            tolerance_profile_id,
            certified_at_ms,
            valid_until_ms,
            hard_constraint_count,
            proof_digest,
        })
    }

    // —— 只读访问面 ——

    pub fn certificate_id(&self) -> &CertificateId {
        &self.certificate_id
    }
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn plan_sha256(&self) -> &[u8; 32] {
        &self.plan_sha256
    }
    pub fn snapshot_id(&self) -> crate::ids::SnapshotId {
        self.snapshot_id
    }
    pub fn certifier_version(&self) -> u32 {
        self.certifier_version
    }
    pub fn constraint_catalog_version(&self) -> u64 {
        self.constraint_catalog_version
    }
    pub fn tolerance_profile_id(&self) -> u32 {
        self.tolerance_profile_id
    }
    pub fn certified_at_ms(&self) -> u64 {
        self.certified_at_ms
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }
    pub fn hard_constraint_count(&self) -> u32 {
        self.hard_constraint_count
    }
    pub fn proof_digest(&self) -> &[u8; 32] {
        &self.proof_digest
    }

    /// 证书有效期判定。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms <= self.valid_until_ms
    }

    /// 绑定校验：证书是否覆盖给定 (plan_id, plan_wire_sha256)——
    /// CommandBatch/编译器的强制前置。
    pub fn verify_binding(&self, plan_id: &PlanId, plan_wire_sha256: &[u8; 32]) -> bool {
        &self.plan_id == plan_id && self.plan_sha256 == *plan_wire_sha256
    }

    // —— wire V1 ——

    /// V1 wire 编码。
    pub fn encode_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&MAGIC_CERT);
        put_u16(&mut out, crate::CONTRACT_WIRE_V1);
        put_u16(&mut out, 0);
        out.extend_from_slice(&self.certificate_id.0);
        out.extend_from_slice(&self.plan_id.0);
        out.extend_from_slice(&self.plan_sha256);
        out.extend_from_slice(&self.snapshot_id.0);
        put_u32(&mut out, self.certifier_version);
        put_u64(&mut out, self.constraint_catalog_version);
        put_u32(&mut out, self.tolerance_profile_id);
        put_u64(&mut out, self.certified_at_ms);
        put_u64(&mut out, self.valid_until_ms);
        put_u32(&mut out, self.hard_constraint_count);
        out.push(0); // reserved
        out.extend_from_slice(&self.proof_digest);
        out
    }

    /// V1 wire 解码（外部唯一获得证书的途径之一：decode + verify）。
    pub fn decode_v1(buf: &[u8]) -> Result<Self, ContractError> {
        if buf.len() < 8 || buf[0..4] != MAGIC_CERT {
            return Err(ContractError::Malformed("magic"));
        }
        let version = u16::from_le_bytes(get_u16(buf, 4)?);
        if version != crate::CONTRACT_WIRE_V1 {
            return Err(ContractError::VersionUnsupported {
                found: version,
                supported: crate::CONTRACT_WIRE_V1,
            });
        }
        if u16::from_le_bytes(get_u16(buf, 6)?) != 0 {
            return Err(ContractError::Malformed("flags"));
        }
        let mut pos = 8usize;
        let need = |pos: usize, add: usize, len: usize| -> Result<(), ContractError> {
            if pos + add > len {
                return Err(ContractError::Malformed("truncated"));
            }
            Ok(())
        };
        need(pos, 32, buf.len())?;
        let certificate_id = CertificateId(buf[pos..pos + 16].try_into().unwrap());
        let plan_id = PlanId(buf[pos + 16..pos + 32].try_into().unwrap());
        pos += 32;
        need(pos, 32, buf.len())?;
        let mut plan_sha256 = [0u8; 32];
        plan_sha256.copy_from_slice(&buf[pos..pos + 32]);
        pos += 32;
        need(pos, 16, buf.len())?;
        let snapshot_id = crate::ids::SnapshotId(buf[pos..pos + 16].try_into().unwrap());
        pos += 16;
        need(pos, 4, buf.len())?;
        let certifier_version = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let constraint_catalog_version = u64::from_le_bytes(get_u64(buf, pos)?);
        pos += 8;
        need(pos, 4, buf.len())?;
        let tolerance_profile_id = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let certified_at_ms = u64::from_le_bytes(get_u64(buf, pos)?);
        pos += 8;
        let valid_until_ms = u64::from_le_bytes(get_u64(buf, pos)?);
        pos += 8;
        need(pos, 4, buf.len())?;
        let hard_constraint_count = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        need(pos, 1, buf.len())?;
        if buf[pos] != 0 {
            return Err(ContractError::Malformed("reserved"));
        }
        pos += 1;
        need(pos, 32, buf.len())?;
        let mut proof_digest = [0u8; 32];
        proof_digest.copy_from_slice(&buf[pos..pos + 32]);
        pos += 32;
        if pos != buf.len() {
            return Err(ContractError::Malformed("trailing"));
        }
        // 经同一校验路径（外部 decode 的字段同样不可为空摘要/倒窗）。
        Self::new_sealed(
            // decode 路径无需 seal——wire 是认证侧签发后的字面回放；
            // 但字段合法性必须复验（收口在"构造"，不在"传输"）。
            CertifierSeal { _priv: () },
            CertifiedPlanFields {
                certificate_id,
                plan_id,
                plan_sha256,
                snapshot_id,
                certifier_version,
                constraint_catalog_version,
                tolerance_profile_id,
                certified_at_ms,
                valid_until_ms,
                hard_constraint_count,
                proof_digest,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[cfg(feature = "certifier")]
    fn sealed_cert() -> CertifiedPlan {
        CertifiedPlan::new_sealed(
            CertifierSeal::issue(),
            CertifiedPlanFields {
                certificate_id: CertificateId([9u8; 16]),
                plan_id: PlanId([1u8; 16]),
                plan_sha256: [0xAB; 32],
                snapshot_id: crate::ids::SnapshotId([2u8; 16]),
                certifier_version: 1,
                constraint_catalog_version: 1,
                tolerance_profile_id: 1,
                certified_at_ms: 1_000,
                valid_until_ms: 61_000,
                hard_constraint_count: 12,
                proof_digest: [0xCD; 32],
            },
        )
        .unwrap()
    }

    #[test]
    #[cfg(feature = "certifier")]
    fn roundtrip_and_binding() {
        let c = sealed_cert();
        let wire = c.encode_v1();
        let d = CertifiedPlan::decode_v1(&wire).unwrap();
        assert_eq!(d, c);
        // 绑定校验。
        assert!(d.verify_binding(d.plan_id(), d.plan_sha256()));
        let wrong: [u8; 32] = [0; 32];
        assert!(!d.verify_binding(d.plan_id(), &wrong));
        assert!(d.is_valid_at(60_000));
        assert!(!d.is_valid_at(61_001));
    }

    #[test]
    #[cfg(feature = "certifier")]
    fn empty_digest_rejected() {
        let r = CertifiedPlan::new_sealed(
            CertifierSeal::issue(),
            CertifiedPlanFields {
                certificate_id: CertificateId([9u8; 16]),
                plan_id: PlanId([1u8; 16]),
                plan_sha256: [0u8; 32],
                snapshot_id: crate::ids::SnapshotId([2u8; 16]),
                certifier_version: 1,
                constraint_catalog_version: 1,
                tolerance_profile_id: 1,
                certified_at_ms: 1_000,
                valid_until_ms: 2_000,
                hard_constraint_count: 1,
                proof_digest: [0xCD; 32],
            },
        );
        assert_eq!(r, Err(ContractError::MissingField("digest")));
        // 倒窗。
        let r = CertifiedPlan::new_sealed(
            CertifierSeal::issue(),
            CertifiedPlanFields {
                certificate_id: CertificateId([9u8; 16]),
                plan_id: PlanId([1u8; 16]),
                plan_sha256: [0xAB; 32],
                snapshot_id: crate::ids::SnapshotId([2u8; 16]),
                certifier_version: 1,
                constraint_catalog_version: 1,
                tolerance_profile_id: 1,
                certified_at_ms: 2_000,
                valid_until_ms: 2_000,
                hard_constraint_count: 1,
                proof_digest: [0xCD; 32],
            },
        );
        assert_eq!(r, Err(ContractError::InvalidWindow));
        // 零 plan/certificate ID = 缺字段（与 CandidatePlan 同纪律）。
        let r = CertifiedPlan::new_sealed(
            CertifierSeal::issue(),
            CertifiedPlanFields {
                certificate_id: CertificateId([9u8; 16]),
                plan_id: PlanId([0u8; 16]),
                plan_sha256: [0xAB; 32],
                snapshot_id: crate::ids::SnapshotId([2u8; 16]),
                certifier_version: 1,
                constraint_catalog_version: 1,
                tolerance_profile_id: 1,
                certified_at_ms: 1_000,
                valid_until_ms: 2_000,
                hard_constraint_count: 1,
                proof_digest: [0xCD; 32],
            },
        );
        assert_eq!(r, Err(ContractError::ZeroId));
        // decode 路径同门复验：合法 wire 的 plan_id 清零 → 拒绝。
        let mut wire = sealed_cert().encode_v1();
        wire[8 + 16..8 + 32].fill(0); // plan_id 域（cert_id 之后 16B）
        assert_eq!(CertifiedPlan::decode_v1(&wire), Err(ContractError::ZeroId));
    }

    /// 无 feature 时：CertifierSeal 与 CertifiedPlan::new_sealed 均
    /// crate 私有——外部唯一途径是 decode_v1（过渡发放面已按 ADR-051
    /// 出口条件删除：`transition-certifier` feature 与构造器均不存在）。
    #[test]
    fn decode_rejects_version_and_layout() {
        let mut wire = vec![b'C', b'E', b'R', b'T', 0x09, 0x00];
        wire.extend_from_slice(&[0u8; 200]);
        assert!(matches!(
            CertifiedPlan::decode_v1(&wire),
            Err(ContractError::VersionUnsupported { .. })
        ));
        assert_eq!(
            CertifiedPlan::decode_v1(b"NOPE"),
            Err(ContractError::Malformed("magic"))
        );
    }
}
