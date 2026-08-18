//! WP-37.7：唯一可信启动链——签名域构造与域校验（纯逻辑，host 可测）。
//!
//! 蓝图要求：boot manager 只调用 `BootSignatureProvider`（具体 Ed25519/SM2
//! 由 36 批准 provider 实现）；key ID、algorithm ID、security version、
//! product/SKU scope 都进入签名域；test/dev key 编译进 release 失败；
//! rollback counter 未知/不可读时 fail closed；recovery/factory key 域与
//! normal release 分离；所有服务/配置/模型 digest 由同一 ProductManifest
//! 验证（不能后加载未签 payload）。
//!
//! 本模块提供：签名域类型 + 域校验（算法/域分离/版本单调/dev key）+ trait；
//! 密码学 verify 由 provider 实现（36 crypto-policy 批准）。

/// WP-37.7：签名算法 ID（36 crypto-policy 批准集合；未知算法拒绝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// Ed25519（当前正式 UEFI 链；36 policy boot_signature 批准）。
    Ed25519,
    /// SM2（36 crypto-policy 中列为未批准用途时不得用于 release）。
    Sm2,
}

/// WP-37.7：密钥域（recovery/factory 与 normal 分离——recovery key 不能
/// 启动 normal image）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDomain {
    /// 正常发布域。
    Normal,
    /// 恢复域（仅恢复介质）。
    Recovery,
    /// 工厂域（仅工厂/产线介质）。
    Factory,
}

/// WP-37.7：签名域——**进入签名运算的全部绑定字段**（key ID/algorithm/
/// security version/scope + 被签名负载 digest）。任何字段漂移即验签失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureDomain {
    /// 算法 ID。
    pub algorithm: SignatureAlgorithm,
    /// key ID（公钥身份摘要；release 构建只记录 public identity/digest，
    /// 不泄露私钥）。
    pub key_id: [u8; 32],
    /// 单调安全版本（downgrade 拒绝）。
    pub security_version: u64,
    /// 密钥域。
    pub key_domain: KeyDomain,
    /// 被签名负载 digest（manifest/服务 ELF/DTB/config/model 之一）。
    pub payload_digest: [u8; 32],
}

/// 签名域校验错误（fail-closed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// 算法与批准/期望不符（混用算法 ID 与签名 bytes）。
    AlgorithmMismatch,
    /// 密钥域不符（recovery/factory key 验 normal image 等）。
    KeyDomainMismatch,
    /// 安全版本降级（offered < recorded）。
    SecurityVersionDowngrade { recorded: u64, offered: u64 },
    /// dev/test key 用于 release（编译进 release 失败）。
    DevKeyRejected,
    /// payload digest 与签名覆盖内容不符（替换负载）。
    PayloadDigestMismatch,
    /// rollback counter 未知/不可读（fail closed）。
    RollbackUnreadable,
    /// 签名 bytes 与算法长度不符（混用算法字节）。
    SignatureLengthMismatch,
    /// 公钥格式非法（provider 密码学侧）。
    InvalidKey,
    /// 密码学验签失败（provider 密码学侧；域检查已先行通过）。
    InvalidSignature,
}

/// WP-37.7：签名长度（按算法固定；长度不符即算法混用）。
impl SignatureAlgorithm {
    /// 该算法签名的固定字节长度。
    pub fn signature_len(&self) -> usize {
        match self {
            Self::Ed25519 => 64,
            Self::Sm2 => 64, // DER 长度可变，SM2 批准前不走 release；占位 64
        }
    }
}

/// WP-37.7：域校验（provider 无关纯逻辑；provider 负责密码学 verify）：
/// 1. 算法匹配；
/// 2. 签名长度匹配（混用算法 bytes 拒绝）；
/// 3. 密钥域匹配（normal image 必须 normal 域 key；recovery/factory 分离）；
/// 4. security version 单调（offered >= recorded；downgrade 拒绝）；
/// 5. dev/test key 在 release 下拒绝；
/// 6. 负载 digest 由 provider 比对（本函数校验 digest 非全零——零 digest
///    即未绑定负载，拒绝）。
pub fn check_signature_domain(
    domain: &SignatureDomain,
    expected_algorithm: SignatureAlgorithm,
    expected_domain: KeyDomain,
    recorded_security_version: u64,
    allow_dev_key: bool,
    signature: &[u8],
) -> Result<(), SignatureError> {
    if domain.algorithm != expected_algorithm {
        return Err(SignatureError::AlgorithmMismatch);
    }
    if signature.len() != expected_algorithm.signature_len() {
        return Err(SignatureError::SignatureLengthMismatch);
    }
    if domain.key_domain != expected_domain {
        return Err(SignatureError::KeyDomainMismatch);
    }
    if domain.security_version < recorded_security_version {
        return Err(SignatureError::SecurityVersionDowngrade {
            recorded: recorded_security_version,
            offered: domain.security_version,
        });
    }
    // dev key 标记：key_id 全 0xDD = 开发密钥（构建期测试签名产物）。
    // release 下拒绝（allow_dev_key=false）。
    if !allow_dev_key && domain.key_id.iter().all(|b| *b == 0xDD) {
        return Err(SignatureError::DevKeyRejected);
    }
    // 负载 digest 必须非零（零 = 未绑定真实负载，后加载未签 payload 拒绝）。
    if domain.payload_digest.iter().all(|b| *b == 0) {
        return Err(SignatureError::PayloadDigestMismatch);
    }
    Ok(())
}

/// WP-37.7：签名提供者（boot manager 只调用此 trait；具体 Ed25519/SM2
/// 由 36 批准的 provider 实现）。
pub trait BootSignatureProvider {
    /// 校验 `domain` 绑定负载的 `signature`。域级检查（算法/长度/域/版本/
    /// dev key）由 [`check_signature_domain`] 先行，本方法执行密码学验证
    /// 与 payload digest 比对。
    ///
    /// `message` 为被签名原始字节（Ed25519 签消息而非摘要——verify_strict
    /// 语义；provider 内部须校验 `sha256(message) == domain.payload_digest`
    /// 防止摘要/消息错配——"替换 payload"负例的密码学侧）。
    fn verify(
        &self,
        domain: &SignatureDomain,
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), SignatureError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal_domain(version: u64) -> SignatureDomain {
        SignatureDomain {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id: [0x11; 32],
            security_version: version,
            key_domain: KeyDomain::Normal,
            payload_digest: [0xAB; 32],
        }
    }

    fn dev_domain(version: u64) -> SignatureDomain {
        SignatureDomain {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id: [0xDD; 32],
            security_version: version,
            key_domain: KeyDomain::Normal,
            payload_digest: [0xAB; 32],
        }
    }

    /// 正例：算法/长度/域/版本/digest 全匹配。
    #[test]
    fn check_domain_accepts_matching() {
        let d = normal_domain(7);
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 64]
            ),
            Ok(())
        );
    }

    /// 负例：算法混用 / 签名长度不符。
    #[test]
    fn check_domain_rejects_algorithm_and_length_mismatch() {
        let d = normal_domain(7);
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Sm2,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 64]
            ),
            Err(SignatureError::AlgorithmMismatch)
        );
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 63]
            ),
            Err(SignatureError::SignatureLengthMismatch)
        );
    }

    /// 负例：recovery 域 key 验 normal image。
    #[test]
    fn check_domain_rejects_key_domain_mismatch() {
        let mut d = normal_domain(7);
        d.key_domain = KeyDomain::Recovery;
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 64]
            ),
            Err(SignatureError::KeyDomainMismatch)
        );
    }

    /// 负例：security version 降级（rollback）。
    #[test]
    fn check_domain_rejects_downgrade() {
        let d = normal_domain(7);
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                8,
                false,
                &[0u8; 64]
            ),
            Err(SignatureError::SecurityVersionDowngrade {
                recorded: 8,
                offered: 7
            })
        );
    }

    /// 负例：dev key 编译进 release（allow_dev_key=false）拒绝；测试构建
    /// （allow_dev_key=true）放行。
    #[test]
    fn check_domain_rejects_dev_key_in_release() {
        let d = dev_domain(7);
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 64]
            ),
            Err(SignatureError::DevKeyRejected)
        );
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                true,
                &[0u8; 64]
            ),
            Ok(())
        );
    }

    /// 负例：零负载 digest（后加载未签 payload）拒绝。
    #[test]
    fn check_domain_rejects_zero_payload_digest() {
        let mut d = normal_domain(7);
        d.payload_digest = [0u8; 32];
        assert_eq!(
            check_signature_domain(
                &d,
                SignatureAlgorithm::Ed25519,
                KeyDomain::Normal,
                7,
                false,
                &[0u8; 64]
            ),
            Err(SignatureError::PayloadDigestMismatch)
        );
    }
}
