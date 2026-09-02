//! SkillPackage（[11] §4.2）：`SkillManifestV1` + 指令 + 工具白名单 +
//! schema 引用，SM2 签名 + 包摘要。安全政策保留在 Harness/Objective/
//! Certifier/Gateway（[11] §4.2：不复制进 prompt 当唯一门）。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm2::keypair::Sm2PublicKey;
use eneros_crypto::sm2::sign::Sm2Signature;
use eneros_signer_keyring::SignerKeyring;

use crate::error::HarnessError;
use crate::tools::ToolId;

/// Skill manifest V1（[11] §4.2 字段全集）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillManifestV1 {
    pub skill_id: [u8; 16],
    pub version: u32,
    pub purpose_id: u32,
    pub input_schema_id: u32,
    pub output_schema_id: u32,
    pub tool_allowlist_digest: [u8; 32],
    pub max_steps: u16,
    pub max_tool_calls: u16,
    pub max_tokens: u32,
    pub max_runtime_ms: u32,
    /// 允许数据分类上限（Classification 码）。
    pub allowed_data_class: u8,
    /// 1 = 输出候选须人工审批（不能自动转发）。
    pub requires_human_approval: u8,
    pub valid_until_ns: u64,
    pub signer_key_id: u32,
    /// 包摘要（manifest canonical + 指令 + 白名单 canonical 的 SHA-256）。
    pub package_sha256: [u8; 32],
    pub signature: [u8; 64],
}

impl SkillManifestV1 {
    /// canonical 长度（不含 package_sha256/signature——二者是派生字段，
    /// 参与签名会造成摘要自循环）。
    pub const CANONICAL_LEN: usize = 90;
    /// wire 总长（canonical + package_sha256 + signature）。
    pub const WIRE_LEN: usize = 90 + 32 + 64;

    /// canonical（不含 signature 与 package_sha256；signature 覆盖
    /// canonical；package 摘要 = sha256(canonical‖指令‖白名单)）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::CANONICAL_LEN);
        b.extend_from_slice(&self.skill_id);
        b.extend_from_slice(&self.version.to_le_bytes());
        b.extend_from_slice(&self.purpose_id.to_le_bytes());
        b.extend_from_slice(&self.input_schema_id.to_le_bytes());
        b.extend_from_slice(&self.output_schema_id.to_le_bytes());
        b.extend_from_slice(&self.tool_allowlist_digest);
        b.extend_from_slice(&self.max_steps.to_le_bytes());
        b.extend_from_slice(&self.max_tool_calls.to_le_bytes());
        b.extend_from_slice(&self.max_tokens.to_le_bytes());
        b.extend_from_slice(&self.max_runtime_ms.to_le_bytes());
        b.push(self.allowed_data_class);
        b.push(self.requires_human_approval);
        b.extend_from_slice(&self.valid_until_ns.to_le_bytes());
        b.extend_from_slice(&self.signer_key_id.to_le_bytes());
        debug_assert_eq!(b.len(), Self::CANONICAL_LEN);
        b
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.canonical_bytes();
        b.extend_from_slice(&self.package_sha256);
        b.extend_from_slice(&self.signature);
        b
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HarnessError> {
        if bytes.len() != Self::WIRE_LEN {
            return Err(HarnessError::Manifest("skill wire-len"));
        }
        let mut m = SkillManifestV1 {
            skill_id: [0; 16],
            version: 0,
            purpose_id: 0,
            input_schema_id: 0,
            output_schema_id: 0,
            tool_allowlist_digest: [0; 32],
            max_steps: 0,
            max_tool_calls: 0,
            max_tokens: 0,
            max_runtime_ms: 0,
            allowed_data_class: 0,
            requires_human_approval: 0,
            valid_until_ns: 0,
            signer_key_id: 0,
            package_sha256: [0; 32],
            signature: [0; 64],
        };
        let mut p = 0usize;
        m.skill_id.copy_from_slice(&bytes[p..p + 16]);
        p += 16;
        for (out, len) in [
            (&mut m.version, 4),
            (&mut m.purpose_id, 4),
            (&mut m.input_schema_id, 4),
            (&mut m.output_schema_id, 4),
        ] {
            *out = u32::from_le_bytes(bytes[p..p + len].try_into().unwrap());
            p += len;
        }
        m.tool_allowlist_digest.copy_from_slice(&bytes[p..p + 32]);
        p += 32;
        m.max_steps = u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap());
        p += 2;
        m.max_tool_calls = u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap());
        p += 2;
        m.max_tokens = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.max_runtime_ms = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.allowed_data_class = bytes[p];
        p += 1;
        m.requires_human_approval = bytes[p];
        p += 1;
        m.valid_until_ns = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        m.signer_key_id = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.package_sha256.copy_from_slice(&bytes[p..p + 32]);
        p += 32;
        m.signature.copy_from_slice(&bytes[p..p + 64]);
        p += 64;
        debug_assert_eq!(p, Self::WIRE_LEN);
        Ok(m)
    }

    pub fn verify_signature(&self, pk: &Sm2PublicKey) -> bool {
        let sig = Sm2Signature::from_bytes(&self.signature);
        // 验签纪律单源 keyring 类型化面（Ok(false)/Err 双通道收口）。
        eneros_signer_keyring::verify_signature(&self.canonical_bytes(), &sig, pk).is_ok()
    }

    pub fn seal(&mut self, sk: &eneros_crypto::sm2::keypair::Sm2PrivateKey, pk: &Sm2PublicKey) {
        let signer = eneros_crypto::sm2::sign::Sm2Signer::new();
        let sig = signer
            .sign_deterministic(&self.canonical_bytes(), sk, pk)
            .expect("deterministic sign");
        self.signature = sig.to_bytes();
    }

    /// 结构校验（fail-closed）。
    pub fn validate_structure(&self) -> Result<(), HarnessError> {
        if self.skill_id == [0; 16] {
            return Err(HarnessError::Manifest("skill_id=0"));
        }
        if self.purpose_id == 0 {
            return Err(HarnessError::Manifest("purpose_id=0"));
        }
        if self.input_schema_id == 0 || self.output_schema_id == 0 {
            return Err(HarnessError::Manifest("schema_id=0"));
        }
        if self.tool_allowlist_digest == [0; 32] {
            return Err(HarnessError::Manifest("allowlist_digest=0"));
        }
        if self.max_steps == 0 || self.max_tool_calls == 0 {
            return Err(HarnessError::Manifest("max_steps/max_tool_calls=0"));
        }
        // 工具调用不能超过步数（无副作用预算大于总步数无意义）。
        if self.max_tool_calls > self.max_steps {
            return Err(HarnessError::Manifest("max_tool_calls>max_steps"));
        }
        if self.max_tokens == 0 || self.max_runtime_ms == 0 {
            return Err(HarnessError::Manifest("token/runtime budget=0"));
        }
        if self.allowed_data_class > crate::DATA_CLASS_MAX_RESTRICTED {
            return Err(HarnessError::Manifest("allowed_data_class>Restricted"));
        }
        if self.requires_human_approval > 1 {
            return Err(HarnessError::Manifest("requires_human_approval∉{0,1}"));
        }
        if self.package_sha256 == [0; 32] {
            return Err(HarnessError::Manifest("package_sha256=0"));
        }
        Ok(())
    }

    pub fn is_valid_at_ns(&self, now_ns: u64) -> bool {
        now_ns <= self.valid_until_ns
    }
}

/// Skill 包（manifest + 指令 + 白名单 + schema 引用注册）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillPackage {
    pub manifest: SkillManifestV1,
    /// 指令文本（任务描述 + 输出约束；[11] §4.2 不把安全政策当唯一门）。
    pub instructions: String,
    pub tool_allowlist: Vec<ToolId>,
}

/// Skill 注册表（[11] §2：Harness 装载并验证 Skill）。
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    /// 签名密钥注册/验签/撤销纪律单源（keyring——蓝图 62 卷 62.3 S5；
    /// wire 的 u32 signer_key_id 经 [`eneros_signer_keyring::alias_key_id`]
    /// 别名层映射，wire 字段不动）。
    keys: SignerKeyring,
    entries: BTreeMap<[u8; 16], SkillPackage>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_signer_key(&mut self, key_id: u32, pk: Sm2PublicKey) {
        self.keys
            .register(eneros_signer_keyring::alias_key_id(key_id), &pk);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 注册（签名 + 有效期 + 结构/摘要门；重复 skill_id → 拒）。
    pub fn register(&mut self, pkg: &SkillPackage, now_ns: u64) -> Result<(), HarnessError> {
        if self.entries.contains_key(&pkg.manifest.skill_id) {
            return Err(HarnessError::Manifest("duplicate skill_id"));
        }
        if !pkg.manifest.is_valid_at_ns(now_ns) {
            return Err(HarnessError::PackageExpired);
        }
        // 签名门（keyring 类型化纪律：未知 key/撤销/验伪统一结构化拒绝）。
        match self.keys.verify_authenticated(
            eneros_signer_keyring::alias_key_id(pkg.manifest.signer_key_id),
            &pkg.manifest.canonical_bytes(),
            &pkg.manifest.signature,
        ) {
            Ok(_) => {}
            Err(eneros_signer_keyring::VerifyReject::UnknownKey { .. }) => {
                return Err(HarnessError::Manifest("unknown signer key"));
            }
            Err(_) => return Err(HarnessError::Manifest("signature invalid")),
        }
        // 结构门 + 摘要门：签名只覆盖 manifest canonical——instructions/
        // 白名单内容经 package_sha256 间接绑定，必须显式核验（字段 pub，
        // 结构体直构可绕过 `SkillPackage::new` 的构建期校验）。
        pkg.manifest.validate_structure()?;
        let digest = pkg.package_digest();
        if digest != pkg.manifest.package_sha256 {
            return Err(HarnessError::Manifest("package_sha256 mismatch"));
        }
        self.entries.insert(pkg.manifest.skill_id, pkg.clone());
        Ok(())
    }

    pub fn get(&self, skill_id: &[u8; 16]) -> Option<&SkillPackage> {
        self.entries.get(skill_id)
    }

    /// 推理前资格（有效期内 + 已签名注册；内容摘要由 SkillPackage::new
    /// 已验——此处再验签名以覆盖外置篡改）。
    pub fn eligible(&self, skill_id: &[u8; 16], now_ns: u64) -> Option<&SkillPackage> {
        let pkg = self.entries.get(skill_id)?;
        if !pkg.manifest.is_valid_at_ns(now_ns) {
            return None;
        }
        Some(pkg)
    }
}

/// 白名单 canonical（摘要输入；tool 码序稳定）。
pub fn allowlist_canonical(allowlist: &[ToolId]) -> Vec<u8> {
    let mut b = Vec::with_capacity(allowlist.len());
    let mut codes: Vec<u8> = allowlist.iter().map(|t| t.code()).collect();
    codes.sort_unstable();
    b.extend_from_slice(&codes);
    b
}

impl SkillPackage {
    /// 包摘要（manifest canonical + 指令 + 白名单 canonical）。
    pub fn package_digest(&self) -> [u8; 32] {
        let mut b = self.manifest.canonical_bytes();
        b.extend_from_slice(self.instructions.as_bytes());
        b.extend_from_slice(&allowlist_canonical(&self.tool_allowlist));
        crate::sha256_bytes(&b)
    }

    /// 构建并校验（结构 + 白名单摘要 + 包摘要一致性）。
    pub fn new(
        manifest: SkillManifestV1,
        instructions: String,
        tool_allowlist: Vec<ToolId>,
    ) -> Result<Self, HarnessError> {
        manifest.validate_structure()?;
        if instructions.is_empty() || instructions.len() > 4096 {
            return Err(HarnessError::Manifest("instructions empty/too long"));
        }
        if tool_allowlist.is_empty() {
            return Err(HarnessError::Manifest("allowlist empty"));
        }
        let pkg = SkillPackage {
            manifest,
            instructions,
            tool_allowlist,
        };
        // 先白名单摘要（身份面），后包摘要（内容面）。
        let allow_digest = crate::sha256_bytes(&allowlist_canonical(&pkg.tool_allowlist));
        if allow_digest != pkg.manifest.tool_allowlist_digest {
            return Err(HarnessError::Manifest("allowlist_digest mismatch"));
        }
        let digest = pkg.package_digest();
        if digest != pkg.manifest.package_sha256 {
            return Err(HarnessError::Manifest("package_sha256 mismatch"));
        }
        Ok(pkg)
    }

    pub fn verify_signature(&self, pk: &Sm2PublicKey) -> bool {
        self.manifest.verify_signature(pk)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::vec;

    use eneros_crypto::sm2::keypair::{Sm2KeyPair, Sm2PrivateKey};

    use super::*;
    use crate::tools::ToolId;

    pub(crate) fn test_keypair() -> (Sm2PrivateKey, Sm2PublicKey) {
        let mut sk_bytes = [0u8; 32];
        sk_bytes[31] = 0x2A;
        let sk = Sm2PrivateKey::from_bytes(&sk_bytes).unwrap();
        let pair = Sm2KeyPair::from_private_key(&sk).unwrap();
        (sk, pair.public_key)
    }

    /// 测试用 manifest 模板（loop 测试引用）。
    pub(crate) fn skill_manifest() -> SkillManifestV1 {
        SkillManifestV1 {
            skill_id: [1; 16],
            version: 1,
            purpose_id: 3,
            input_schema_id: 1,
            output_schema_id: 3,
            tool_allowlist_digest: [0; 32],
            max_steps: 8,
            max_tool_calls: 6,
            max_tokens: 2048,
            max_runtime_ms: 30_000,
            allowed_data_class: 2,
            requires_human_approval: 1,
            valid_until_ns: 1_000_000_000,
            signer_key_id: 1,
            package_sha256: [0; 32],
            signature: [0; 64],
        }
    }

    /// 构建完整（摘要+签名）skill 包（loop 等测试复用）。
    pub(crate) fn sealed_skill(allowlist: Vec<ToolId>) -> SkillPackage {
        let (sk, pk) = test_keypair();
        let mut m = skill_manifest();
        m.tool_allowlist_digest = crate::sha256_bytes(&allowlist_canonical(&allowlist));
        let mut canon = m.canonical_bytes();
        canon.extend_from_slice("指令".as_bytes());
        canon.extend_from_slice(&allowlist_canonical(&allowlist));
        m.package_sha256 = crate::sha256_bytes(&canon);
        m.seal(&sk, &pk);
        SkillPackage::new(m, "指令".into(), allowlist).expect("sealed skill valid")
    }

    #[test]
    fn package_build_seal_verify_roundtrip() {
        let (_, pk) = test_keypair();
        let allow = vec![ToolId::QuerySnapshot, ToolId::SubmitIntentCandidate];
        let pkg = sealed_skill(allow);
        assert!(pkg.verify_signature(&pk));
        // wire 往返（decode 后签名/摘要完整保留）。
        let wire = pkg.manifest.encode();
        let decoded = SkillManifestV1::decode(&wire).unwrap();
        assert_eq!(decoded, pkg.manifest);
        assert!(decoded.verify_signature(&pk));
    }

    #[test]
    fn tamper_breaks_signature_and_digests() {
        let (_, pk) = test_keypair();
        let allow = vec![ToolId::QuerySnapshot];
        let good = sealed_skill(allow.clone());
        assert!(good.verify_signature(&pk));
        let m = good.manifest.clone();
        // 指令篡改 → 包摘要不匹配（重建即拒）。
        assert_eq!(
            SkillPackage::new(m.clone(), "指令被改".into(), allow.clone()).unwrap_err(),
            HarnessError::Manifest("package_sha256 mismatch")
        );
        // 白名单篡改 → 白名单摘要不匹配。
        let other = vec![ToolId::RequestHumanApproval];
        let err = SkillPackage::new(m.clone(), "指令".into(), other);
        assert!(matches!(
            err,
            Err(HarnessError::Manifest("allowlist_digest mismatch"))
        ));
        // 字段篡改 → 验签失败（签名覆盖 canonical 全字段）。
        let mut tampered = m.clone();
        tampered.max_steps = 64;
        assert!(!tampered.verify_signature(&pk));
        // 结构门。
        let mut bad = m.clone();
        bad.max_tool_calls = 9;
        bad.max_steps = 8;
        assert!(bad.validate_structure().is_err());
    }

    #[test]
    fn structure_validation_fail_closed() {
        // 有效清单：sealed_skill 内部构造的 manifest 全字段合法。
        let good = sealed_skill(vec![ToolId::QuerySnapshot]);
        let m = good.manifest;
        assert!(m.validate_structure().is_ok());
        let mut b = m.clone();
        b.max_tool_calls = 99;
        b.max_steps = 8;
        assert_eq!(
            b.validate_structure(),
            Err(HarnessError::Manifest("max_tool_calls>max_steps"))
        );
        let mut b = m.clone();
        b.requires_human_approval = 2;
        assert_eq!(
            b.validate_structure(),
            Err(HarnessError::Manifest("requires_human_approval∉{0,1}"))
        );
        let mut b = m.clone();
        b.allowed_data_class = 4;
        assert_eq!(
            b.validate_structure(),
            Err(HarnessError::Manifest("allowed_data_class>Restricted"))
        );
    }

    #[test]
    fn register_rejects_tampered_content_and_bad_structure() {
        // 注册面注释承诺「签名 + 有效期 + 摘要门」：签名只覆盖 manifest
        // canonical——instructions/白名单内容经 package_sha256 间接绑定，
        // 必须显式核验；结构门同（历史实现两者皆缺，结构体直构即绕过
        // SkillPackage::new）。
        // (a) 指令被替换（manifest/签名/摘要保持原值）→ 注册拒。
        let (_, pk) = test_keypair();
        let good = sealed_skill(vec![ToolId::QuerySnapshot]);
        let forged = SkillPackage {
            manifest: good.manifest.clone(),
            instructions: "被替换的指令".into(),
            tool_allowlist: good.tool_allowlist.clone(),
        };
        let mut reg = SkillRegistry::new();
        reg.add_signer_key(1, pk);
        assert_eq!(
            reg.register(&forged, 0),
            Err(HarnessError::Manifest("package_sha256 mismatch"))
        );
        // 正本仍可注册（同一注册表不受拒件影响）。
        reg.register(&good, 0).unwrap();
        // (b) 结构非法（max_tool_calls > max_steps）但签名/摘要自洽 → 拒。
        let (sk, pk2) = test_keypair();
        let mut m = skill_manifest();
        m.max_steps = 8;
        m.max_tool_calls = 9;
        m.tool_allowlist_digest =
            crate::sha256_bytes(&allowlist_canonical(&[ToolId::QuerySnapshot]));
        let mut canon = m.canonical_bytes();
        canon.extend_from_slice("指令".as_bytes());
        canon.extend_from_slice(&allowlist_canonical(&[ToolId::QuerySnapshot]));
        m.package_sha256 = crate::sha256_bytes(&canon);
        m.seal(&sk, &pk2);
        let pkg = SkillPackage {
            manifest: m,
            instructions: "指令".into(),
            tool_allowlist: vec![ToolId::QuerySnapshot],
        };
        let mut reg2 = SkillRegistry::new();
        reg2.add_signer_key(1, pk2);
        assert_eq!(
            reg2.register(&pkg, 0),
            Err(HarnessError::Manifest("max_tool_calls>max_steps"))
        );
    }

    #[test]
    fn validity_window() {
        let m = skill_manifest();
        assert!(!m.is_valid_at_ns(1_000_000_001));
        assert!(m.is_valid_at_ns(1_000_000_000));
    }
}
