//! SM2 椭圆曲线点运算与密钥对类型.
//!
//! 提供：
//! - [`EcPoint`]：仿射坐标椭圆曲线点，支持点加、点倍、标量乘法（Montgomery ladder）
//! - [`Sm2PrivateKey`] / [`Sm2PublicKey`] / [`Sm2KeyPair`]：密钥对类型
//!
//! # 安全说明
//! - 私钥派生 [`Drop`] trait，析构时自动清零
//! - 标量乘法使用 Montgomery ladder（恒定时间，抗简单功耗分析）
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use super::{SM2_A, SM2_B, SM2_GX, SM2_GY, SM2_N, SM2_P};
use crate::bigint::U256;
use crate::error::CryptoError;
use crate::rng::fill_bytes_or_reseed;

// ============================================================
// EcPoint: 椭圆曲线点（Jacobian 投影坐标，CRY-001）
// ============================================================

/// 椭圆曲线点（Jacobian 投影坐标）.
///
/// 仿射点 (x, y) 表示为 (X = x, Y = y, Z = 1)；Jacobian 坐标满足
/// x = X/Z², y = Y/Z³。CRY-001：点加/点倍全程无模逆（消除旧仿射坐标
/// 每步一次逐步 EEA 求逆），仅在 `to_affine` 末尾做一次定次费马模逆。
///
/// `is_infinity = true` 表示无穷远点（曲线单位元，Z = 0）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcPoint {
    /// Jacobian X 坐标.
    pub x: U256,
    /// Jacobian Y 坐标.
    pub y: U256,
    /// Jacobian Z 坐标（Z = 1 即仿射点；Z = 0 即无穷远点）.
    pub z: U256,
    pub is_infinity: bool,
}

impl EcPoint {
    /// 无穷远点（曲线单位元）.
    pub const INFINITY: EcPoint = EcPoint {
        x: U256 { limbs: [0; 4] },
        y: U256 { limbs: [0; 4] },
        z: U256 { limbs: [0; 4] },
        is_infinity: true,
    };

    /// SM2 基点 G.
    pub fn generator() -> Self {
        EcPoint {
            x: SM2_GX,
            y: SM2_GY,
            z: crate::bigint::ONE,
            is_infinity: false,
        }
    }

    /// 验证点是否在 SM2 曲线上: y^2 ≡ x^3 + ax + b (mod p).
    ///
    /// CRY-001：接受 Jacobian 点——先转仿射（Z=1 快速路径免模逆）再检查。
    pub fn is_on_curve(&self) -> bool {
        if self.is_infinity {
            return true;
        }
        let aff = if self.z == crate::bigint::ONE {
            *self
        } else {
            self.to_affine()
        };
        if aff.is_infinity {
            return false;
        }
        // y^2 mod p
        let y_sq = aff.y.mul_mod(&aff.y, &SM2_P);
        // x^3 mod p = x * x^2
        let x_sq = aff.x.mul_mod(&aff.x, &SM2_P);
        let x_cu = x_sq.mul_mod(&aff.x, &SM2_P);
        // a*x mod p
        let ax = SM2_A.mul_mod(&aff.x, &SM2_P);
        // x^3 + ax + b mod p
        let rhs = x_cu.add_mod(&ax, &SM2_P).add_mod(&SM2_B, &SM2_P);
        y_sq == rhs
    }

    /// 点加：self + other（Jacobian 坐标，CRY-001，无模逆）。
    ///
    /// 一般 Jacobian 加法公式（a = p-3 的 SM2 曲线；此处用通用公式，
    /// 输入可为任意 Jacobian 点）：
    ///   U1 = X1·Z2², U2 = X2·Z1²
    ///   S1 = Y1·Z2³, S2 = Y2·Z1³
    ///   H = U2-U1, R = S2-S1
    ///   H=0 且 R=0 → 加倍；H=0 → 无穷远
    ///   X3 = R²-H³-2U1H², Y3 = R(U1H²-X3)-S1H³, Z3 = Z1·Z2·H
    pub fn add(&self, other: &EcPoint) -> EcPoint {
        if self.is_infinity {
            return *other;
        }
        if other.is_infinity {
            return *self;
        }
        let z1 = self.z;
        let z2 = other.z;
        let z1_sq = z1.mul_mod(&z1, &SM2_P);
        let z2_sq = z2.mul_mod(&z2, &SM2_P);
        let z1_cu = z1_sq.mul_mod(&z1, &SM2_P);
        let z2_cu = z2_sq.mul_mod(&z2, &SM2_P);
        let u1 = self.x.mul_mod(&z2_sq, &SM2_P);
        let u2 = other.x.mul_mod(&z1_sq, &SM2_P);
        let s1 = self.y.mul_mod(&z2_cu, &SM2_P);
        let s2 = other.y.mul_mod(&z1_cu, &SM2_P);
        let h = u2.sub_mod(&u1, &SM2_P);
        let r = s2.sub_mod(&s1, &SM2_P);
        if h.is_zero() {
            if r.is_zero() {
                return self.double();
            }
            return EcPoint::INFINITY;
        }
        let h_sq = h.mul_mod(&h, &SM2_P);
        let h_cu = h_sq.mul_mod(&h, &SM2_P);
        let u1_h_sq = u1.mul_mod(&h_sq, &SM2_P);
        let r_sq = r.mul_mod(&r, &SM2_P);
        // X3 = R² - H³ - 2·U1·H²
        let x3 = r_sq
            .sub_mod(&h_cu, &SM2_P)
            .sub_mod(&u1_h_sq, &SM2_P)
            .sub_mod(&u1_h_sq, &SM2_P);
        // Y3 = R·(U1·H² - X3) - S1·H³
        let y3 = r
            .mul_mod(&u1_h_sq.sub_mod(&x3, &SM2_P), &SM2_P)
            .sub_mod(&s1.mul_mod(&h_cu, &SM2_P), &SM2_P);
        // Z3 = Z1·Z2·H
        let z3 = z1.mul_mod(&z2, &SM2_P).mul_mod(&h, &SM2_P);
        EcPoint {
            x: x3,
            y: y3,
            z: z3,
            is_infinity: false,
        }
    }

    /// 点倍：2 * self（Jacobian 坐标，CRY-001，无模逆）。
    ///
    /// a = -3 优化（SM2 曲线 a = p-3）：
    ///   A = X², B = Y², C = B²
    ///   D = 2·((X+B)² - A - C)
    ///   E = 3·(A - Z⁴)
    ///   X3 = E² - 2D, Y3 = E·(D-X3) - 8C, Z3 = 2·Y·Z
    pub fn double(&self) -> EcPoint {
        if self.is_infinity || self.y.is_zero() {
            return EcPoint::INFINITY;
        }
        let a = self.x.mul_mod(&self.x, &SM2_P);
        let b = self.y.mul_mod(&self.y, &SM2_P);
        let c = b.mul_mod(&b, &SM2_P);
        let x_plus_b = self.x.add_mod(&b, &SM2_P);
        let d = {
            let t = x_plus_b
                .mul_mod(&x_plus_b, &SM2_P)
                .sub_mod(&a, &SM2_P)
                .sub_mod(&c, &SM2_P);
            t.add_mod(&t, &SM2_P)
        };
        let z_sq = self.z.mul_mod(&self.z, &SM2_P);
        let z_4 = z_sq.mul_mod(&z_sq, &SM2_P);
        let e = a
            .sub_mod(&z_4, &SM2_P)
            .add_mod(&a.sub_mod(&z_4, &SM2_P), &SM2_P)
            .add_mod(&a.sub_mod(&z_4, &SM2_P), &SM2_P);
        let e_sq = e.mul_mod(&e, &SM2_P);
        let x3 = e_sq.sub_mod(&d, &SM2_P).sub_mod(&d, &SM2_P);
        let eight_c = c
            .add_mod(&c, &SM2_P) // 2C
            .add_mod(&c, &SM2_P) // 3C
            .add_mod(&c, &SM2_P) // 4C
            .add_mod(&c, &SM2_P) // 5C
            .add_mod(&c, &SM2_P) // 6C
            .add_mod(&c, &SM2_P) // 7C
            .add_mod(&c, &SM2_P); // 8C
        let y3 = e
            .mul_mod(&d.sub_mod(&x3, &SM2_P), &SM2_P)
            .sub_mod(&eight_c, &SM2_P);
        let two_yz = self
            .y
            .mul_mod(&self.z, &SM2_P)
            .add_mod(&self.y.mul_mod(&self.z, &SM2_P), &SM2_P);
        EcPoint {
            x: x3,
            y: y3,
            z: two_yz,
            is_infinity: false,
        }
    }

    /// 标量乘法 k * P（恒定时间，CRY-001）——Montgomery ladder +
    /// Brumley-Tuveri 标量掩码（k' = k + n，257 位）。
    ///
    /// - 掩码保证 k' 的最高位（bit256 或 bit255）为 1——ladder 首轮
    ///   r0 即非无穷远点，消除旧实现前导零阶段 `add(O, R1)` 快速路径
    ///   的时序差异（k=1 与 k≈2^256 耗时一致）。
    /// - 每轮无条件 add(r0,r1) + double×2，仅按位 ct_select 选择结果
    ///   （旧实现 `if k.bit(i)` 结果选择分支已消除）。
    /// - 正确性：k'·G = (k+n)·G = k·G（n 为 G 的阶）。
    pub fn scalar_mult(&self, k: &U256) -> EcPoint {
        if self.is_infinity || k.is_zero() {
            return EcPoint::INFINITY;
        }
        let (masked, carry) = k.add_full(&SM2_N); // k' = k + n（257 位）
        let mut r0 = EcPoint::INFINITY;
        let mut r1 = *self;
        for i in (0..=256).rev() {
            let bit = if i == 256 { carry == 1 } else { masked.bit(i) };
            let add_01 = r0.add(&r1);
            let dbl_r1 = r1.double();
            let dbl_r0 = r0.double();
            r0 = ct_select_point(&add_01, &dbl_r0, bit);
            r1 = ct_select_point(&dbl_r1, &add_01, bit);
        }
        r0
    }

    /// 转为仿射坐标（CRY-001）：末尾一次定次费马模逆。
    ///
    /// x = X/Z², y = Y/Z³（Z=1 时原样返回）。无穷远点返回自身。
    pub fn to_affine(&self) -> EcPoint {
        if self.is_infinity || self.z == crate::bigint::ONE {
            return *self;
        }
        let z_inv = match self.z.inv_mod(&SM2_P) {
            Ok(v) => v,
            Err(_) => return EcPoint::INFINITY,
        };
        let z_inv_sq = z_inv.mul_mod(&z_inv, &SM2_P);
        let z_inv_cu = z_inv_sq.mul_mod(&z_inv, &SM2_P);
        let x = self.x.mul_mod(&z_inv_sq, &SM2_P);
        let y = self.y.mul_mod(&z_inv_cu, &SM2_P);
        EcPoint {
            x,
            y,
            z: crate::bigint::ONE,
            is_infinity: false,
        }
    }

    /// 基点标量乘法 k * G.
    pub fn scalar_base_mult(k: &U256) -> EcPoint {
        EcPoint::generator().scalar_mult(k)
    }

    /// 编码为未压缩格式: 04 || x || y (65 bytes).
    ///
    /// CRY-001：先转仿射坐标再编码（Jacobian X/Y 不是曲线点坐标）。
    pub fn to_bytes_uncompressed(&self) -> [u8; 65] {
        let aff = self.to_affine();
        let mut out = [0u8; 65];
        out[0] = 0x04;
        out[1..33].copy_from_slice(&aff.x.to_be_bytes());
        out[33..65].copy_from_slice(&aff.y.to_be_bytes());
        out
    }

    /// 从字节解析点.
    ///
    /// 支持格式：
    /// - `04` 未压缩（65 字节）：04 || x || y
    /// - `02`/`03` 压缩（33 字节）：v0.31.0 暂不支持，返回 [`CryptoError::InvalidPointEncoding`]
    pub fn from_bytes(bytes: &[u8]) -> Result<EcPoint, CryptoError> {
        if bytes.is_empty() {
            return Err(CryptoError::InvalidPointEncoding);
        }
        match bytes[0] {
            0x04 => {
                if bytes.len() != 65 {
                    return Err(CryptoError::InvalidLength {
                        expected: 65,
                        actual: bytes.len(),
                    });
                }
                let mut x_bytes = [0u8; 32];
                let mut y_bytes = [0u8; 32];
                x_bytes.copy_from_slice(&bytes[1..33]);
                y_bytes.copy_from_slice(&bytes[33..65]);
                let point = EcPoint {
                    x: U256::from_be_bytes(&x_bytes),
                    y: U256::from_be_bytes(&y_bytes),
                    z: crate::bigint::ONE,
                    is_infinity: false,
                };
                if !point.is_on_curve() {
                    return Err(CryptoError::PointNotOnCurve);
                }
                Ok(point)
            }
            // 压缩格式 (02/03): v0.31.0 暂不支持，需实现模幂运算以计算平方根
            0x02 | 0x03 => Err(CryptoError::InvalidPointEncoding),
            _ => Err(CryptoError::InvalidPointEncoding),
        }
    }
}

// ============================================================
// Sm2PrivateKey: SM2 私钥
// ============================================================

/// SM2 私钥 (256-bit 标量 d ∈ [1, n-1]).
///
/// 实现 [`Drop`] trait，析构时自动安全清零。
/// `Debug` impl 不暴露私钥内容（输出 `<Sm2PrivateKey: redacted>`）以防止日志泄露。
#[derive(Clone, PartialEq, Eq)]
pub struct Sm2PrivateKey {
    pub d: U256,
}

impl core::fmt::Debug for Sm2PrivateKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("<Sm2PrivateKey: redacted>")
    }
}

impl Sm2PrivateKey {
    /// 从大端序 32 字节创建私钥（验证范围 [1, n-1]）.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        let d = U256::from_be_bytes(bytes);
        if d.is_zero() || d >= SM2_N {
            return Err(CryptoError::ScalarOutOfRange);
        }
        Ok(Self { d })
    }

    /// 转换为 32 字节大端序.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.d.to_be_bytes()
    }

    /// 安全清零.
    pub fn zeroize(&mut self) {
        self.d.zeroize();
    }
}

impl Drop for Sm2PrivateKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// ============================================================
// Sm2PublicKey: SM2 公钥
// ============================================================

/// SM2 公钥（椭圆曲线点）.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sm2PublicKey {
    pub point: EcPoint,
}

impl Sm2PublicKey {
    /// 从未压缩字节创建公钥 (65 字节, 04 前缀).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let point = EcPoint::from_bytes(bytes)?;
        Ok(Self { point })
    }

    /// 转换为未压缩字节 (65 字节, 04 前缀).
    pub fn to_bytes_uncompressed(&self) -> [u8; 65] {
        self.point.to_bytes_uncompressed()
    }
}

// ============================================================
// Sm2KeyPair: SM2 密钥对
// ============================================================

/// SM2 密钥对.
///
/// `Debug` impl 不暴露私钥内容（输出 `<Sm2KeyPair: redacted>`）以防止日志泄露。
#[derive(Clone)]
pub struct Sm2KeyPair {
    pub private_key: Sm2PrivateKey,
    pub public_key: Sm2PublicKey,
}

impl core::fmt::Debug for Sm2KeyPair {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("<Sm2KeyPair: redacted>")
    }
}

impl Sm2KeyPair {
    /// 从私钥派生密钥对: P = d * G.
    ///
    /// CRY-001：公钥点规范化到仿射坐标（z=1）——Jacobian 表示不唯一，
    /// 公钥语义要求 affine 点（序列化/比较/证书一致）。
    pub fn from_private_key(sk: &Sm2PrivateKey) -> Result<Self, CryptoError> {
        let point = EcPoint::scalar_base_mult(&sk.d).to_affine();
        if point.is_infinity {
            return Err(CryptoError::InternalError);
        }
        Ok(Self {
            private_key: sk.clone(),
            public_key: Sm2PublicKey { point },
        })
    }

    /// 使用 CSRNG 生成密钥对.
    ///
    /// 循环生成随机标量直到 d ∈ [1, n-1]。
    pub fn generate(rng: &mut crate::rng::CsRng) -> Result<Self, CryptoError> {
        loop {
            let mut buf = [0u8; 32];
            fill_bytes_or_reseed(rng, &mut buf).map_err(|_| CryptoError::RngFailed)?;
            match Sm2PrivateKey::from_bytes(&buf) {
                Ok(sk) => return Self::from_private_key(&sk),
                Err(CryptoError::ScalarOutOfRange) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

// ============================================================
// ============================================================================
// 恒定时间点选择（CRY-001）
// ============================================================================

/// 按 bit 恒定时间选择两个点之一（逐 limb 掩码，无数据相关分支）。
pub fn ct_select_point_for_test(a: &EcPoint, b: &EcPoint, bit: bool) -> EcPoint {
    ct_select_point(a, b, bit)
}

/// 按 bit 恒定时间选择两个点之一（逐 limb 掩码，无数据相关分支）。
fn ct_select_point(a: &EcPoint, b: &EcPoint, bit: bool) -> EcPoint {
    let mask = if bit { u64::MAX } else { 0 };
    let nmask = !mask;
    let mut x = [0u64; 4];
    let mut y = [0u64; 4];
    let mut z = [0u64; 4];
    for i in 0..4 {
        x[i] = (a.x.limbs[i] & mask) | (b.x.limbs[i] & nmask);
        y[i] = (a.y.limbs[i] & mask) | (b.y.limbs[i] & nmask);
        z[i] = (a.z.limbs[i] & mask) | (b.z.limbs[i] & nmask);
    }
    EcPoint {
        x: U256 { limbs: x },
        y: U256 { limbs: y },
        z: U256 { limbs: z },
        is_infinity: (a.is_infinity as u8 & (bit as u8)) != 0
            || (b.is_infinity as u8 & (!bit as u8)) != 0,
    }
}

// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bigint::U256;
    use crate::rng::CsRng;

    // ============================================================
    // 曲线常量与基点验证
    // ============================================================

    #[test]
    fn test_generator_on_curve() {
        let g = EcPoint::generator();
        assert!(g.is_on_curve(), "基点 G 必须在 SM2 曲线上");
    }

    #[test]
    fn test_generator_correct_value() {
        let g = EcPoint::generator();
        assert_eq!(g.x, SM2_GX, "Gx 必须匹配 GB/T 32918.5");
        assert_eq!(g.y, SM2_GY, "Gy 必须匹配 GB/T 32918.5");
        assert!(!g.is_infinity);
    }

    // ============================================================
    // 点加测试
    // ============================================================

    #[test]
    fn test_infinity_add_self() {
        let result = EcPoint::INFINITY.add(&EcPoint::INFINITY);
        assert_eq!(result, EcPoint::INFINITY);
    }

    #[test]
    fn test_infinity_add_point() {
        let g = EcPoint::generator();
        let result = EcPoint::INFINITY.add(&g);
        assert_eq!(result, g);
    }

    #[test]
    fn test_point_add_inverse() {
        let g = EcPoint::generator();
        // -G = (Gx, -Gy mod p) = (Gx, p - Gy)
        let neg_g = EcPoint {
            x: g.x,
            y: SM2_P.sub_mod(&g.y, &SM2_P),
            z: crate::bigint::ONE,
            is_infinity: false,
        };
        let result = g.add(&neg_g);
        assert_eq!(result, EcPoint::INFINITY, "P + (-P) = O");
    }

    #[test]
    fn test_point_add_self() {
        let g = EcPoint::generator();
        let added = g.add(&g);
        let doubled = g.double();
        assert_eq!(added, doubled, "P + P 应等于 2*P (double)");
        assert!(added.is_on_curve(), "结果点必须在曲线上");
    }

    // ============================================================
    // 点倍测试
    // ============================================================

    #[test]
    fn test_point_double() {
        let g = EcPoint::generator();
        let doubled = g.double();
        assert!(!doubled.is_infinity);
        assert!(doubled.is_on_curve(), "2*G 必须在曲线上");
    }

    #[test]
    fn test_point_double_infinity() {
        let result = EcPoint::INFINITY.double();
        assert_eq!(result, EcPoint::INFINITY);
    }

    // ============================================================
    // 标量乘法测试
    // ============================================================

    #[test]
    fn test_scalar_mult_zero() {
        let g = EcPoint::generator();
        let zero = U256 { limbs: [0; 4] };
        let result = g.scalar_mult(&zero);
        assert_eq!(result, EcPoint::INFINITY, "0 * G = O");
    }

    #[test]
    fn test_scalar_mult_one() {
        let g = EcPoint::generator();
        let one = U256 {
            limbs: [1, 0, 0, 0],
        };
        let result = g.scalar_mult(&one).to_affine();
        assert_eq!(result, g, "1 * G = G");
    }

    #[test]
    fn test_scalar_mult_two() {
        let g = EcPoint::generator();
        let two = U256 {
            limbs: [2, 0, 0, 0],
        };
        let result = g.scalar_mult(&two).to_affine();
        assert_eq!(result, g.double().to_affine(), "2 * G = G.double()");
    }

    #[test]
    fn test_scalar_mult_n() {
        // n * G = O (曲线阶的基本性质)
        let result = EcPoint::scalar_base_mult(&SM2_N);
        assert_eq!(result, EcPoint::INFINITY, "n * G = O (曲线阶性质)");
    }

    #[test]
    fn test_scalar_mult_distributive() {
        // (a + b) * G = a*G + b*G
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let b = U256 {
            limbs: [5, 0, 0, 0],
        };
        let a_plus_b = a.add_mod(&b, &SM2_N);
        let left = EcPoint::scalar_base_mult(&a_plus_b).to_affine();
        let right = EcPoint::scalar_base_mult(&a)
            .add(&EcPoint::scalar_base_mult(&b))
            .to_affine();
        assert_eq!(left, right, "(a+b)*G = a*G + b*G");
    }

    #[test]
    fn test_scalar_mult_associative() {
        // k * (l * G) = (k*l mod n) * G
        let k = U256 {
            limbs: [7, 0, 0, 0],
        };
        let l = U256 {
            limbs: [11, 0, 0, 0],
        };
        let l_g = EcPoint::scalar_base_mult(&l);
        let left = l_g.scalar_mult(&k).to_affine();
        let kl = k.mul_mod(&l, &SM2_N);
        let right = EcPoint::scalar_base_mult(&kl).to_affine();
        assert_eq!(left, right, "k*(l*G) = (k*l)*G");
    }

    // ============================================================
    // 点序列化测试
    // ============================================================

    #[test]
    fn test_to_bytes_from_bytes_round_trip() {
        let g = EcPoint::generator();
        let encoded = g.to_bytes_uncompressed();
        let decoded = EcPoint::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, g, "编解码往返一致");
    }

    #[test]
    fn test_from_bytes_invalid_prefix() {
        let result = EcPoint::from_bytes(&[0x01; 65]);
        assert_eq!(result, Err(CryptoError::InvalidPointEncoding));
    }

    #[test]
    fn test_from_bytes_invalid_length() {
        let result = EcPoint::from_bytes(&[0x04; 33]);
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 65,
                actual: 33
            })
        );
    }

    #[test]
    fn test_from_bytes_point_not_on_curve() {
        // 构造一个不在曲线上的点: 04 || 0...0 || 0...0
        let mut bytes = [0u8; 65];
        bytes[0] = 0x04;
        // x=0, y=0 → y^2=0, x^3+ax+b=b≠0，不在曲线上
        let result = EcPoint::from_bytes(&bytes);
        assert_eq!(result, Err(CryptoError::PointNotOnCurve));
    }

    #[test]
    fn test_from_bytes_compressed_unsupported() {
        let bytes = [0x02u8; 33];
        let result = EcPoint::from_bytes(&bytes);
        assert_eq!(result, Err(CryptoError::InvalidPointEncoding));
    }

    // ============================================================
    // 私钥测试
    // ============================================================

    #[test]
    fn test_private_key_from_bytes_valid() {
        // d = 1 (有效标量)
        let mut bytes = [0u8; 32];
        bytes[31] = 1;
        let sk = Sm2PrivateKey::from_bytes(&bytes).unwrap();
        assert_eq!(
            sk.d,
            U256 {
                limbs: [1, 0, 0, 0]
            }
        );
    }

    #[test]
    fn test_private_key_from_bytes_zero() {
        let bytes = [0u8; 32];
        let result = Sm2PrivateKey::from_bytes(&bytes);
        assert_eq!(result, Err(CryptoError::ScalarOutOfRange));
    }

    #[test]
    fn test_private_key_from_bytes_n() {
        // d = n (超出范围 [1, n-1])
        let bytes = SM2_N.to_be_bytes();
        let result = Sm2PrivateKey::from_bytes(&bytes);
        assert_eq!(result, Err(CryptoError::ScalarOutOfRange));
    }

    #[test]
    fn test_private_key_zeroize() {
        let mut bytes = [0u8; 32];
        bytes[31] = 0x42;
        let mut sk = Sm2PrivateKey::from_bytes(&bytes).unwrap();
        sk.zeroize();
        assert!(sk.d.is_zero(), "zeroize 后私钥应为零");
    }

    // ============================================================
    // 密钥对测试
    // ============================================================

    #[test]
    fn test_keypair_from_private_key() {
        let mut bytes = [0u8; 32];
        bytes[31] = 0x01;
        let sk = Sm2PrivateKey::from_bytes(&bytes).unwrap();
        let kp = Sm2KeyPair::from_private_key(&sk).unwrap();
        // P = 1 * G = G
        assert_eq!(kp.public_key.point, EcPoint::generator());
        assert!(kp.public_key.point.is_on_curve());
    }

    #[test]
    fn test_keypair_generate() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        assert!(kp.public_key.point.is_on_curve(), "生成的公钥必须在曲线上");
        assert!(!kp.public_key.point.is_infinity);
        // 验证 P = d * G（公钥为 affine，expected 也转 affine 比较）
        let expected = EcPoint::scalar_base_mult(&kp.private_key.d).to_affine();
        assert_eq!(kp.public_key.point, expected);
    }

    #[test]
    fn test_public_key_serialization() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let encoded = kp.public_key.to_bytes_uncompressed();
        let decoded = Sm2PublicKey::from_bytes(&encoded).unwrap();
        // 序列化是 affine 规范化：from_bytes(to_bytes(p)) == p.to_affine()
        assert_eq!(decoded.point, kp.public_key.point.to_affine());
    }
}
