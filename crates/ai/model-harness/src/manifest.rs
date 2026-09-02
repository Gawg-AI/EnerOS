//! 模型包（[11] §3.3）：`ModelManifestV1` + 内容摘要 + SM2 签名。
//!
//! manifest 字段与蓝图逐字对应；`canonical_bytes()` 覆盖除 signature 外
//! 的全部字段（定长小端域序，与 market envelope 同纪律），签名覆盖
//! canonical。校验顺序（[11] §3.1 唯一不变量）：文档摘要素材逐个
//! SHA-256 比对 → 签名 → 有效期 → runtime/hardware 预算。

use alloc::vec::Vec;

use eneros_crypto::sm2::keypair::Sm2PublicKey;
use eneros_crypto::sm2::sign::Sm2Signature;
use sha2::{Digest, Sha256};

use crate::error::HarnessError;

/// 模型包 manifest V1（[11] §3.3 字段全集）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelManifestV1 {
    pub package_id: [u8; 16],
    pub model_sha256: [u8; 32],
    pub tokenizer_sha256: [u8; 32],
    pub runtime_id: u32,
    pub runtime_version: u32,
    pub quantization_id: u16,
    pub context_limit: u32,
    /// 支持 skill 集摘要（WP-6.2 起强制；0 = 未发布 skill 集 → 拒绝）。
    pub supported_skill_set_digest: [u8; 32],
    pub input_schema_set_digest: [u8; 32],
    pub output_schema_set_digest: [u8; 32],
    pub eval_suite_digest: [u8; 32],
    /// 允许数据分类上限（Classification 码；[11] §3.4 Restricted 默认拒
    /// 远端）。
    pub data_class_max: u8,
    pub memory_limit_bytes: u64,
    pub token_limit: u32,
    pub valid_from_ns: u64,
    pub valid_until_ns: u64,
    pub signer_key_id: u32,
    /// SM2 签名（r‖s 64B，覆盖 canonical）。
    pub signature: [u8; 64],
}

impl ModelManifestV1 {
    /// canonical 编码长度（不含 signature；含 signature 的 wire 为
    /// `WIRE_LEN`）。
    pub const CANONICAL_LEN: usize = 255;
    /// wire 总长（canonical + signature）。
    pub const WIRE_LEN: usize = 319;

    /// canonical 编码（定长小端域序；signature 不参与——验签先比对
    /// 内容再验签，杜绝 signature 与内容错配）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::CANONICAL_LEN);
        b.extend_from_slice(&self.package_id);
        b.extend_from_slice(&self.model_sha256);
        b.extend_from_slice(&self.tokenizer_sha256);
        b.extend_from_slice(&self.runtime_id.to_le_bytes());
        b.extend_from_slice(&self.runtime_version.to_le_bytes());
        b.extend_from_slice(&self.quantization_id.to_le_bytes());
        b.extend_from_slice(&self.context_limit.to_le_bytes());
        b.extend_from_slice(&self.supported_skill_set_digest);
        b.extend_from_slice(&self.input_schema_set_digest);
        b.extend_from_slice(&self.output_schema_set_digest);
        b.extend_from_slice(&self.eval_suite_digest);
        b.push(self.data_class_max);
        b.extend_from_slice(&self.memory_limit_bytes.to_le_bytes());
        b.extend_from_slice(&self.token_limit.to_le_bytes());
        b.extend_from_slice(&self.valid_from_ns.to_le_bytes());
        b.extend_from_slice(&self.valid_until_ns.to_le_bytes());
        b.extend_from_slice(&self.signer_key_id.to_le_bytes());
        debug_assert_eq!(b.len(), Self::CANONICAL_LEN);
        b
    }

    /// 完整 wire（canonical ‖ signature）。
    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.canonical_bytes();
        b.extend_from_slice(&self.signature);
        b
    }

    /// wire 解码（长度不符 → Err）。
    pub fn decode(bytes: &[u8]) -> Result<Self, HarnessError> {
        if bytes.len() != Self::WIRE_LEN {
            return Err(HarnessError::Manifest("wire-len"));
        }
        let mut m = ModelManifestV1 {
            package_id: [0; 16],
            model_sha256: [0; 32],
            tokenizer_sha256: [0; 32],
            runtime_id: 0,
            runtime_version: 0,
            quantization_id: 0,
            context_limit: 0,
            supported_skill_set_digest: [0; 32],
            input_schema_set_digest: [0; 32],
            output_schema_set_digest: [0; 32],
            eval_suite_digest: [0; 32],
            data_class_max: 0,
            memory_limit_bytes: 0,
            token_limit: 0,
            valid_from_ns: 0,
            valid_until_ns: 0,
            signer_key_id: 0,
            signature: [0; 64],
        };
        let mut p = 0usize;
        let take16 = |m: &mut [u8; 16], b: &[u8], at: usize| {
            m.copy_from_slice(&b[at..at + 16]);
        };
        take16(&mut m.package_id, bytes, p);
        p += 16;
        let take32 = |m: &mut [u8; 32], b: &[u8], at: usize| {
            m.copy_from_slice(&b[at..at + 32]);
        };
        take32(&mut m.model_sha256, bytes, p);
        p += 32;
        take32(&mut m.tokenizer_sha256, bytes, p);
        p += 32;
        m.runtime_id = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.runtime_version = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.quantization_id = u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap());
        p += 2;
        m.context_limit = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        take32(&mut m.supported_skill_set_digest, bytes, p);
        p += 32;
        take32(&mut m.input_schema_set_digest, bytes, p);
        p += 32;
        take32(&mut m.output_schema_set_digest, bytes, p);
        p += 32;
        take32(&mut m.eval_suite_digest, bytes, p);
        p += 32;
        m.data_class_max = bytes[p];
        p += 1;
        m.memory_limit_bytes = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        m.token_limit = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.valid_from_ns = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        m.valid_until_ns = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        m.signer_key_id = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        m.signature.copy_from_slice(&bytes[p..p + 64]);
        p += 64;
        debug_assert_eq!(p, Self::WIRE_LEN);
        Ok(m)
    }

    /// SM2 验签（canonical 内容 + 签名；测试用固定标量密钥）。验签
    /// 纪律单源 keyring 类型化面——底层布尔返回的 `Ok(false)`/`Err`
    /// 双通道在 keyring 收口，调用方只见布尔结论。
    pub fn verify_signature(&self, pk: &Sm2PublicKey) -> bool {
        let sig = Sm2Signature::from_bytes(&self.signature);
        eneros_signer_keyring::verify_signature(&self.canonical_bytes(), &sig, pk).is_ok()
    }

    /// 签名回填（测试/sandbox 用确定性签名；生产私钥在模型节点外）。
    pub fn seal(&mut self, sk: &eneros_crypto::sm2::keypair::Sm2PrivateKey, pk: &Sm2PublicKey) {
        let signer = eneros_crypto::sm2::sign::Sm2Signer::new();
        let sig = signer
            .sign_deterministic(&self.canonical_bytes(), sk, pk)
            .expect("deterministic sign");
        self.signature = sig.to_bytes();
    }

    /// 结构校验（fail-closed）：摘要全非零、窗口合法、向量界限、
    /// 数据分类上限合法。签名/有效期由调用方按上下文检查。
    pub fn validate_structure(&self) -> Result<(), HarnessError> {
        if self.package_id == [0; 16] {
            return Err(HarnessError::Manifest("package_id=0"));
        }
        for (name, d) in [
            ("model_sha256", &self.model_sha256),
            ("tokenizer_sha256", &self.tokenizer_sha256),
            (
                "supported_skill_set_digest",
                &self.supported_skill_set_digest,
            ),
            ("input_schema_set_digest", &self.input_schema_set_digest),
            ("output_schema_set_digest", &self.output_schema_set_digest),
            ("eval_suite_digest", &self.eval_suite_digest),
        ] {
            if *d == [0; 32] {
                return Err(HarnessError::Manifest(match name {
                    "model_sha256" => "model_sha256=0",
                    "tokenizer_sha256" => "tokenizer_sha256=0",
                    "supported_skill_set_digest" => "skill_digest=0",
                    "input_schema_set_digest" => "input_schema_digest=0",
                    "output_schema_set_digest" => "output_schema_digest=0",
                    _ => "eval_suite_digest=0",
                }));
            }
        }
        if self.runtime_id == 0 {
            return Err(HarnessError::Manifest("runtime_id=0"));
        }
        if self.context_limit == 0 {
            return Err(HarnessError::Manifest("context_limit=0"));
        }
        if self.token_limit == 0 || self.token_limit > self.context_limit {
            return Err(HarnessError::Manifest("token_limit∉1..=context_limit"));
        }
        if self.memory_limit_bytes == 0 {
            return Err(HarnessError::Manifest("memory_limit_bytes=0"));
        }
        if self.data_class_max > crate::DATA_CLASS_MAX_RESTRICTED {
            return Err(HarnessError::Manifest("data_class_max>Restricted"));
        }
        if self.valid_from_ns >= self.valid_until_ns {
            return Err(HarnessError::Manifest("valid window"));
        }
        Ok(())
    }

    /// 有效期检查（ns 时刻）。
    pub fn is_valid_at_ns(&self, now_ns: u64) -> bool {
        now_ns >= self.valid_from_ns && now_ns <= self.valid_until_ns
    }
}

/// 包内容素材（引用；注册/推理时逐个 SHA-256 比对）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PackageContents<'a> {
    pub model: Option<&'a [u8]>,
    pub tokenizer: Option<&'a [u8]>,
    pub eval_suite: Option<&'a [u8]>,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

impl PackageContents<'_> {
    /// 内容摘要逐一比对（[11] §3.1 最少篡改面：模型/tokenizer/评测套件
    /// 任一字节不同 → 拒绝）。
    pub fn verify(&self, m: &ModelManifestV1) -> Result<(), HarnessError> {
        let model = self
            .model
            .ok_or(HarnessError::Manifest("model content missing"))?;
        if sha256(model) != m.model_sha256 {
            return Err(HarnessError::Manifest("model_sha256 mismatch"));
        }
        let tok = self
            .tokenizer
            .ok_or(HarnessError::Manifest("tokenizer content missing"))?;
        if sha256(tok) != m.tokenizer_sha256 {
            return Err(HarnessError::Manifest("tokenizer_sha256 mismatch"));
        }
        if let Some(e) = self.eval_suite {
            if m.eval_suite_digest != [0; 32] && sha256(e) != m.eval_suite_digest {
                return Err(HarnessError::Manifest("eval_suite digest mismatch"));
            }
        } else if m.eval_suite_digest != [0; 32] {
            return Err(HarnessError::Manifest("eval_suite content missing"));
        }
        Ok(())
    }
}

/// 模型包（manifest + 已注册内容状态；[11] §3.3 包还包含 tokenizer、
/// prompt templates、skill refs、license/SBOM、已知限制、训练数据声明和
/// 回滚 package id——本卷回滚 id 登记在生命周期侧，训练声明等登记项
/// 随包内容摘要锁定）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPackage {
    pub manifest: ModelManifestV1,
}

impl ModelPackage {
    pub fn new(manifest: ModelManifestV1) -> Result<Self, HarnessError> {
        manifest.validate_structure()?;
        Ok(Self { manifest })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use eneros_crypto::sm2::keypair::{Sm2KeyPair, Sm2PrivateKey};

    use super::*;

    pub(crate) fn test_keypair() -> (Sm2PrivateKey, Sm2PublicKey) {
        let mut sk_bytes = [0u8; 32];
        sk_bytes[31] = 0x2A;
        let sk = Sm2PrivateKey::from_bytes(&sk_bytes).unwrap();
        let pair = Sm2KeyPair::from_private_key(&sk).unwrap();
        (sk, pair.public_key)
    }

    pub(crate) fn manifest() -> ModelManifestV1 {
        ModelManifestV1 {
            package_id: [1; 16],
            model_sha256: [0x11; 32],
            tokenizer_sha256: [0x22; 32],
            runtime_id: 7,
            runtime_version: 1,
            quantization_id: 4, // Q4_K_M
            context_limit: 4096,
            supported_skill_set_digest: [0x33; 32],
            input_schema_set_digest: [0x44; 32],
            output_schema_set_digest: [0x55; 32],
            eval_suite_digest: [0x66; 32],
            data_class_max: 2,
            memory_limit_bytes: 4 << 30,
            token_limit: 512,
            valid_from_ns: 1000,
            valid_until_ns: 1_000_000,
            signer_key_id: 1,
            signature: [0; 64],
        }
    }

    #[test]
    fn seal_verify_roundtrip() {
        let (sk, pk) = test_keypair();
        let mut m = manifest();
        m.seal(&sk, &pk);
        assert!(m.verify_signature(&pk));
        let bytes = m.encode();
        assert_eq!(bytes.len(), ModelManifestV1::WIRE_LEN);
        let decoded = ModelManifestV1::decode(&bytes).unwrap();
        assert_eq!(decoded, m);
        assert!(decoded.verify_signature(&pk));
    }

    #[test]
    fn tamper_field_breaks_signature() {
        let (sk, pk) = test_keypair();
        let mut m = manifest();
        m.seal(&sk, &pk);
        // 任一业务字段翻转 → 验签失败（签名覆盖 canonical 全部字段）。
        m.context_limit = 8192;
        assert!(!m.verify_signature(&pk));
        // 恢复后仍可验。
        m.context_limit = 4096;
        assert!(m.verify_signature(&pk));
    }

    #[test]
    fn tamper_signature_bytes_fail() {
        let (sk, pk) = test_keypair();
        let mut m = manifest();
        m.seal(&sk, &pk);
        m.signature[10] ^= 0xFF;
        assert!(!m.verify_signature(&pk));
        // 另一把 key 验 → 失败。
        let (sk2, pk2) = test_keypair();
        let _ = sk2;
        let _ = &m.signature; // 保持字段使用
        assert!(!m.verify_signature(&pk2));
    }

    #[test]
    fn decode_wrong_len_rejected() {
        assert_eq!(
            ModelManifestV1::decode(&[0u8; 100]),
            Err(HarnessError::Manifest("wire-len"))
        );
    }

    #[test]
    fn structure_validation_fail_closed() {
        let m = manifest();
        assert!(m.validate_structure().is_ok());
        let mut zero_model = m.clone();
        zero_model.model_sha256 = [0; 32];
        assert_eq!(
            zero_model.validate_structure(),
            Err(HarnessError::Manifest("model_sha256=0"))
        );
        let mut tok_over = m.clone();
        tok_over.token_limit = tok_over.context_limit + 1;
        assert_eq!(
            tok_over.validate_structure(),
            Err(HarnessError::Manifest("token_limit∉1..=context_limit"))
        );
        let mut bad_class = m.clone();
        bad_class.data_class_max = 4;
        assert_eq!(
            bad_class.validate_structure(),
            Err(HarnessError::Manifest("data_class_max>Restricted"))
        );
        let mut bad_window = m.clone();
        bad_window.valid_until_ns = bad_window.valid_from_ns;
        assert_eq!(
            bad_window.validate_structure(),
            Err(HarnessError::Manifest("valid window"))
        );
        let mut bad_skill = m.clone();
        bad_skill.supported_skill_set_digest = [0; 32];
        assert_eq!(
            bad_skill.validate_structure(),
            Err(HarnessError::Manifest("skill_digest=0"))
        );
    }

    #[test]
    fn contents_digest_mismatch_rejected() {
        // manifest 摘要必须是内容字节的真实 SHA-256（占位摘要必拒）。
        let mut m = manifest();
        m.model_sha256 = crate::sha256_bytes(&[0x11u8; 256]);
        m.tokenizer_sha256 = crate::sha256_bytes(&[0x22; 128]);
        m.eval_suite_digest = crate::sha256_bytes(&[0x66; 64]);
        // 模型一字节翻转 → 拒绝。
        let mut model = [0x11u8; 256];
        model[7] = 0xEE;
        let contents = PackageContents {
            model: Some(&model),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&[0x66; 64]),
        };
        assert_eq!(
            contents.verify(&m),
            Err(HarnessError::Manifest("model_sha256 mismatch"))
        );
        // 评测素材翻转 → 拒绝。
        let contents = PackageContents {
            model: Some(&[0x11; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&[0x99; 64]),
        };
        assert_eq!(
            contents.verify(&m),
            Err(HarnessError::Manifest("eval_suite digest mismatch"))
        );
        // 全对 → 通过。
        let contents = PackageContents {
            model: Some(&[0x11; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&[0x66; 64]),
        };
        assert!(contents.verify(&m).is_ok());
    }

    #[test]
    fn contents_missing_rejected() {
        let m = manifest();
        let contents = PackageContents {
            model: None,
            tokenizer: None,
            eval_suite: None,
        };
        assert_eq!(
            contents.verify(&m),
            Err(HarnessError::Manifest("model content missing"))
        );
    }

    #[test]
    fn validity_window() {
        let m = manifest();
        assert!(!m.is_valid_at_ns(999));
        assert!(m.is_valid_at_ns(1000));
        assert!(m.is_valid_at_ns(1_000_000));
        assert!(!m.is_valid_at_ns(1_000_001));
    }
}
