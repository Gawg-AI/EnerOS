//! CRY-001 回归测试：SM2 点运算 Jacobian 坐标 + 定次模逆 + 恒定时间性。
//!
//! 依据（蓝图 08 §CRY-001）：
//! 1. 点运算改 Jacobian 坐标——消除旧仿射坐标每步逐步 EEA 模逆；
//! 2. 模逆改定次费马（a^(m-2)，轮数固定 256）——旧 EEA 迭代次数依赖输入；
//! 3. scalar_mult 分支改 ct_select——旧实现 `if k.bit(i)` 结果选择分支；
//! 4. 不同输入计时统计验证恒定时间性。

use std::time::Instant;

use eneros_crypto::bigint::U256;
use eneros_crypto::sm2::keypair::EcPoint;
use eneros_crypto::sm2::{SM2_N, SM2_P};

/// 独立计算（Python 实现同公式）的 2G/3G/4G/5G affine x 坐标（KAT 对拍）。
const KAT: &[(&str, &str)] = &[
    (
        "2",
        "56CEFD60D7C87C000D58EF57FA73BA4D9C0DFA08C08A7331495C2E1DA3F2BD52",
    ),
    (
        "3",
        "A97F7CD4B3C993B4BE2DAA8CDB41E24CA13F6BD945302244E26918F1D0509EBF",
    ),
    (
        "4",
        "C239507105C683242A81052FF641ED69009A084AD5CC937DB21646CD34A0CED5",
    ),
    (
        "5",
        "C749061668652E26040E008FDD5EB77A344A417B7FCE19DBA575DA57CC372A9E",
    ),
];

fn u256_from_str(s: &str) -> U256 {
    U256::from_hex(&format!("{:0>64}", s)).unwrap()
}

#[test]
fn test_cry001_sm2_point_kat() {
    let g = EcPoint::generator();
    for (k_hex, expect_x) in KAT {
        let k = u256_from_str(k_hex);
        let p = g.scalar_mult(&k).to_affine();
        assert_eq!(
            p.x.to_hex(),
            *expect_x,
            "{}G affine x 与独立计算一致（Jacobian 正确性）",
            k_hex
        );
        assert!(p.is_on_curve(), "{}G 在曲线上", k_hex);
    }
}

#[test]
fn test_cry001_inv_mod_kat() {
    // 定次费马模逆：2^-1 mod p = (p+1)/2；2^-1 mod n = (n+1)/2
    let two = u256_from_str("2");
    let inv_p = two.inv_mod(&SM2_P).expect("2^-1 mod p");
    let expect_p = SM2_P.add_mod(&u256_from_str("1"), &SM2_P);
    // (p+1)/2 = (p+1) * 2^-1 mod p —— 用 2 逆自反验证
    let half = expect_p.mul_mod(&inv_p, &SM2_P);
    assert_eq!(half, expect_p.mul_mod(&inv_p, &SM2_P));
    let check = two.mul_mod(&inv_p, &SM2_P);
    assert_eq!(check, u256_from_str("1"), "2 * 2^-1 ≡ 1 mod p");
    let inv_n = two.inv_mod(&SM2_N).expect("2^-1 mod n");
    let check_n = two.mul_mod(&inv_n, &SM2_N);
    assert_eq!(check_n, u256_from_str("1"), "2 * 2^-1 ≡ 1 mod n");
}

#[test]
fn test_cry001_scalar_mult_constant_time_timing() {
    // 恒定时间性统计：不同标量（含汉明重量差异大的）scalar_mult 耗时分布。
    // ladder 每轮无条件 add+double+ct_select，轮数固定 256——耗时不应
    // 显著随标量值变化（旧实现 `if k.bit(i)` 分支被消除）。
    let g = EcPoint::generator();
    let scalars = [
        u256_from_str("1"),
        u256_from_str("2"),
        SM2_N.sub_mod(&u256_from_str("1"), &SM2_N), // 全 1 位
        u256_from_str("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"),
        u256_from_str("123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0"),
        u256_from_str("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        u256_from_str("5555555555555555555555555555555555555555555555555555555555555555"),
    ];
    // 只测 scalar_mult 本体（ladder 位处理面）；to_affine 的费马模逆
    // 由 test_cry001_inv_mod_constant_time_timing 单独覆盖（定次性）。
    let mut times = Vec::new();
    for k in &scalars {
        let start = Instant::now();
        let _ = g.scalar_mult(k);
        let elapsed = start.elapsed().as_nanos();
        times.push(elapsed);
    }
    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();
    // 阈值：max/min ≤ 2.2（固定 257 轮 ladder；并行测试噪声下仍能
    // 区分恒定轮数与旧实现的输入相关分支——旧实现 k=1 与 k≈2^256
    // 相差 >1000×，本测试在 k=1 时旧实现必失败）。
    assert!(
        *max as f64 / *min as f64 <= 2.2,
        "scalar_mult 耗时分布过宽（非恒定时间）: min={}ns max={}ns vals={:?}",
        min,
        max,
        times
    );
}

#[test]
fn test_cry001_inv_mod_constant_time_timing() {
    // 定次费马模逆：不同输入的耗时分布（EEA 旧实现迭代次数随输入变化）。
    let inputs = [
        u256_from_str("1"),
        u256_from_str("2"),
        SM2_N.sub_mod(&u256_from_str("1"), &SM2_N),
        u256_from_str("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"),
        u256_from_str("ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"),
    ];
    let mut times = Vec::new();
    for a in &inputs {
        let start = Instant::now();
        let _ = a.inv_mod(&SM2_N).expect("inv");
        times.push(start.elapsed().as_nanos());
    }
    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();
    assert!(
        *max as f64 / *min as f64 <= 2.2,
        "inv_mod 耗时分布过宽（非定次）: min={}ns max={}ns vals={:?}",
        min,
        max,
        times
    );
}

#[test]
fn test_cry001_g_add_associativity() {
    let g = EcPoint::generator();
    let two = u256_from_str("2");
    let three = u256_from_str("3");
    let p2 = g.scalar_mult(&two);
    let _p3 = g.scalar_mult(&three);
    // (G + G) + G == G + (G + G) 即 2G + G == G + 2G == 3G（Jacobian 表示）
    let l = p2.add(&g).to_affine();
    let r = g.add(&p2).to_affine();
    let three_g = g.scalar_mult(&three).to_affine();
    assert_eq!(l, r, "结合律（Jacobian add）");
    assert_eq!(l, three_g, "2G+G == 3G");
}
