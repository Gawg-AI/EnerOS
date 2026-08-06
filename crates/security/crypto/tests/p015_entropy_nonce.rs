//! v0.209.0（P0-15）：SM2 两次签名 nonce 必须不同（熵源驱动）。

use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

#[test]
fn test_sm2_two_signatures_have_different_nonce() {
    let mut rng = CsRng::from_seed(&[1u8; 32]);
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let msg = b"nonce differentiation";

    // 两次独立签名（RNG 状态推进 → 不同 nonce → 不同 r 分量）。
    let mut rng1 = CsRng::from_seed(&[2u8; 32]);
    let sig1 = sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng1).unwrap();

    let mut rng2 = CsRng::from_seed(&[3u8; 32]);
    let sig2 = sm2_sign(msg, &kp.private_key, &kp.public_key, &mut rng2).unwrap();

    // 相同消息+密钥，但 RNG 不同 → nonce（r）必须不同。
    assert_ne!(
        sig1.r, sig2.r,
        "不同熵源驱动下两次签名 nonce（r 分量）必须不同（P0-15）"
    );
    // 两签名都能通过验签。
    assert!(eneros_crypto::sm2_verify(msg, &sig1, &kp.public_key).unwrap());
    assert!(eneros_crypto::sm2_verify(msg, &sig2, &kp.public_key).unwrap());
}
