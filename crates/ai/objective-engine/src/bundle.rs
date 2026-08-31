//! 签名 RuleBundle（[09] §2.2 输入）：版本化规则束 + 内容 digest +
//! HMAC 签名。篡改（digest 复算不符）、回滚（版本退阶）、未知 kind
//! （构造期拒绝）在编译入口 fail-closed。

use alloc::vec::Vec;

use crate::compiler::CompileError;
use crate::spec::{ConstraintSpec, HorizonPlan, ObjectiveTerm};

/// 签名（站内规则签发 key 的 HMAC 绑定）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BundleSignature {
    /// 密钥 ID（验签方按 id 查 key ring）。
    pub key_id: u16,
    /// HMAC-SHA256(签发 key, bundle_content_digest || version)。
    pub mac: [u8; 32],
}

/// 规则束（公开字段只读；经 builder 校验构造）。
#[derive(Debug, Clone, PartialEq)]
pub struct RuleBundle {
    version: u64,
    issuer: [u8; 16],
    valid_from_ns: u64,
    valid_until_ns: u64,
    horizon: HorizonPlan,
    /// 站额定功率（per-unit 归一基准，kW；>0）。
    site_rating_kw: f64,
    /// 约束规则（构造期按 (priority, constraint_id) 排序）。
    constraints: Vec<ConstraintSpec>,
    /// 目标项（构造期按 term_id 排序）。
    terms: Vec<ObjectiveTerm>,
    /// 必备约束清单（安全底线 manifest；编译期缺一即拒）。
    required_constraints: Vec<u64>,
    /// 内容摘要（全部字段参与）。
    digest: [u8; 32],
    signature: Option<BundleSignature>,
}

impl RuleBundle {
    /// builder 入口。
    pub fn builder(issuer: [u8; 16], version: u64) -> RuleBundleBuilder {
        RuleBundleBuilder::new(issuer, version)
    }

    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn issuer(&self) -> &[u8; 16] {
        &self.issuer
    }
    pub fn valid_from_ns(&self) -> u64 {
        self.valid_from_ns
    }
    pub fn valid_until_ns(&self) -> u64 {
        self.valid_until_ns
    }
    pub fn horizon(&self) -> HorizonPlan {
        self.horizon
    }
    pub fn site_rating_kw(&self) -> f64 {
        self.site_rating_kw
    }
    pub fn constraints(&self) -> &[ConstraintSpec] {
        &self.constraints
    }
    pub fn terms(&self) -> &[ObjectiveTerm] {
        &self.terms
    }
    pub fn required_constraints(&self) -> &[u64] {
        &self.required_constraints
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub fn signature(&self) -> Option<&BundleSignature> {
        self.signature.as_ref()
    }

    /// 复算内容摘要并与登记值比对（防篡改锚）。
    pub fn verify_digest(&self) -> bool {
        self.compute_digest() == self.digest
    }

    /// 用 `key` 签发（幂等覆盖）。
    pub fn seal(&mut self, key_id: u16, key: &[u8]) {
        self.signature = Some(BundleSignature {
            key_id,
            mac: self.sign_with(key),
        });
    }

    /// 签名验证（key 按 key_id 由调用方提供）。
    pub fn verify_signature(&self, key_id: u16, key: &[u8]) -> Result<(), CompileError> {
        let sig = self
            .signature
            .ok_or(CompileError::BundleSignatureRequired)?;
        if sig.key_id != key_id {
            return Err(CompileError::BundleSignatureInvalid);
        }
        let expect = self.sign_with(key);
        if eneros_decision_contract::keyed::mac_eq(&expect, &sig.mac) {
            Ok(())
        } else {
            Err(CompileError::BundleSignatureInvalid)
        }
    }

    fn sign_with(&self, key: &[u8]) -> [u8; 32] {
        let mut input = Vec::with_capacity(40);
        input.extend_from_slice(&self.digest);
        input.extend_from_slice(&self.version.to_le_bytes());
        eneros_decision_contract::keyed::hmac_sha256(key, &input)
    }

    /// 测试专用：破坏登记 digest（模拟篡改后重签的载体）。
    #[cfg(test)]
    pub(crate) fn corrupt_digest_for_test(&mut self) {
        self.digest = [0u8; 32];
    }

    /// 内容摘要（digest/signature 不入自身；签名绑定 digest）。
    fn compute_digest(&self) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"eneros.rulebundle.v1");
        h.update(self.version.to_le_bytes());
        h.update(self.issuer);
        h.update(self.valid_from_ns.to_le_bytes());
        h.update(self.valid_until_ns.to_le_bytes());
        h.update(self.horizon.step_ns.to_le_bytes());
        h.update(self.horizon.step_count.to_le_bytes());
        h.update(self.site_rating_kw.to_le_bytes());
        h.update((self.constraints.len() as u64).to_le_bytes());
        for c in &self.constraints {
            h.update(c.rule_digest);
        }
        h.update((self.terms.len() as u64).to_le_bytes());
        for t in &self.terms {
            h.update(t.term_id.to_le_bytes());
            h.update(t.kind.to_le_bytes());
            h.update(t.weight.to_le_bytes());
            h.update(t.unit_code.to_le_bytes());
            h.update(t.normalization_id.to_le_bytes());
            h.update(t.priority.to_le_bytes());
            h.update(t.source_event_id);
        }
        h.update((self.required_constraints.len() as u64).to_le_bytes());
        for id in &self.required_constraints {
            h.update(id.to_le_bytes());
        }
        h.finalize().into()
    }
}

/// 规则束构建器（校验 + 排序 + digest 计算）。
pub struct RuleBundleBuilder {
    issuer: [u8; 16],
    version: u64,
    valid_from_ns: u64,
    valid_until_ns: u64,
    horizon: Option<HorizonPlan>,
    site_rating_kw: f64,
    constraints: Vec<ConstraintSpec>,
    terms: Vec<ObjectiveTerm>,
    required_constraints: Vec<u64>,
}

impl RuleBundleBuilder {
    pub fn new(issuer: [u8; 16], version: u64) -> Self {
        Self {
            issuer,
            version,
            valid_from_ns: 0,
            valid_until_ns: u64::MAX,
            horizon: None,
            site_rating_kw: 0.0,
            constraints: Vec::new(),
            terms: Vec::new(),
            required_constraints: Vec::new(),
        }
    }

    pub fn validity_window(mut self, from_ns: u64, until_ns: u64) -> Self {
        self.valid_from_ns = from_ns;
        self.valid_until_ns = until_ns;
        self
    }

    pub fn horizon(mut self, h: HorizonPlan) -> Self {
        self.horizon = Some(h);
        self
    }

    /// 站额定功率（>0；per-unit 归一基准）。
    pub fn site_rating_kw(mut self, kw: f64) -> Self {
        self.site_rating_kw = kw;
        self
    }

    pub fn add_constraint(mut self, c: ConstraintSpec) -> Self {
        self.constraints.push(c);
        self
    }

    pub fn add_term(mut self, t: ObjectiveTerm) -> Self {
        self.terms.push(t);
        self
    }

    /// 登记必备约束（安全底线 manifest）。
    pub fn required(mut self, ids: &[u64]) -> Self {
        self.required_constraints = ids.to_vec();
        self
    }

    pub fn build(self) -> Result<RuleBundle, CompileError> {
        if self.issuer == [0u8; 16] {
            return Err(CompileError::InvalidRule("issuer=0"));
        }
        if self.version == 0 {
            return Err(CompileError::InvalidRule("version=0"));
        }
        if self.valid_until_ns <= self.valid_from_ns {
            return Err(CompileError::InvalidRule("bundle window"));
        }
        let horizon = self.horizon.ok_or(CompileError::InvalidRule("horizon"))?;
        if self.site_rating_kw <= 0.0 || !self.site_rating_kw.is_finite() {
            return Err(CompileError::InvalidRule("site_rating_kw"));
        }
        // 硬约束至少一条（[09] §2.8：solver 只接受 ObjectiveSpec——无硬
        // 约束的规划没有安全边界）。
        if !self.constraints.iter().any(|c| c.hardness == 0) {
            return Err(CompileError::NoHardConstraints);
        }
        // 必备清单引用的 id 必须存在。
        for id in &self.required_constraints {
            if !self.constraints.iter().any(|c| &c.constraint_id == id) {
                return Err(CompileError::InvalidRule("required id not in bundle"));
            }
        }
        // 排序（确定性：顺序无关）。
        let mut constraints = self.constraints;
        constraints.sort_by_key(|a| (a.priority, a.constraint_id));
        let mut terms = self.terms;
        terms.sort_by_key(|a| a.term_id);
        // digest 重复检查（同 id 规则视为配置错误）。
        for w in constraints.windows(2) {
            if w[0].constraint_id == w[1].constraint_id {
                return Err(CompileError::InvalidRule("duplicate constraint_id"));
            }
        }
        for w in terms.windows(2) {
            if w[0].term_id == w[1].term_id {
                return Err(CompileError::InvalidRule("duplicate term_id"));
            }
        }
        let mut bundle = RuleBundle {
            version: self.version,
            issuer: self.issuer,
            valid_from_ns: self.valid_from_ns,
            valid_until_ns: self.valid_until_ns,
            horizon,
            site_rating_kw: self.site_rating_kw,
            constraints,
            terms,
            required_constraints: self.required_constraints,
            digest: [0u8; 32],
            signature: None,
        };
        bundle.digest = bundle.compute_digest();
        Ok(bundle)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use eneros_telemetry_model::unit_catalog;

    use super::*;
    use crate::layer;

    pub(super) fn soc_rule(id: u64, layer_p: u16, lo: f64, hi: f64) -> ConstraintSpec {
        ConstraintSpec::new(
            id,
            eneros_decision_contract::objective::ConstraintKind::MinSoc.code(),
            0,
            layer_p,
            crate::spec::ScopeRef::SITE,
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Dimensionless),
            lo,
            hi,
            0,
            u64::MAX,
            [1; 16],
        )
        .unwrap()
    }

    #[test]
    fn bundle_digest_binds_content_and_order_independent() {
        let mut c1 = soc_rule(1, layer::DEVICE_SAFETY, 0.1, 0.9);
        let c2 = soc_rule(2, layer::OPERATIONAL, 0.15, 0.85);
        // 改变 rule_digest 再复原 → digest 回到原值（内容绑定）。
        let b1 = RuleBundle::builder([7; 16], 3)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(c1.clone())
            .add_constraint(c2.clone())
            .build()
            .unwrap();
        c1.rule_digest = [9; 32];
        let b2 = RuleBundle::builder([7; 16], 3)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(c2)
            .add_constraint(c1)
            .build()
            .unwrap();
        // 内容被篡改（digest 变）→ 不同 bundle digest。
        assert_ne!(b1.digest(), b2.digest());
        // 但等价内容（顺序无关、digest 一致）：
        let b3 = RuleBundle::builder([7; 16], 3)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(soc_rule(1, layer::DEVICE_SAFETY, 0.1, 0.9))
            .add_constraint(soc_rule(2, layer::OPERATIONAL, 0.15, 0.85))
            .build()
            .unwrap();
        assert_eq!(b1.digest(), b3.digest(), "顺序无关 + 内容一致");
        assert!(b1.verify_digest());
    }

    #[test]
    fn bundle_seal_and_verify() {
        let mut b = RuleBundle::builder([7; 16], 1)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(soc_rule(1, layer::DEVICE_SAFETY, 0.1, 0.9))
            .build()
            .unwrap();
        b.seal(1, b"key-material");
        assert!(b.verify_signature(1, b"key-material").is_ok());
        assert!(matches!(
            b.verify_signature(1, b"wrong-key"),
            Err(CompileError::BundleSignatureInvalid)
        ));
        assert!(matches!(
            b.verify_signature(2, b"key-material"),
            Err(CompileError::BundleSignatureInvalid)
        ));
        // 篡改内容 → 验签失败（绑定 digest）。
        let mut tampered = b.clone();
        tampered.corrupt_digest_for_test();
        assert!(matches!(
            tampered.verify_signature(1, b"key-material"),
            Err(CompileError::BundleSignatureInvalid)
        ));
    }

    #[test]
    fn unsigned_bundle_verify_requires_signature() {
        let b = RuleBundle::builder([7; 16], 1)
            .validity_window(0, u64::MAX)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(100.0)
            .add_constraint(soc_rule(1, layer::DEVICE_SAFETY, 0.1, 0.9))
            .build()
            .unwrap();
        assert!(matches!(
            b.verify_signature(1, b"k"),
            Err(CompileError::BundleSignatureRequired)
        ));
    }

    #[test]
    fn builder_negatives() {
        let ok_rule = soc_rule(1, layer::DEVICE_SAFETY, 0.1, 0.9);
        // 无 horizon。
        assert!(RuleBundle::builder([7; 16], 1)
            .site_rating_kw(1.0)
            .add_constraint(ok_rule.clone())
            .build()
            .is_err());
        // 零 issuer。
        assert!(RuleBundle::builder([0; 16], 1)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(1.0)
            .add_constraint(ok_rule.clone())
            .build()
            .is_err());
        // 无硬约束。
        let soft_only = ConstraintSpec::new(
            1,
            eneros_decision_contract::objective::ConstraintKind::MaxPowerKw.code(),
            1,
            layer::ECONOMIC,
            crate::spec::ScopeRef::SITE,
            unit_catalog::code_of(eneros_telemetry_model::PhysicalUnit::Kilowatts),
            0.0,
            100.0,
            0,
            u64::MAX,
            [1; 16],
        )
        .unwrap();
        assert_eq!(
            RuleBundle::builder([7; 16], 1)
                .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
                .site_rating_kw(1.0)
                .add_constraint(soft_only)
                .build(),
            Err(CompileError::NoHardConstraints)
        );
        // 必备清单引用缺失 id。
        assert!(RuleBundle::builder([7; 16], 1)
            .horizon(HorizonPlan::new(900_000_000_000, 96).unwrap())
            .site_rating_kw(1.0)
            .add_constraint(ok_rule)
            .required(&[99])
            .build()
            .is_err());
    }
}
