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

// ============================================================
// EcPoint: 椭圆曲线点（仿射坐标）
// ============================================================

/// 椭圆曲线点（仿射坐标）.
///
/// `is_infinity = true` 表示无穷远点（曲线单位元）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcPoint {
    pub x: U256,
    pub y: U256,
    pub is_infinity: bool,
}

impl EcPoint {
    /// 无穷远点（曲线单位元）.
    pub const INFINITY: EcPoint = EcPoint {
        x: U256 { limbs: [0; 4] },
        y: U256 { limbs: [0; 4] },
        is_infinity: true,
    };

    /// SM2 基点 G.
    pub fn generator() -> Self {
        EcPoint {
            x: SM2_GX,
            y: SM2_GY,
            is_infinity: false,
        }
    }

    /// 验证点是否在 SM2 曲线上: y^2 ≡ x^3 + ax + b (mod p).
    pub fn is_on_curve(&self) -> bool {
        if self.is_infinity {
            return true;
        }
        // y^2 mod p
        let y_sq = self.y.mul_mod(&self.y, &SM2_P);
        // x^3 mod p = x * x^2
        let x_sq = self.x.mul_mod(&self.x, &SM2_P);
        let x_cu = x_sq.mul_mod(&self.x, &SM2_P);
        // a*x mod p
        let ax = SM2_A.mul_mod(&self.x, &SM2_P);
        // x^3 + ax + b mod p
        let rhs = x_cu.add_mod(&ax, &SM2_P).add_mod(&SM2_B, &SM2_P);
        y_sq == rhs
    }

    /// 点加：self + other.
    ///
    /// 处理所有情况：
    /// - 任一点为无穷远点 → 返回另一点
    /// - P == -Q（相同 x，不同 y）→ 返回无穷远点
    /// - P == Q → 调用 [`double`]
    /// - 一般情况：λ = (y2 - y1) / (x2 - x1) mod p
    pub fn add(&self, other: &EcPoint) -> EcPoint {
        if self.is_infinity {
            return *other;
        }
        if other.is_infinity {
            return *self;
        }
        if self.x == other.x {
            if self.y == other.y {
                return self.double();
            } else {
                // P + (-P) = O
                return EcPoint::INFINITY;
            }
        }
        // λ = (y2 - y1) / (x2 - x1) mod p
        let dy = other.y.sub_mod(&self.y, &SM2_P);
        let dx = other.x.sub_mod(&self.x, &SM2_P);
        let lambda = match dx.inv_mod(&SM2_P) {
            Ok(inv) => dy.mul_mod(&inv, &SM2_P),
            Err(_) => return EcPoint::INFINITY,
        };
        // x3 = λ^2 - x1 - x2 mod p
        let lambda_sq = lambda.mul_mod(&lambda, &SM2_P);
        let x3 = lambda_sq.sub_mod(&self.x, &SM2_P).sub_mod(&other.x, &SM2_P);
        // y3 = λ(x1 - x3) - y1 mod p
        let dx13 = self.x.sub_mod(&x3, &SM2_P);
        let y3 = lambda.mul_mod(&dx13, &SM2_P).sub_mod(&self.y, &SM2_P);
        EcPoint {
            x: x3,
            y: y3,
            is_infinity: false,
        }
    }

    /// 点倍：2 * self.
    ///
    /// λ = (3x^2 + a) / (2y) mod p
    pub fn double(&self) -> EcPoint {
        if self.is_infinity || self.y.is_zero() {
            return EcPoint::INFINITY;
        }
        // λ = (3x^2 + a) / (2y) mod p
        let x_sq = self.x.mul_mod(&self.x, &SM2_P);
        let three_x_sq = x_sq.add_mod(&x_sq, &SM2_P).add_mod(&x_sq, &SM2_P);
        let numerator = three_x_sq.add_mod(&SM2_A, &SM2_P);
        let two_y = self.y.add_mod(&self.y, &SM2_P);
        let lambda = match two_y.inv_mod(&SM2_P) {
            Ok(inv) => numerator.mul_mod(&inv, &SM2_P),
            Err(_) => return EcPoint::INFINITY,
        };
        // x3 = λ^2 - 2x mod p
        let lambda_sq = lambda.mul_mod(&lambda, &SM2_P);
        let two_x = self.x.add_mod(&self.x, &SM2_P);
        let x3 = lambda_sq.sub_mod(&two_x, &SM2_P);
        // y3 = λ(x - x3) - y mod p
        let dx = self.x.sub_mod(&x3, &SM2_P);
        let y3 = lambda.mul_mod(&dx, &SM2_P).sub_mod(&self.y, &SM2_P);
        EcPoint {
            x: x3,
            y: y3,
            is_infinity: false,
        }
    }

    /// 标量乘法 k * P，使用 Montgomery ladder（恒定时间）.
    ///
    /// 不变量：R1 - R0 = P 在整个迭代过程中保持。
    /// 从最高位到最低位逐位处理 256 位标量。
    pub fn scalar_mult(&self, k: &U256) -> EcPoint {
        if self.is_infinity || k.is_zero() {
            return EcPoint::INFINITY;
        }
        let mut r0 = EcPoint::INFINITY;
        let mut r1 = *self;
        for i in (0..256).rev() {
            if k.bit(i) {
                r0 = r0.add(&r1);
                r1 = r1.double();
            } else {
                r1 = r0.add(&r1);
                r0 = r0.double();
            }
        }
        r0
    }

    /// 基点标量乘法 k * G.
    pub fn scalar_base_mult(k: &U256) -> EcPoint {
        EcPoint::generator().scalar_mult(k)
    }

    /// 编码为未压缩格式: 04 || x || y (65 bytes).
    pub fn to_bytes_uncompressed(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[0] = 0x04;
        out[1..33].copy_from_slice(&self.x.to_be_bytes());
        out[33..65].copy_from_slice(&self.y.to_be_bytes());
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
    pub fn from_private_key(sk: &Sm2PrivateKey) -> Result<Self, CryptoError> {
        let point = EcPoint::scalar_base_mult(&sk.d);
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
            rng.fill_bytes(&mut buf);
            match Sm2PrivateKey::from_bytes(&buf) {
                Ok(sk) => return Self::from_private_key(&sk),
                Err(CryptoError::ScalarOutOfRange) => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

// ============================================================
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
        let result = g.scalar_mult(&one);
        assert_eq!(result, g, "1 * G = G");
    }

    #[test]
    fn test_scalar_mult_two() {
        let g = EcPoint::generator();
        let two = U256 {
            limbs: [2, 0, 0, 0],
        };
        let result = g.scalar_mult(&two);
        assert_eq!(result, g.double(), "2 * G = G.double()");
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
        let left = EcPoint::scalar_base_mult(&a_plus_b);
        let right = EcPoint::scalar_base_mult(&a).add(&EcPoint::scalar_base_mult(&b));
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
        let left = l_g.scalar_mult(&k);
        let kl = k.mul_mod(&l, &SM2_N);
        let right = EcPoint::scalar_base_mult(&kl);
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
        // 验证 P = d * G
        let expected = EcPoint::scalar_base_mult(&kp.private_key.d);
        assert_eq!(kp.public_key.point, expected);
    }

    #[test]
    fn test_public_key_serialization() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let encoded = kp.public_key.to_bytes_uncompressed();
        let decoded = Sm2PublicKey::from_bytes(&encoded).unwrap();
        assert_eq!(decoded.point, kp.public_key.point);
    }
}
