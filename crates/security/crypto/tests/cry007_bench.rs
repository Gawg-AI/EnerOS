//! CRY-007 性能基准：SM2 验签耗时（host 实测，固定标量多轮取均值）。
//!
//! 目标：CRY-001 Jacobian 化后验签从旧仿射 150–214ms 显著下降；
//! 指标（S3 出口）：host dev 单次验签 < 20ms（旧实现 ≥150ms 的 7 倍余量）。

use std::time::Instant;

use eneros_crypto::{sm2_sign, sm2_verify, CsRng, Sm2KeyPair};

#[test]
fn test_cry007_verify_latency_metric() {
    let mut rng = CsRng::from_seed(&[0x51; 32]);
    let kp = Sm2KeyPair::generate(&mut rng).unwrap();
    let digest = [0xABu8; 32];
    let sig = sm2_sign(&digest, &kp.private_key, &kp.public_key, &mut rng).unwrap();
    let _ = sig;
    // 预热 + 多轮取均值（抗噪声）
    for _ in 0..5 {
        assert!(sm2_verify(&digest, &sig, &kp.public_key).unwrap());
    }
    let rounds = 20;
    let start = Instant::now();
    for _ in 0..rounds {
        assert!(sm2_verify(&digest, &sig, &kp.public_key).unwrap());
    }
    let avg_ms = start.elapsed().as_millis() as f64 / rounds as f64;
    eprintln!("SM2 verify avg = {:.2} ms ({} rounds)", avg_ms, rounds);
    assert!(
        avg_ms < 20.0,
        "SM2 验签性能指标未达成：avg={:.2}ms（指标 <20ms；旧仿射实现 150–214ms）",
        avg_ms
    );
}
