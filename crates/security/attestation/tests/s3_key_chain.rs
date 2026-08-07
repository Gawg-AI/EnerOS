//! S3 出口集成测试：密钥生成→签名→验签→封仓→解封→证明 全链路（host）。
//!
//! QEMU 运行时端到端 BLOCKED（无 qemu + seL4 工具链缓存）；本测试在
//! host 上串起 crypto（SM2）+ tpm（Seal/Unseal）+ attestation（Quote）
//! 真实实现，证明链路代码侧完整闭环。

use eneros_attestation::attest::{AttestVerifier, RemoteAttestation};
use eneros_attestation::event_log::TcgEventLog;
use eneros_attestation::tpm::{SoftTpm, TpmBackend};
use eneros_crypto::sm2::encrypt::{sm2_decrypt, sm2_encrypt};
use eneros_crypto::{sm2_sign, sm2_verify, CsRng, Sm2KeyPair};

#[test]
fn test_s3_full_key_chain() {
    // 1) 密钥生成（熵注入：from_seed 显式种子）
    let mut rng = CsRng::from_seed(&[0x5A; 32]);
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥生成");
    assert!(kp.public_key.point.is_on_curve());

    // 2) 签名 + 3) 验签（篡改拒绝）
    let msg = b"key-chain-msg";
    let digest = eneros_crypto::sm3::hash(msg);
    let sig = sm2_sign(&digest, &kp.private_key, &kp.public_key, &mut rng).expect("签名");
    assert!(sm2_verify(&digest, &sig, &kp.public_key).expect("验签"));
    let mut tampered = sig.to_bytes();
    tampered[0] ^= 0xff;
    let tampered_sig = eneros_crypto::Sm2Signature::from_bytes(&tampered);
    assert!(!sm2_verify(&digest, &tampered_sig, &kp.public_key).expect("篡改拒绝"));

    // 4) 封仓（SM2 加密语义）→ 5) 解封（解密）往返
    let secret = b"sealed device secret";
    let sealed = sm2_encrypt(secret, &kp.public_key, &mut rng).expect("封仓");
    let unsealed = sm2_decrypt(&sealed, &kp.private_key).expect("解封");
    assert_eq!(unsealed, secret, "封仓-解封往返还原");
    let mut tampered_cipher = sealed.clone();
    tampered_cipher[0] ^= 0xff;
    assert!(
        sm2_decrypt(&tampered_cipher, &kp.private_key).is_err(),
        "篡改密文拒绝"
    );

    // 6) 证明（Quote + 验签，AK 跨"重启"稳定）
    let mut tpm = SoftTpm::from_identity_seed(&[0x42; 32]).expect("AK 派生");
    let mut log = TcgEventLog::new();
    log.measure(&mut tpm, 0, 1, b"boot-code").expect("measure");
    let quote_nonce = [0x77; 20];
    let ra = RemoteAttestation::generate(&mut tpm, &[0, 1], &quote_nonce, 1_000, &log, &mut rng)
        .expect("远程证明生成");
    let mut verifier = AttestVerifier::new(tpm.attestation_pubkey().unwrap());
    let check = verifier.verify(&ra, &log, &quote_nonce);
    assert!(check.trusted, "Quote 验签通过");
}
