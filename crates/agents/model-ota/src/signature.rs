//! 模型签名验证（v0.111.0，D7：复用 eneros-crypto 国密实现，禁止重复造轮子）.
//!
//! 验证编排：算法门（D6）→ SM3 哈希比对 → SM2 验签。签名消息为模型字节的
//! SM3 哈希（32 字节），与蓝图 `sm2_verify(&signature, &hash, &ca_pubkey)`
//! 语义一致。

use eneros_crypto::{sm2_verify, sm3_hash, Sm2PublicKey, Sm2Signature};

use crate::ota_client::{ModelInfo, SigAlgorithm};
use crate::OtaError;

/// 验证模型签名（纯函数，D7）.
///
/// 1. `info.signature.algorithm != Sm2Sm3` → `Err(UnsupportedAlgorithm)`（D6）
/// 2. `sm3_hash(data) != info.hash` → `Err(HashMismatch)`
/// 3. 签名值长度 != 64 → `Err(SignatureInvalid)`，否则 `Sm2Signature::from_bytes` 解码
/// 4. `sm2_verify(&hash, &sig, pubkey)` → false 或内部错误 ⇒ `Err(SignatureInvalid)`，
///    true ⇒ `Ok(())`
pub fn verify_model_signature(
    data: &[u8],
    info: &ModelInfo,
    pubkey: &Sm2PublicKey,
) -> Result<(), OtaError> {
    if info.signature.algorithm != SigAlgorithm::Sm2Sm3 {
        return Err(OtaError::UnsupportedAlgorithm);
    }
    let hash = sm3_hash(data);
    if hash != info.hash {
        return Err(OtaError::HashMismatch);
    }
    let sig_bytes: &[u8; 64] = info
        .signature
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| OtaError::SignatureInvalid)?;
    let sig = Sm2Signature::from_bytes(sig_bytes);
    match sm2_verify(&hash, &sig, pubkey) {
        Ok(true) => Ok(()),
        Ok(false) => Err(OtaError::SignatureInvalid),
        Err(_) => Err(OtaError::SignatureInvalid),
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec::Vec;

    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;
    use crate::ota_client::ModelSignature;

    /// 构造真实 SM2 签名的清单.
    fn signed_info(data: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng) -> ModelInfo {
        let hash = sm3_hash(data);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).unwrap();
        ModelInfo {
            model_id: "m".to_string(),
            version: "2.0.0".to_string(),
            hash,
            size: data.len() as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: sig.to_bytes().to_vec(),
                timestamp: 42,
            },
            created_at: 24,
            capabilities: Vec::new(),
        }
    }

    /// SIG11 Sm2 真实签名往返（蓝图 §6.1/§7.3）.
    #[test]
    fn sig11_real_sm2_roundtrip() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let data = b"sig11-model-weights";
        let info = signed_info(data, &kp, &mut rng);
        assert_eq!(verify_model_signature(data, &info, &kp.public_key), Ok(()));
    }

    /// SIG12 篡改数据 1 字节 → Err(HashMismatch).
    #[test]
    fn sig12_tampered_data_hash_mismatch() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let data = b"sig12-model-weights";
        let info = signed_info(data, &kp, &mut rng);
        let mut tampered = data.to_vec();
        tampered[3] ^= 0x01;
        assert_eq!(
            verify_model_signature(&tampered, &info, &kp.public_key),
            Err(OtaError::HashMismatch)
        );
    }

    /// SIG13 RsaSha256 占位 → Err(UnsupportedAlgorithm)（D6）.
    #[test]
    fn sig13_rsa_sha256_unsupported() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let data = b"sig13-model-weights";
        let mut info = signed_info(data, &kp, &mut rng);
        info.signature.algorithm = SigAlgorithm::RsaSha256;
        assert_eq!(
            verify_model_signature(data, &info, &kp.public_key),
            Err(OtaError::UnsupportedAlgorithm)
        );
    }
}
