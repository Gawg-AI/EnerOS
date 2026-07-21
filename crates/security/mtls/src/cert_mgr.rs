//! 证书管理：链式验签 + 有效期 + CRL 吊销检查（蓝图 §4.3）.
//!
//! [`CertManager::verify_cert`] 验证顺序固定：
//! 1. **链式验签**：在信任根中按颁发者 DN 查找颁发者证书，复用
//!    eneros-crypto `verify_signature` 验证证书签名
//! 2. **有效期**：`now` 越界 → [`CertError::NotYetValid`] / [`CertError::Expired`]
//! 3. **CRL 吊销**：序列号命中已加载 CRL → [`CertError::Revoked`]
//!
//! 错误显式传播，不吞错。CRL 复用 eneros-crypto `Crl` 结构，不自研解析。
//!
//! # no_std 合规
//! 仅使用 `alloc::vec::Vec` / `core::*`，不依赖 `std::*`。

use alloc::vec::Vec;

use eneros_crypto::{verify_signature, Crl, X509Certificate};

use crate::CertError;

/// 证书管理器（信任根 + 可选 CRL）.
///
/// 与 eneros-crypto `CertVerifier` 的差异：本类型服务于 mTLS 握手路径，
/// 验证顺序固定为「验签 → 有效期 → 吊销」（蓝图 §4.3 规定顺序），且错误
/// 模型收敛为 Copy 的 [`CertError`]（`CertVerifier` 顺序为有效期 → 吊销 →
/// 验签，错误为带 payload 的 `PkiError`）。
pub struct CertManager {
    /// 信任的根证书列表.
    trusted_roots: Vec<X509Certificate>,
    /// 可选的证书吊销列表（`load_crl` 加载后生效）.
    crl: Option<Crl>,
}

impl CertManager {
    /// 创建证书管理器（至少一个信任根）.
    pub fn new(trusted_roots: Vec<X509Certificate>) -> Self {
        Self {
            trusted_roots,
            crl: None,
        }
    }

    /// 加载证书吊销列表（覆盖旧表；复用 eneros-crypto `Crl`，不自研解析）.
    pub fn load_crl(&mut self, crl: Crl) {
        self.crl = Some(crl);
    }

    /// 吊销检查：证书序列号命中已加载 CRL → [`CertError::Revoked`].
    ///
    /// 未加载 CRL 时视为「无吊销信息」，返回 `Ok(())`（蓝图 §4.3：
    /// CRL 为可选加固项）。
    pub fn check_revocation(&self, cert: &X509Certificate) -> Result<(), CertError> {
        if let Some(ref crl) = self.crl {
            if crl.is_revoked(&cert.serial_number) {
                return Err(CertError::Revoked);
            }
        }
        Ok(())
    }

    /// 验证证书（顺序固定：链式验签 → 有效期 → CRL 吊销）.
    ///
    /// # 参数
    /// - `cert`：待验证的对端证书
    /// - `now`：当前 Unix 时间戳（秒，no_std 无系统时钟，外部注入）
    ///
    /// # 返回
    /// - `Ok(())`：证书有效
    /// - `Err(CertError::ChainBroken)`：信任根中找不到颁发者
    /// - `Err(CertError::SignatureInvalid)`：颁发者公钥验签失败
    /// - `Err(CertError::NotYetValid)` / [`CertError::Expired`]：有效期越界
    /// - `Err(CertError::Revoked)`：序列号命中 CRL
    pub fn verify_cert(&self, cert: &X509Certificate, now: u64) -> Result<(), CertError> {
        // 1. 链式验签：按颁发者 DN 在信任根中查找颁发者证书
        let issuer = self
            .trusted_roots
            .iter()
            .find(|root| root.subject == cert.issuer)
            .ok_or(CertError::ChainBroken)?;
        verify_signature(cert, issuer).map_err(|_| CertError::SignatureInvalid)?;

        // 2. 有效期检查
        if now < cert.not_before {
            return Err(CertError::NotYetValid);
        }
        if now > cert.not_after {
            return Err(CertError::Expired);
        }

        // 3. CRL 吊销检查
        self.check_revocation(cert)
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{
        build_certificate, build_self_signed, CertRequest, Crl, CsRng, DistinguishedName,
        RevocationReason, RevokedCert, Sm2KeyPair, SubjectPublicKey,
    };

    use super::*;

    const NOW: u64 = 1_700_000_000;
    const DAY: u64 = 86_400;

    /// 构建自签名根 CA（有效期 365 天）.
    fn make_ca(rng: &mut CsRng, now: u64) -> (Sm2KeyPair, X509Certificate) {
        let kp = Sm2KeyPair::generate(rng).expect("CA 密钥对生成");
        let subject = DistinguishedName::new("EnerOS Test Root CA")
            .with_o("EnerOS")
            .with_c("CN");
        let req = CertRequest::new(subject, SubjectPublicKey::Sm2(kp.public_key));
        let cert = build_self_signed(&req, &kp.private_key, &kp.public_key, now, rng)
            .expect("CA 自签名证书");
        (kp, cert)
    }

    /// 构建 CA 签发的叶子证书（validity_days 有效期）.
    fn make_leaf(
        rng: &mut CsRng,
        ca_kp: &Sm2KeyPair,
        ca_cert: &X509Certificate,
        cn: &str,
        serial: &[u8],
        validity_days: u32,
        now: u64,
    ) -> (Sm2KeyPair, X509Certificate) {
        let kp = Sm2KeyPair::generate(rng).expect("叶子密钥对生成");
        let subject = DistinguishedName::new(cn).with_o("EnerOS").with_c("CN");
        let req = CertRequest::new(subject, SubjectPublicKey::Sm2(kp.public_key))
            .with_validity_days(validity_days);
        let cert = build_certificate(
            &req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            serial,
            now,
            rng,
        )
        .expect("叶子证书签发");
        (kp, cert)
    }

    /// CERT4：有效证书通过验证（验签 ✓ → 有效期 ✓ → 无 CRL ✓）.
    #[test]
    fn cert4_valid_cert_passes() {
        let mut rng = CsRng::new();
        let (ca_kp, ca_cert) = make_ca(&mut rng, NOW);
        let (_leaf_kp, leaf) = make_leaf(&mut rng, &ca_kp, &ca_cert, "edge-node-01", &[2], 30, NOW);
        let mgr = CertManager::new(alloc::vec![ca_cert]);
        assert_eq!(
            mgr.verify_cert(&leaf, NOW + DAY),
            Ok(()),
            "有效证书应通过验证"
        );
    }

    /// CERT5：过期证书 → CertError::Expired.
    #[test]
    fn cert5_expired_rejected() {
        let mut rng = CsRng::new();
        let (ca_kp, ca_cert) = make_ca(&mut rng, NOW);
        let (_leaf_kp, leaf) = make_leaf(&mut rng, &ca_kp, &ca_cert, "edge-node-02", &[3], 1, NOW);
        let mgr = CertManager::new(alloc::vec![ca_cert]);
        assert_eq!(
            mgr.verify_cert(&leaf, NOW + 2 * DAY),
            Err(CertError::Expired),
            "过期证书应返回 Expired"
        );
    }

    /// CERT6：未生效证书 → CertError::NotYetValid.
    #[test]
    fn cert6_not_yet_valid_rejected() {
        let mut rng = CsRng::new();
        let (ca_kp, ca_cert) = make_ca(&mut rng, NOW);
        let (_leaf_kp, leaf) = make_leaf(&mut rng, &ca_kp, &ca_cert, "edge-node-03", &[4], 30, NOW);
        let mgr = CertManager::new(alloc::vec![ca_cert]);
        assert_eq!(
            mgr.verify_cert(&leaf, NOW - 1),
            Err(CertError::NotYetValid),
            "未生效证书应返回 NotYetValid"
        );
    }

    /// CERT7：吊销证书 → CertError::Revoked（签名/有效期均正常，CRL 命中）.
    #[test]
    fn cert7_revoked_rejected() {
        let mut rng = CsRng::new();
        let (ca_kp, ca_cert) = make_ca(&mut rng, NOW);
        let (_leaf_kp, leaf) = make_leaf(&mut rng, &ca_kp, &ca_cert, "edge-node-04", &[5], 30, NOW);
        let mut mgr = CertManager::new(alloc::vec![ca_cert.clone()]);
        // 加载含该序列号的 CRL（复用 eneros-crypto Crl 结构）
        let mut crl = Crl::new(ca_cert.subject.clone(), NOW + 30 * DAY);
        crl.add_revoked(RevokedCert::new(
            &leaf.serial_number,
            NOW,
            RevocationReason::KeyCompromise,
        ));
        mgr.load_crl(crl);
        assert_eq!(
            mgr.verify_cert(&leaf, NOW + DAY),
            Err(CertError::Revoked),
            "CRL 命中证书应返回 Revoked"
        );
        assert_eq!(
            mgr.check_revocation(&leaf),
            Err(CertError::Revoked),
            "check_revocation 应独立命中"
        );
    }

    /// CERT8：坏签名 → CertError::SignatureInvalid（验签最先执行，先于有效期）.
    #[test]
    fn cert8_bad_signature_rejected() {
        let mut rng = CsRng::new();
        let (ca_kp, ca_cert) = make_ca(&mut rng, NOW);
        let (_leaf_kp, mut leaf) =
            make_leaf(&mut rng, &ca_kp, &ca_cert, "edge-node-05", &[6], 30, NOW);
        // 篡改签名字节（验签必失败）
        leaf.signature[10] ^= 0xFF;
        let mgr = CertManager::new(alloc::vec![ca_cert.clone()]);
        assert_eq!(
            mgr.verify_cert(&leaf, NOW + DAY),
            Err(CertError::SignatureInvalid),
            "坏签名应返回 SignatureInvalid"
        );

        // 链断裂：颁发者不在信任根中 → ChainBroken
        // （他 CA 必须使用不同 DN：DN 相同会被 issuer 查找命中而退化为 SignatureInvalid）
        let other_ca_kp = Sm2KeyPair::generate(&mut rng).expect("他 CA 密钥对生成");
        let other_req = CertRequest::new(
            DistinguishedName::new("EnerOS Other Root CA")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(other_ca_kp.public_key),
        );
        let other_ca_cert = build_self_signed(
            &other_req,
            &other_ca_kp.private_key,
            &other_ca_kp.public_key,
            NOW,
            &mut rng,
        )
        .expect("他 CA 自签名证书");
        let (_l_kp, other_leaf) = make_leaf(
            &mut rng,
            &other_ca_kp,
            &other_ca_cert,
            "other",
            &[8],
            30,
            NOW,
        );
        let mgr2 = CertManager::new(alloc::vec![ca_cert]);
        assert_eq!(
            mgr2.verify_cert(&other_leaf, NOW + DAY),
            Err(CertError::ChainBroken),
            "颁发者不在信任根中应返回 ChainBroken"
        );
    }
}
