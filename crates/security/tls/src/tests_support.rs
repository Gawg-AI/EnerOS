//! 测试身份构造支持（`embedded` + `std` 测试面共享——embedded.rs 与
//! target_factory.rs 的握手负例/正例同源；生产零依赖）。

#![cfg(all(test, feature = "embedded", feature = "std"))]

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::pki::parser::to_der;
use eneros_crypto::pki::x509::{
    DistinguishedName, SignatureAlgorithm, SubjectPublicKey, X509Certificate,
};
use p256::ecdsa::signature::Signer;

pub(crate) const NOW: u64 = 1_700_000_000;

/// 测试身份：本仓生成的 ECDSA P-256 自签证书 + SEC1 私钥。
pub(crate) struct TestIdentity {
    pub(crate) cert_der: Vec<u8>,
    pub(crate) key_der: Vec<u8>,
    pub(crate) x509: X509Certificate,
    pub(crate) cn: String,
}

pub(crate) fn make_identity(cn: &str, scalar: u8) -> TestIdentity {
    let sk: p256::ecdsa::SigningKey =
        p256::ecdsa::SigningKey::from_bytes(&[scalar; 32].into()).unwrap();
    let pk = SubjectPublicKey::EcdsaP256(
        sk.verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec(),
    );
    let dn = DistinguishedName::new(cn);
    let unsigned = X509Certificate::new(
        2,
        vec![1],
        SignatureAlgorithm::EcdsaWithSha256,
        dn.clone(),
        dn.clone(),
        NOW,
        NOW + 86400 * 365,
        pk,
        Vec::new(),
        Vec::new(),
    );
    let tbs = unsigned.encode_tbs().unwrap();
    let sig: p256::ecdsa::Signature = sk.sign(&tbs);
    let cert = X509Certificate::new(
        2,
        vec![1],
        SignatureAlgorithm::EcdsaWithSha256,
        dn.clone(),
        dn.clone(),
        NOW,
        NOW + 86400 * 365,
        SubjectPublicKey::EcdsaP256(
            sk.verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        ),
        Vec::new(),
        sig.to_der().as_bytes().to_vec(),
    );
    let der = to_der(&cert).unwrap();
    // SEC1 DER（RFC 5915）：SEQUENCE { INTEGER(1), OCTET STRING(scalar),
    //   [0] prime256v1 OID, [1] BIT STRING 公钥 }——rustls 私钥解析器
    //   要求曲线参数 OID。
    let point = sk
        .verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec();
    let oid = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07]; // prime256v1
    let mut inner = vec![0x02, 0x01, 0x01, 0x04, 0x20];
    inner.extend_from_slice(&[scalar; 32]);
    inner.extend_from_slice(&[0xA0, 0x0A]);
    inner.extend_from_slice(&oid);
    inner.extend_from_slice(&[0xA1, 0x44, 0x03, 0x42, 0x00]);
    inner.extend_from_slice(&point);
    let mut key_der = vec![0x30, inner.len() as u8];
    key_der.extend(inner);
    TestIdentity {
        cert_der: der,
        key_der,
        x509: cert,
        cn: cn.to_string(),
    }
}
