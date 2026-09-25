//! installer-uefi host 可测核心层（WP-74.3.1，蓝图 74 卷三 R15-C）。
//!
//! UEFI 无关面：安装 manifest 验签（唯一入口 =
//! [`eneros_boot_protocol::signature::BootSignatureProvider`]——installer
//! 不再自带任何密钥/算法路径）与失败码词表。bin（`main.rs`）持 UEFI IO
//! 管道（`ScopedProtocol<BlockIO>` adapter、目标选择、写盘与回读），
//! 本 crate 纯逻辑供 host 测试（CI `-p eneros-installer-uefi`）。
//!
//! 词形冻结：[`InstallFailure`] 的 `code()` 集即
//! `eneros.install-rejection.v1` JSON 的 code 集，逐字冻结，不新增拒绝码。

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use eneros_boot_protocol::signature::{
    check_signature_domain, BootSignatureProvider, Ed25519SignatureProvider, KeyDomain,
    SignatureAlgorithm, SignatureDomain,
};
use eneros_boot_protocol::{InstallManifest, INSTALL_MANIFEST_SIGNED_SIZE};
use sha2::{Digest, Sha256};

/// 安装失败码（`eneros.install-rejection.v1` code 集——词形逐字冻结）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallFailure {
    SourceNotFound,
    SourceOpen,
    SourceRead,
    ManifestFormat,
    Platform,
    KeyId,
    Rollback,
    Signature,
    PayloadDigest,
    TargetMissing,
    TargetAmbiguous,
    TargetGeometry,
    TargetNotBlank,
    TargetWrite,
    TargetReadback,
}

impl InstallFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SourceNotFound => "SOURCE_NOT_FOUND",
            Self::SourceOpen => "SOURCE_OPEN",
            Self::SourceRead => "SOURCE_READ",
            Self::ManifestFormat => "INSTALL_MANIFEST",
            Self::Platform => "PLATFORM",
            Self::KeyId => "KEY_ID",
            Self::Rollback => "ROLLBACK",
            Self::Signature => "SIGNATURE",
            Self::PayloadDigest => "PAYLOAD_DIGEST",
            Self::TargetMissing => "TARGET_MISSING",
            Self::TargetAmbiguous => "TARGET_AMBIGUOUS",
            Self::TargetGeometry => "TARGET_GEOMETRY",
            Self::TargetNotBlank => "TARGET_NOT_BLANK",
            Self::TargetWrite => "TARGET_WRITE",
            Self::TargetReadback => "TARGET_READBACK",
        }
    }
}

/// 安装信任锚（bin 侧由 build.rs 生成的 `installer_trust.rs` 常量构造；
/// host 测试现配。build.rs 信任生成链不动——WP-74.3.1 边界）.
#[derive(Debug, Clone, Copy)]
pub struct TrustAnchor {
    /// 信任公钥（32 字节）.
    pub public_key: [u8; 32],
    /// 信任 key ID（16 字节）.
    pub key_id: [u8; 16],
    /// 平台 ID（16 字节）.
    pub platform_id: [u8; 16],
    /// 最低安全版本（下限，低于即 Rollback）.
    pub minimum_security_version: u64,
}

/// 安装 manifest 验签（WP-74.3.1 重写：验签只经 `BootSignatureProvider`
/// 唯一入口；函数名与 [`InstallFailure`] 映射保持不变）.
///
/// 前置检查原序保留：`platform_id`/`key_id`/`security_version < min` →
/// `Platform`/`KeyId`/`Rollback`（词形与拒绝码不变）；随后签名域门
/// （算法/签名长度/密钥域/版本下限/dev-key 拒绝——新拒绝面）+ provider
/// 密码学验证，错误一律映射 `Signature`（code 集冻结，不新增拒绝码）。
///
/// domain 语义登记：installer 的被签消息是 manifest 被签区自身
/// （`encoded[..INSTALL_MANIFEST_SIGNED_SIZE]`），`payload_digest` 字段
/// 约束 PAYLOAD.RAW（不在被签消息内）——故 domain 取被签区现算摘要。
/// 对 installer 而言 provider 的摘要一致性检查是平凡真（domain 从同一段
/// 字节现算）；实质增益 = dev-key 拒绝面 + 算法/长度/域统一门 + 单点
/// 密码学路径 + 「唯一入口」宣称成真（蓝图 74 卷三 §1）.
pub fn verify_install_manifest(
    encoded: &[u8],
    anchor: &TrustAnchor,
) -> Result<InstallManifest, InstallFailure> {
    let manifest = InstallManifest::decode(encoded).map_err(|_| InstallFailure::ManifestFormat)?;
    if manifest.platform_id != anchor.platform_id {
        return Err(InstallFailure::Platform);
    }
    if manifest.key_id != anchor.key_id {
        return Err(InstallFailure::KeyId);
    }
    if manifest.security_version < anchor.minimum_security_version {
        // 与 check_signature_domain 的 `< recorded → Downgrade` 同向；
        // 拒绝码保持 Rollback（版本语义同向，蓝图 74 卷三 §1）.
        return Err(InstallFailure::Rollback);
    }
    let payload_digest: [u8; 32] = Sha256::digest(&encoded[..INSTALL_MANIFEST_SIGNED_SIZE]).into();
    // 16→32 位身份提升：manifest.key_id（16B）进 SignatureDomain（32B）。
    // 高半区以 0xDD 填充——谓词保持（x 全 0xDD ⇔ x‖0xDD 全 0xDD），使
    // check_signature_domain 的 dev-key 门对 16B 全 0xDD 的构建期测试
    // 签名产物真实可达（零填充会静默绕过该门——红测实锤）；key_id 在
    // 域门中仅消费 dev-key 谓词，填充不引入其他语义。
    let mut key_id_32 = [0xDDu8; 32];
    key_id_32[..16].copy_from_slice(&manifest.key_id);
    let domain = SignatureDomain {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: key_id_32,
        security_version: manifest.security_version,
        key_domain: KeyDomain::Normal,
        payload_digest,
    };
    check_signature_domain(
        &domain,
        SignatureAlgorithm::Ed25519,
        KeyDomain::Normal,
        anchor.minimum_security_version,
        false, // 安装链等同 release：dev/test key（key_id 全 0xDD）拒绝
        &manifest.signature,
    )
    .map_err(|_| InstallFailure::Signature)?;
    let provider = Ed25519SignatureProvider {
        public_key: anchor.public_key,
    };
    provider
        .verify(
            &domain,
            &encoded[..INSTALL_MANIFEST_SIGNED_SIZE],
            &manifest.signature,
        )
        .map_err(|_| InstallFailure::Signature)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use eneros_boot_protocol::INSTALL_MANIFEST_SIZE;

    use super::*;

    /// 测试用 payload 长度：≥16 MiB 下限且 512 对齐（encode 校验域）.
    const PAYLOAD_LENGTH: u64 = 128 * 1024 * 1024;

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn anchor_for(key: &SigningKey, key_id: [u8; 16]) -> TrustAnchor {
        TrustAnchor {
            public_key: key.verifying_key().to_bytes(),
            key_id,
            platform_id: [0x41; 16],
            minimum_security_version: 7,
        }
    }

    fn signed_manifest(
        key: &SigningKey,
        key_id: [u8; 16],
        security_version: u64,
    ) -> [u8; INSTALL_MANIFEST_SIZE] {
        let manifest = InstallManifest::new(
            [0x41; 16],
            key_id,
            security_version,
            1_785_465_257,
            PAYLOAD_LENGTH,
            512,
            [3; 16],
            [4; 16],
            [5; 32],
        );
        let unsigned = manifest.encode_unsigned().unwrap();
        let signature = key.sign(&unsigned).to_bytes();
        manifest.with_signature(signature).encode_signed().unwrap()
    }

    /// 正例：信任锚现签 manifest 全过（解码 + 前置检查 + 域门 + provider）.
    #[test]
    fn signed_install_manifest_is_accepted() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);
        let manifest = verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])).unwrap();
        assert_eq!(manifest.payload_length, PAYLOAD_LENGTH);
        assert_eq!(manifest.security_version, 7);
    }

    /// 负例：签名区字节翻转 → 域门通过（长度/域/版本合法）但 provider
    /// 密码学验证拒绝 → `Signature`（不新增拒绝码）.
    #[test]
    fn flipped_signature_byte_is_rejected() {
        let key = signing_key(0x37);
        let mut encoded = signed_manifest(&key, [2; 16], 7);
        encoded[INSTALL_MANIFEST_SIGNED_SIZE] ^= 1;
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：异钥现签（key_id 冒用锚值）→ decode/前置检查全过，密码学
    /// 验证拒绝 → `Signature`.
    #[test]
    fn foreign_key_signature_is_rejected() {
        let trusted = signing_key(0x37);
        let attacker = signing_key(0x99);
        let encoded = signed_manifest(&attacker, [2; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&trusted, [2; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：security_version 低于锚下限 → `Rollback`（前置检查原序保留，
    /// 拒绝码词形不变）.
    #[test]
    fn security_version_below_minimum_maps_to_rollback() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 6);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [2; 16])),
            Err(InstallFailure::Rollback)
        );
    }

    /// 负例：platform/key_id 不符 → 原码 `Platform`/`KeyId`.
    #[test]
    fn platform_and_key_id_mismatches_keep_original_codes() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);

        let mut anchor = anchor_for(&key, [2; 16]);
        anchor.platform_id = [0x42; 16];
        assert_eq!(
            verify_install_manifest(&encoded, &anchor),
            Err(InstallFailure::Platform)
        );

        let mut anchor = anchor_for(&key, [2; 16]);
        anchor.key_id = [3; 16];
        assert_eq!(
            verify_install_manifest(&encoded, &anchor),
            Err(InstallFailure::KeyId)
        );
    }

    /// **dev-key 新拒绝面**：key_id 全 0xDD（构建期测试签名产物）现签，
    /// 锚亦持同 key_id（前置 key_id 相等才走到域门）→
    /// `check_signature_domain` DevKeyRejected → `Signature`。修复前该
    /// manifest 经裸 verify_strict 直接放行（R15-C 指控面）.
    #[test]
    fn dev_key_is_rejected_through_provider_domain_gate() {
        let key = signing_key(0xDD);
        let encoded = signed_manifest(&key, [0xDD; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded, &anchor_for(&key, [0xDD; 16])),
            Err(InstallFailure::Signature)
        );
    }

    /// 负例：长度不符（截断/超长）→ `ManifestFormat`（decode 长度域）.
    #[test]
    fn wrong_length_encoded_is_manifest_format() {
        let key = signing_key(0x37);
        let encoded = signed_manifest(&key, [2; 16], 7);
        assert_eq!(
            verify_install_manifest(&encoded[..200], &anchor_for(&key, [2; 16])),
            Err(InstallFailure::ManifestFormat)
        );
        assert_eq!(
            verify_install_manifest(&encoded[..255], &anchor_for(&key, [2; 16])),
            Err(InstallFailure::ManifestFormat)
        );
    }

    /// 词形冻结锁：`eneros.install-rejection.v1` code 集逐字冻结
    ///（15 词形，不新增不 renaming）.
    #[test]
    fn install_rejection_code_set_is_frozen() {
        let expected: [(&'static str, &'static str); 15] = [
            ("SourceNotFound", "SOURCE_NOT_FOUND"),
            ("SourceOpen", "SOURCE_OPEN"),
            ("SourceRead", "SOURCE_READ"),
            ("ManifestFormat", "INSTALL_MANIFEST"),
            ("Platform", "PLATFORM"),
            ("KeyId", "KEY_ID"),
            ("Rollback", "ROLLBACK"),
            ("Signature", "SIGNATURE"),
            ("PayloadDigest", "PAYLOAD_DIGEST"),
            ("TargetMissing", "TARGET_MISSING"),
            ("TargetAmbiguous", "TARGET_AMBIGUOUS"),
            ("TargetGeometry", "TARGET_GEOMETRY"),
            ("TargetNotBlank", "TARGET_NOT_BLANK"),
            ("TargetWrite", "TARGET_WRITE"),
            ("TargetReadback", "TARGET_READBACK"),
        ];
        for (variant, code) in expected {
            assert_eq!(failure_code_by_variant_name(variant), code);
        }
    }

    /// 按变体名取 code（仅供冻结锁使用，避免为测试暴露反射面）.
    fn failure_code_by_variant_name(name: &str) -> &'static str {
        match name {
            "SourceNotFound" => InstallFailure::SourceNotFound.code(),
            "SourceOpen" => InstallFailure::SourceOpen.code(),
            "SourceRead" => InstallFailure::SourceRead.code(),
            "ManifestFormat" => InstallFailure::ManifestFormat.code(),
            "Platform" => InstallFailure::Platform.code(),
            "KeyId" => InstallFailure::KeyId.code(),
            "Rollback" => InstallFailure::Rollback.code(),
            "Signature" => InstallFailure::Signature.code(),
            "PayloadDigest" => InstallFailure::PayloadDigest.code(),
            "TargetMissing" => InstallFailure::TargetMissing.code(),
            "TargetAmbiguous" => InstallFailure::TargetAmbiguous.code(),
            "TargetGeometry" => InstallFailure::TargetGeometry.code(),
            "TargetNotBlank" => InstallFailure::TargetNotBlank.code(),
            "TargetWrite" => InstallFailure::TargetWrite.code(),
            "TargetReadback" => InstallFailure::TargetReadback.code(),
            _ => unreachable!(),
        }
    }
}
