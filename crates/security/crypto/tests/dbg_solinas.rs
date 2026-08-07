#![allow(clippy::disallowed_macros)]

#[test]
fn dbg_solinas_p_vs_bitwise() {
    // 随机 prod 对拍 reduce_512_sm2_p vs reduce_512_to_256
    let p = eneros_crypto::sm2::SM2_P;
    let mut seed: u64 = 0x1234;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    };
    for _ in 0..100 {
        let prod = [
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
            next(),
        ];
        let fast = eneros_crypto::bigint::reduce_512_sm2_p_for_test(&prod);
        let slow = eneros_crypto::bigint::reduce_512_slow_for_test(&prod, &p);
        if fast != slow {
            panic!(
                "p 归约不一致: fast={:?} slow={:?} prod={:?}",
                fast, slow, prod
            );
        }
    }
}
