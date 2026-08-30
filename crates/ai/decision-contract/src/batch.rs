//! `CommandBatch` — 有序命令批（[06] §3.3：构造函数只接受 `&CertifiedPlan`
//! 与 `&ResourceRegistry`）。
//!
//! `AuthorizedCommand` 不在本 crate：批内的每个 [`CommandIntent`] 仍须经
//! 唯一 ControlGateway 裁决才成为 AuthorizedCommand（逐意图，.40 前批大小
//! 生产=1）。

use alloc::vec::Vec;

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

/// 批大小上界（迁移期单 PCS=1；.40 Saga 卷放宽并加 Saga journal）。
pub const MAX_BATCH_SIZE: usize = 64;

/// 有序命令批（意图序 = 编译器裁决序；逐意图经 Gateway 授权）。
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
        Ok(Self {
            batch_id,
            plan_id: *cert.plan_id(),
            plan_sha256: *cert.plan_sha256(),
            certificate_id: *cert.certificate_id(),
            intents,
            catalog_version: registry.catalog_version(),
            built_at_ms,
        })
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;
    #[cfg(feature = "transition-certifier")]
    use crate::certificate::CertifierSeal;
    use crate::ids::{CertificateId, IntentId, SnapshotId};
    use crate::intent::{CompensationHint, Precondition};

    struct SinglePcs;

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
            CertificateId([9u8; 16]),
            PlanId([1u8; 16]),
            [0xAB; 32],
            SnapshotId([2u8; 16]),
            1,
            1, // catalog version
            1,
            1_000,
            61_000,
            1,
            [0xCD; 32],
        )
        .unwrap()
    }

    fn intent() -> CommandIntent {
        CommandIntent::new(
            IntentId([3u8; 16]),
            PlanId([1u8; 16]),
            [0xAB; 32],
            1,
            1,
            10,
            -50.0,
            6,
            1_000,
            60_000,
            Precondition::None,
            CompensationHint::None,
        )
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
        let bad = CommandIntent::new(
            *i.intent_id(),
            *i.plan_id(),
            [0xEE; 32],
            1,
            1,
            10,
            -50.0,
            6,
            1_000,
            60_000,
            Precondition::None,
            CompensationHint::None,
        )
        .unwrap();
        assert_eq!(
            CommandBatch::new([7u8; 16], &c, &SinglePcs, vec![bad], 2_000),
            Err(ContractError::CertificatePlanMismatch)
        );
        // 意图指向注册表之外的绑定（点/单位不符）。
        let wrong_point = CommandIntent::new(
            *i.intent_id(),
            *i.plan_id(),
            [0xAB; 32],
            1,
            1,
            99,
            -50.0,
            6,
            1_000,
            60_000,
            Precondition::None,
            CompensationHint::None,
        )
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
}
