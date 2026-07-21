//! U256 256-bit 无符号大整数，用于 SM2 椭圆曲线运算.
//!
//! `U256` 表示 256 位无符号整数，采用小端序 64 位 limb 存储（`limbs[0]` 为最低有效字）。
//! 提供：
//! - 字节/十六进制转换
//! - 比较与位运算
//! - 模加、模减、模乘、模逆
//!
//! # 安全说明
//! `zeroize` 方法使用 `crate::constant_time::ct_zeroize` 安全清除敏感数据。
//! 模逆使用迭代扩展欧几里得算法（非递归，避免栈溢出）。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::string::String;
use core::cmp::Ordering;

use crate::error::CryptoError;

/// 256-bit unsigned integer, little-endian limbs.
///
/// `limbs[0]` is the least significant 64-bit word.
/// Used for SM2 elliptic curve arithmetic (P, A, B, N, scalars, coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct U256 {
    pub limbs: [u64; 4],
}

/// Zero constant: `U256 { limbs: [0; 4] }`.
pub const ZERO: U256 = U256 { limbs: [0; 4] };

/// One constant: `U256 { limbs: [1, 0, 0, 0] }`.
pub const ONE: U256 = U256 {
    limbs: [1, 0, 0, 0],
};

impl U256 {
    // ============================================================
    // SubTask 2.2: byte/hex conversions
    // ============================================================

    /// 从大端序 32 字节构造 `U256`.
    ///
    /// `bytes[0]` 为最高有效字节。
    pub fn from_be_bytes(bytes: &[u8; 32]) -> U256 {
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let offset = (3 - i) * 8;
            *limb = u64::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
        }
        U256 { limbs }
    }

    /// 转换为大端序 32 字节.
    ///
    /// `bytes[0]` 为最高有效字节。
    pub fn to_be_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for i in 0..4 {
            let offset = (3 - i) * 8;
            let b = self.limbs[i].to_be_bytes();
            bytes[offset..offset + 8].copy_from_slice(&b);
        }
        bytes
    }

    /// 从 64 字符十六进制字符串解析 `U256`.
    ///
    /// 接受大小写十六进制字符。返回 `CryptoError::InvalidLength` 当长度 ≠ 64，
    /// 返回 `CryptoError::InvalidInput` 当包含非十六进制字符。
    pub fn from_hex(s: &str) -> Result<U256, CryptoError> {
        if s.len() != 64 {
            return Err(CryptoError::InvalidLength {
                expected: 64,
                actual: s.len(),
            });
        }
        let mut bytes = [0u8; 32];
        let src = s.as_bytes();
        for i in 0..32 {
            let hi = hex_digit(src[i * 2])?;
            let lo = hex_digit(src[i * 2 + 1])?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(U256::from_be_bytes(&bytes))
    }

    /// 转换为大写十六进制字符串（64 字符）.
    pub fn to_hex(&self) -> String {
        let bytes = self.to_be_bytes();
        const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";
        let mut result = String::with_capacity(64);
        for &byte in bytes.iter() {
            result.push(HEX_CHARS[(byte >> 4) as usize] as char);
            result.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
        }
        result
    }

    // ============================================================
    // SubTask 2.3: comparison and bit operations
    // ============================================================

    /// 判断是否为零.
    pub fn is_zero(&self) -> bool {
        self.limbs[0] == 0 && self.limbs[1] == 0 && self.limbs[2] == 0 && self.limbs[3] == 0
    }

    /// 三态比较（从最高位 limb 向下比较）.
    pub fn cmp_u256(&self, other: &U256) -> Ordering {
        for i in (0..4).rev() {
            match self.limbs[i].cmp(&other.limbs[i]) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    /// 获取第 `i` 位（0 = LSB）。`i >= 256` 返回 `false`.
    pub fn bit(&self, i: usize) -> bool {
        if i >= 256 {
            return false;
        }
        (self.limbs[i / 64] >> (i % 64)) & 1 == 1
    }

    /// 表示该值所需的最小位数（0 返回 0）.
    pub fn bit_len(&self) -> usize {
        for i in (0..4).rev() {
            if self.limbs[i] != 0 {
                return i * 64 + (64 - self.limbs[i].leading_zeros() as usize);
            }
        }
        0
    }

    /// Check if self < other (unsigned comparison).
    fn lt(&self, other: &U256) -> bool {
        self.cmp_u256(other) == Ordering::Less
    }

    // ============================================================
    // SubTask 2.4: modular add/sub
    // ============================================================

    /// 模加：`(self + other) mod m`.
    ///
    /// 假设 `self, other < m`。使用 `u128` 中间结果处理进位传播。
    pub fn add_mod(&self, other: &U256, m: &U256) -> U256 {
        // self + other, 结果可能溢出 256 位
        let (sum, carry) = self.add_full(other);
        // sum + carry * 2^256 < 2*m（因 self, other < m）
        // 若 carry == 1 或 sum >= m，则减去 m
        let need_sub = carry > 0 || sum.cmp_u256(m) != Ordering::Less;
        if need_sub {
            // (sum + carry * 2^256) - m 的低 256 位 = (sum - m) mod 2^256
            // 当 carry == 1 时，sub_full 产生的借位被隐含的 2^256 吸收
            // 当 carry == 0 时，sum >= m 保证无借位
            sum.sub_full(m).0
        } else {
            sum
        }
    }

    /// 模减：`(self - other) mod m`，处理 `self < other` 的情况.
    ///
    /// 假设 `self, other < m`。当 `self < other` 时计算 `(self + m - other) mod m`。
    pub fn sub_mod(&self, other: &U256, m: &U256) -> U256 {
        if !self.lt(other) {
            // self >= other: self - other，结果 < m（因 self, other < m）
            self.sub_full(other).0
        } else {
            // self < other: (self + m) - other
            // self + m 可能溢出 256 位（当 m >= 2^255 时），需处理进位
            let (sum, carry) = self.add_full(m);
            // sum + carry * 2^256 = self + m
            // (self + m) - other = (sum - other) + carry * 2^256
            // 因 self + m > other（因 m > other > self），结果为正
            let (diff, borrow) = sum.sub_full(other);
            // carry 吸收 borrow：carry - borrow = 0（结果适配 256 位）
            // 数学保证 carry >= borrow（因结果为正且 < 2^256），两者均不直接使用
            let _ = (carry, borrow);
            diff
        }
    }

    // ============================================================
    // SubTask 2.5: modular multiplication
    // ============================================================

    /// 模乘：`(self * other) mod m`.
    ///
    /// 先做教科书乘法（256×256→512 位），再用 `reduce_512_to_256` 归约。
    /// 正确性不依赖 `self, other < m`（归约步骤处理任意 512 位输入）。
    pub fn mul_mod(&self, other: &U256, m: &U256) -> U256 {
        let prod = self.mul_full(other);
        reduce_512_to_256(&prod, m)
    }

    // ============================================================
    // SubTask 2.6: modular inverse (Extended Euclidean Algorithm)
    // ============================================================

    /// 模逆：`self^(-1) mod m`，使用迭代扩展欧几里得算法.
    ///
    /// 返回 `CryptoError::ModInverseFailed` 当 `gcd(self, m) != 1` 或 `self == 0` 或 `m == 0`。
    /// 系数运算在 mod m 下进行，避免有符号算术。
    pub fn inv_mod(&self, m: &U256) -> Result<U256, CryptoError> {
        if m.is_zero() {
            return Err(CryptoError::ModInverseFailed);
        }
        if self.is_zero() {
            return Err(CryptoError::ModInverseFailed);
        }

        // 先归约 self mod m（保证 a < m，使后续 mul_mod 的商 q < m）
        let a = if self.cmp_u256(m) != Ordering::Less {
            self.div_rem(m).1
        } else {
            *self
        };
        if a.is_zero() {
            return Err(CryptoError::ModInverseFailed);
        }

        // 迭代 EEA:
        //   old_r, r = a, m
        //   old_s, s = 1, 0
        //   while r != 0:
        //     q = old_r / r
        //     old_r, r = r, old_r - q*r
        //     old_s, s = s, (old_s - q*s) mod m
        let mut old_r = a;
        let mut r = *m;
        let mut old_s = ONE;
        let mut s = ZERO;

        while !r.is_zero() {
            let (q, rem) = old_r.div_rem(&r);
            old_r = r;
            r = rem;

            // s_new = (old_s - q * s) mod m
            // q < m（因 old_r < m，q = old_r / r < m）
            let temp = q.mul_mod(&s, m);
            let s_new = old_s.sub_mod(&temp, m);

            old_s = s;
            s = s_new;
        }

        if old_r != ONE {
            return Err(CryptoError::ModInverseFailed);
        }
        Ok(old_s)
    }

    // ============================================================
    // Helper methods (private)
    // ============================================================

    /// 全精度加法，返回 (result, carry)。carry ∈ {0, 1}.
    fn add_full(&self, other: &U256) -> (U256, u64) {
        let mut result = [0u64; 4];
        let mut carry: u64 = 0;
        for ((r, a), b) in result
            .iter_mut()
            .zip(self.limbs.iter())
            .zip(other.limbs.iter())
        {
            let sum = (*a as u128) + (*b as u128) + (carry as u128);
            *r = sum as u64;
            carry = (sum >> 64) as u64;
        }
        (U256 { limbs: result }, carry)
    }

    /// 全精度减法，返回 (result, borrow)。borrow ∈ {0, 1}.
    /// 当 self < other 时使用环绕减法（two's complement）。
    fn sub_full(&self, other: &U256) -> (U256, u64) {
        let mut result = [0u64; 4];
        let mut borrow: u64 = 0;
        for ((r, a), b) in result
            .iter_mut()
            .zip(self.limbs.iter())
            .zip(other.limbs.iter())
        {
            let (diff1, b1) = a.overflowing_sub(*b);
            let (diff2, b2) = diff1.overflowing_sub(borrow);
            *r = diff2;
            borrow = (b1 as u64) + (b2 as u64);
        }
        (U256 { limbs: result }, borrow)
    }

    /// 教科书乘法：256 × 256 → 512 位（8 limbs）.
    fn mul_full(&self, other: &U256) -> [u64; 8] {
        let mut result = [0u64; 8];
        for i in 0..4 {
            let mut carry: u128 = 0;
            for j in 0..4 {
                let idx = i + j;
                let sum = (result[idx] as u128)
                    + (self.limbs[i] as u128) * (other.limbs[j] as u128)
                    + carry;
                result[idx] = sum as u64;
                carry = sum >> 64;
            }
            // 将剩余进位传播到更高位
            let mut idx = i + 4;
            let mut c = carry;
            while c > 0 && idx < 8 {
                let sum = (result[idx] as u128) + c;
                result[idx] = sum as u64;
                c = sum >> 64;
                idx += 1;
            }
        }
        result
    }

    /// 无符号除法，返回 (quotient, remainder).
    ///
    /// 使用移位-减法（长除法）。`divisor` 为零时返回 (ZERO, self)。
    fn div_rem(&self, divisor: &U256) -> (U256, U256) {
        if divisor.is_zero() {
            return (ZERO, *self);
        }
        let mut quotient = ZERO;
        let mut remainder = ZERO;

        for i in (0..256).rev() {
            // remainder 左移 1 位前，先捕获最高位（将作为第 256 位）
            let overflow = (remainder.limbs[3] >> 63) & 1 == 1;
            remainder = remainder.shl1();
            // 设置 bit 0 来自 self 的第 i 位
            if self.bit(i) {
                remainder.limbs[0] |= 1;
            }

            // 判断 (remainder, overflow) >= divisor
            // overflow 为 true 时，remainder + 2^256 > 任何 U256
            let ge = overflow || remainder.cmp_u256(divisor) != Ordering::Less;

            if ge {
                // (remainder + overflow * 2^256) - divisor
                // 使用环绕减法：overflow 为 true 时借位被 2^256 吸收
                let (diff, _borrow) = remainder.sub_full(divisor);
                remainder = diff;

                // 设置商的第 i 位
                let limb_idx = i / 64;
                let bit_idx = i % 64;
                quotient.limbs[limb_idx] |= 1u64 << bit_idx;
            }
        }
        (quotient, remainder)
    }

    /// 左移 1 位.
    fn shl1(&self) -> U256 {
        let mut result = [0u64; 4];
        let mut carry = 0u64;
        for (r, &s) in result.iter_mut().zip(self.limbs.iter()) {
            *r = (s << 1) | carry;
            carry = s >> 63;
        }
        U256 { limbs: result }
    }

    /// 安全清零（用于私钥/标量等敏感数据）.
    ///
    /// 使用 `crate::constant_time::ct_zeroize` 进行恒定时间清零，
    /// 防止编译器优化删除清零操作。
    pub fn zeroize(&mut self) {
        for limb in self.limbs.iter_mut() {
            let mut bytes = limb.to_ne_bytes();
            crate::constant_time::ct_zeroize(&mut bytes);
            *limb = u64::from_ne_bytes(bytes);
        }
    }
}

// ============================================================
// Ord / PartialOrd implementations
// ============================================================

impl Ord for U256 {
    fn cmp(&self, other: &U256) -> Ordering {
        self.cmp_u256(other)
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &U256) -> Option<Ordering> {
        Some(<Self as Ord>::cmp(self, other))
    }
}

// ============================================================
// Private free functions
// ============================================================

/// 将单个十六进制字符转换为数值.
fn hex_digit(c: u8) -> Result<u8, CryptoError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(CryptoError::InvalidInput("non-hex character".into())),
    }
}

/// 512 位无符号比较：`a >= b`.
fn ge_512(a: &[u64; 8], b: &[u64; 8]) -> bool {
    for i in (0..8).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true // 相等
}

/// 512 位就地减法：`a -= b`（假设 `a >= b`）.
fn sub_512(a: &mut [u64; 8], b: &[u64; 8]) {
    let mut borrow: u64 = 0;
    for i in 0..8 {
        let (diff1, b1) = a[i].overflowing_sub(b[i]);
        let (diff2, b2) = diff1.overflowing_sub(borrow);
        a[i] = diff2;
        borrow = (b1 as u64) + (b2 as u64);
    }
}

/// 将 512 位值（8 limbs）归约 mod m（4 limbs）为 `U256`.
///
/// 使用移位-减法长除法，从最高位到最低位逐位处理。
/// 不变量：每步后 remainder < m（适配 256 位，但中间用 8 limbs 防溢出）。
fn reduce_512_to_256(prod: &[u64; 8], m: &U256) -> U256 {
    // m 扩展为 8 limbs（高位补零）
    let m_ext: [u64; 8] = [m.limbs[0], m.limbs[1], m.limbs[2], m.limbs[3], 0, 0, 0, 0];
    let mut rem: [u64; 8] = [0; 8];

    for i in (0..512).rev() {
        // rem 左移 1 位
        let mut carry = false;
        for r in rem.iter_mut() {
            let new_carry = (*r >> 63) & 1 == 1;
            *r = (*r << 1) | (if carry { 1 } else { 0 });
            carry = new_carry;
        }
        // 设置 bit 0 来自 prod 的第 i 位
        if (prod[i / 64] >> (i % 64)) & 1 == 1 {
            rem[0] |= 1;
        }
        // 若 rem >= m_ext，减去 m_ext
        if ge_512(&rem, &m_ext) {
            sub_512(&mut rem, &m_ext);
        }
    }

    U256 {
        limbs: [rem[0], rem[1], rem[2], rem[3]],
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // SM2 curve constants (for testing)
    // SM2 prime modulus P (256-bit, 64 hex chars) per GB/T 32918.5-2017
    const SM2_P: &str = "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF";
    // SM2 curve order N (256-bit, 64 hex chars)
    const SM2_N: &str = "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123";

    // ============================================================
    // SubTask 2.2: byte/hex conversion tests
    // ============================================================

    #[test]
    fn test_from_be_bytes_to_be_bytes_roundtrip() {
        let bytes = [
            0x01u8, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x10, 0x20, 0x30, 0x40,
            0x50, 0x60, 0x70, 0x80,
        ];
        let val = U256::from_be_bytes(&bytes);
        let out = val.to_be_bytes();
        assert_eq!(out, bytes);
    }

    #[test]
    fn test_from_be_bytes_zero() {
        let bytes = [0u8; 32];
        let val = U256::from_be_bytes(&bytes);
        assert!(val.is_zero());
        assert_eq!(val, ZERO);
    }

    #[test]
    fn test_from_be_bytes_max() {
        let bytes = [0xFFu8; 32];
        let val = U256::from_be_bytes(&bytes);
        assert_eq!(val.limbs, [u64::MAX, u64::MAX, u64::MAX, u64::MAX]);
    }

    #[test]
    fn test_from_be_bytes_one() {
        let mut bytes = [0u8; 32];
        bytes[31] = 1;
        let val = U256::from_be_bytes(&bytes);
        assert_eq!(val, ONE);
        assert_eq!(val.limbs, [1, 0, 0, 0]);
    }

    #[test]
    fn test_from_hex_valid() {
        let hex = "0000000000000000000000000000000000000000000000000000000000000001";
        let val = U256::from_hex(hex).unwrap();
        assert_eq!(val, ONE);
    }

    #[test]
    fn test_from_hex_sm2_p() {
        let val = U256::from_hex(SM2_P).unwrap();
        let hex_out = val.to_hex();
        assert_eq!(hex_out, SM2_P);
    }

    #[test]
    fn test_from_hex_sm2_n() {
        let val = U256::from_hex(SM2_N).unwrap();
        let hex_out = val.to_hex();
        assert_eq!(hex_out, SM2_N);
    }

    #[test]
    fn test_from_hex_lowercase() {
        let val =
            U256::from_hex("000000000000000000000000000000000000000000000000000000000000000a")
                .unwrap();
        assert_eq!(val.limbs[0], 10);
    }

    #[test]
    fn test_from_hex_wrong_length() {
        let result = U256::from_hex("12345");
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 64,
                actual: 5
            })
        );
    }

    #[test]
    fn test_from_hex_empty() {
        let result = U256::from_hex("");
        assert_eq!(
            result,
            Err(CryptoError::InvalidLength {
                expected: 64,
                actual: 0
            })
        );
    }

    #[test]
    fn test_from_hex_non_hex_char() {
        let result =
            U256::from_hex("00000000000000000000000000000000000000000000000000000000000000GG");
        assert!(matches!(result, Err(CryptoError::InvalidInput(_))));
    }

    #[test]
    fn test_to_hex_format() {
        assert_eq!(
            ZERO.to_hex(),
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            ONE.to_hex(),
            "0000000000000000000000000000000000000000000000000000000000000001"
        );
    }

    #[test]
    fn test_to_hex_roundtrip() {
        let hex = "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789";
        let val = U256::from_hex(hex).unwrap();
        assert_eq!(val.to_hex(), hex);
    }

    // ============================================================
    // SubTask 2.3: comparison and bit operation tests
    // ============================================================

    #[test]
    fn test_is_zero() {
        assert!(ZERO.is_zero());
        assert!(!ONE.is_zero());
        assert!(!U256::from_hex(SM2_P).unwrap().is_zero());
    }

    #[test]
    fn test_cmp_ordering() {
        let a = U256::from_hex("0000000000000000000000000000000000000000000000000000000000000005")
            .unwrap();
        let b = U256::from_hex("000000000000000000000000000000000000000000000000000000000000000A")
            .unwrap();
        assert_eq!(a.cmp_u256(&b), Ordering::Less);
        assert_eq!(b.cmp_u256(&a), Ordering::Greater);
        assert_eq!(a.cmp_u256(&a), Ordering::Equal);
    }

    #[test]
    fn test_partial_ord_operators() {
        let a = U256::from_hex("0000000000000000000000000000000000000000000000000000000000000005")
            .unwrap();
        let b = U256::from_hex("000000000000000000000000000000000000000000000000000000000000000A")
            .unwrap();
        assert!(a < b);
        assert!(b > a);
        assert!(a <= a);
        assert!(a >= a);
        assert!(a != b);
        assert!(a == a);
    }

    #[test]
    fn test_cmp_high_limb_difference() {
        // 区别仅在高 limb
        let a = U256 {
            limbs: [0, 0, 0, 1],
        };
        let b = U256 {
            limbs: [u64::MAX, 0, 0, 0],
        };
        assert!(a > b); // 高 limb 1 > 0
    }

    #[test]
    fn test_bit_lsb() {
        let val = ONE; // bit 0 = 1
        assert!(val.bit(0));
        assert!(!val.bit(1));
    }

    #[test]
    fn test_bit_msb() {
        let val = U256 {
            limbs: [0, 0, 0, 0x8000000000000000],
        }; // bit 255 = 1
        assert!(val.bit(255));
        assert!(!val.bit(254));
    }

    #[test]
    fn test_bit_out_of_range() {
        let val = ONE;
        assert!(!val.bit(256));
        assert!(!val.bit(300));
        assert!(!val.bit(usize::MAX));
    }

    #[test]
    fn test_bit_various() {
        let val = U256 {
            limbs: [0b1010, 0, 0, 0],
        }; // bits 1 and 3 set
        assert!(!val.bit(0));
        assert!(val.bit(1));
        assert!(!val.bit(2));
        assert!(val.bit(3));
    }

    #[test]
    fn test_bit_len_zero() {
        assert_eq!(ZERO.bit_len(), 0);
    }

    #[test]
    fn test_bit_len_one() {
        assert_eq!(ONE.bit_len(), 1);
    }

    #[test]
    fn test_bit_len_64() {
        let val = U256 {
            limbs: [0, 1, 0, 0],
        }; // bit 64 = 1
        assert_eq!(val.bit_len(), 65);
    }

    #[test]
    fn test_bit_len_128() {
        let val = U256 {
            limbs: [0, 0, 1, 0],
        }; // bit 128 = 1
        assert_eq!(val.bit_len(), 129);
    }

    #[test]
    fn test_bit_len_256() {
        let val = U256 {
            limbs: [0, 0, 0, 0x8000000000000000],
        }; // bit 255 = 1
        assert_eq!(val.bit_len(), 256);
    }

    #[test]
    fn test_bit_len_max_value() {
        let val = U256 {
            limbs: [u64::MAX, u64::MAX, u64::MAX, u64::MAX],
        };
        assert_eq!(val.bit_len(), 256);
    }

    // ============================================================
    // SubTask 2.4: modular add/sub tests
    // ============================================================

    #[test]
    fn test_add_mod_basic() {
        // (3 + 4) mod 7 = 0
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let b = U256 {
            limbs: [4, 0, 0, 0],
        };
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        assert_eq!(a.add_mod(&b, &m), ZERO);
    }

    #[test]
    fn test_add_mod_no_wrap() {
        // (2 + 3) mod 10 = 5
        let a = U256 {
            limbs: [2, 0, 0, 0],
        };
        let b = U256 {
            limbs: [3, 0, 0, 0],
        };
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        let result = a.add_mod(&b, &m);
        assert_eq!(result.limbs[0], 5);
    }

    #[test]
    fn test_add_mod_wrap_around() {
        // (6 + 7) mod 10 = 3
        let a = U256 {
            limbs: [6, 0, 0, 0],
        };
        let b = U256 {
            limbs: [7, 0, 0, 0],
        };
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        let result = a.add_mod(&b, &m);
        assert_eq!(result.limbs[0], 3);
    }

    #[test]
    fn test_add_mod_with_carry() {
        // self, other 接近 u64::MAX，测试 limb 间进位
        // m = 2 * u64::MAX + 2 (略大于 self + other)
        let a = U256 {
            limbs: [u64::MAX, 0, 0, 0],
        };
        let b = U256 {
            limbs: [u64::MAX, 0, 0, 0],
        };
        let m = U256 {
            limbs: [1, 2, 0, 0],
        }; // 2 * 2^64 + 1
           // a + b = 2 * (2^64 - 1) = 2^65 - 2, limbs = [u64::MAX-1, 1, 0, 0]
           // mod m = [u64::MAX-1, 1, 0, 0] - [1, 2, 0, 0] = ?
           // 2^65 - 2 mod (2^65 + 1) = 2^65 - 2 (since 2^65 - 2 < 2^65 + 1)
        let result = a.add_mod(&b, &m);
        assert_eq!(result.limbs[0], u64::MAX - 1);
        assert_eq!(result.limbs[1], 1);
    }

    #[test]
    fn test_add_mod_carry_256bit() {
        // 测试 256 位溢出情况：m = 2^256 - 1
        let m = U256 {
            limbs: [u64::MAX, u64::MAX, u64::MAX, u64::MAX],
        }; // 2^256 - 1
           // a = 2^255 - 1 (top limb = 2^63 - 1, others = MAX)
        let a = U256 {
            limbs: [u64::MAX, u64::MAX, u64::MAX, u64::MAX >> 1],
        };
        let b = a;
        // a = 2^255 - 1, a + a = 2^256 - 2
        // (2^256 - 2) mod (2^256 - 1) = 2^256 - 2 = m - 1
        // m - 1 = [MAX-1, MAX, MAX, MAX]
        let result = a.add_mod(&b, &m);
        let expected = U256 {
            limbs: [u64::MAX - 1, u64::MAX, u64::MAX, u64::MAX],
        };
        assert_eq!(result, expected);
    }

    #[test]
    fn test_sub_mod_basic() {
        // (5 - 3) mod 10 = 2
        let a = U256 {
            limbs: [5, 0, 0, 0],
        };
        let b = U256 {
            limbs: [3, 0, 0, 0],
        };
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        let result = a.sub_mod(&b, &m);
        assert_eq!(result.limbs[0], 2);
    }

    #[test]
    fn test_sub_mod_self_less_than_other() {
        // (3 - 5) mod 10 = 8
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let b = U256 {
            limbs: [5, 0, 0, 0],
        };
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        let result = a.sub_mod(&b, &m);
        assert_eq!(result.limbs[0], 8);
    }

    #[test]
    fn test_sub_mod_equal() {
        // (5 - 5) mod 10 = 0
        let a = U256 {
            limbs: [5, 0, 0, 0],
        };
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        let result = a.sub_mod(&a, &m);
        assert_eq!(result, ZERO);
    }

    #[test]
    fn test_sub_mod_sm2_p() {
        let p = U256::from_hex(SM2_P).unwrap();
        let a = U256::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
        // (p - 1) mod p = p - 1
        let result = p.sub_mod(&a, &p);
        let expected =
            U256::from_hex("FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFE")
                .unwrap();
        assert_eq!(result, expected);
    }

    // ============================================================
    // SubTask 2.5: modular multiplication tests
    // ============================================================

    #[test]
    fn test_mul_mod_basic() {
        // (3 * 4) mod 5 = 2
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let b = U256 {
            limbs: [4, 0, 0, 0],
        };
        let m = U256 {
            limbs: [5, 0, 0, 0],
        };
        let result = a.mul_mod(&b, &m);
        assert_eq!(result.limbs[0], 2);
    }

    #[test]
    fn test_mul_mod_zero() {
        let a = ZERO;
        let b = U256 {
            limbs: [42, 0, 0, 0],
        };
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        assert_eq!(a.mul_mod(&b, &m), ZERO);
    }

    #[test]
    fn test_mul_mod_one() {
        let a = ONE;
        let b = U256 {
            limbs: [42, 0, 0, 0],
        };
        let m = U256 {
            limbs: [100, 0, 0, 0],
        };
        let result = a.mul_mod(&b, &m);
        assert_eq!(result.limbs[0], 42);
    }

    #[test]
    fn test_mul_mod_large_values() {
        // 大值模乘: (u64::MAX * u64::MAX) mod (u64::MAX + 1)
        // = (2^128 - 2^65 + 1) mod 2^64 = 1
        let a = U256 {
            limbs: [u64::MAX, 0, 0, 0],
        };
        let b = U256 {
            limbs: [u64::MAX, 0, 0, 0],
        };
        let m = U256 {
            limbs: [0, 1, 0, 0],
        }; // 2^64
        let result = a.mul_mod(&b, &m);
        assert_eq!(result.limbs[0], 1);
    }

    #[test]
    fn test_mul_mod_sm2_p() {
        // 使用 SM2 P 测试大素数模乘
        let p = U256::from_hex(SM2_P).unwrap();
        // (p-1) * (p-1) mod p = 1 (费马小定理: a^(p-1) = 1, 但这里测 a^2 mod p)
        // (p-1)^2 = p^2 - 2p + 1, mod p = 1
        let a = p.sub_mod(&ONE, &p); // p - 1
        let result = a.mul_mod(&a, &p);
        assert_eq!(result, ONE);
    }

    #[test]
    fn test_mul_mod_sm2_p_large() {
        let p = U256::from_hex(SM2_P).unwrap();
        let a = U256::from_hex("00000000000000000000000000000000000000000000FFFFFFFFFFFFFFFFFFFF")
            .unwrap();
        let b = U256::from_hex("00000000000000000000000000000000000000000000FFFFFFFFFFFFFFFFFFFF")
            .unwrap();
        // 手动计算 a * b mod p（信任 mul_mod 实现，做一致性检查）
        let r1 = a.mul_mod(&b, &p);
        let r2 = b.mul_mod(&a, &p); // 交换律
        assert_eq!(r1, r2);
        // r1 应该 < p
        assert!(r1 < p);
    }

    // ============================================================
    // SubTask 2.6: modular inverse tests
    // ============================================================

    #[test]
    fn test_inv_mod_basic() {
        // 3^(-1) mod 7 = 5 (因为 3*5 = 15 = 2*7+1)
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        let inv = a.inv_mod(&m).unwrap();
        assert_eq!(inv.limbs[0], 5);
        // 验证: a * inv mod m == 1
        assert_eq!(a.mul_mod(&inv, &m), ONE);
    }

    #[test]
    fn test_inv_mod_one() {
        // 1^(-1) mod m = 1
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        let inv = ONE.inv_mod(&m).unwrap();
        assert_eq!(inv, ONE);
    }

    #[test]
    fn test_inv_mod_sm2_n() {
        // 使用 SM2 N（素数）测试模逆
        let n = U256::from_hex(SM2_N).unwrap();
        let a = U256::from_hex("0000000000000000000000000000000000000000000000000000000000000002")
            .unwrap();
        let inv = a.inv_mod(&n).unwrap();
        // 验证: a * inv mod n == 1
        let product = a.mul_mod(&inv, &n);
        assert_eq!(product, ONE);
    }

    #[test]
    fn test_inv_mod_sm2_n_large() {
        let n = U256::from_hex(SM2_N).unwrap();
        let a = U256::from_hex("1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF")
            .unwrap();
        let inv = a.inv_mod(&n).unwrap();
        // 验证: a * inv mod n == 1
        let product = a.mul_mod(&inv, &n);
        assert_eq!(product, ONE);
    }

    #[test]
    fn test_inv_mod_sm2_p() {
        // 使用 SM2 P（素数）测试模逆
        let p = U256::from_hex(SM2_P).unwrap();
        let a = U256::from_hex("0000000000000000000000000000000000000000000000000000000000000003")
            .unwrap();
        let inv = a.inv_mod(&p).unwrap();
        // 验证: a * inv mod p == 1
        let product = a.mul_mod(&inv, &p);
        assert_eq!(product, ONE);
    }

    #[test]
    fn test_inv_mod_fails_on_zero() {
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        let result = ZERO.inv_mod(&m);
        assert_eq!(result, Err(CryptoError::ModInverseFailed));
    }

    #[test]
    fn test_inv_mod_fails_on_zero_modulus() {
        let a = U256 {
            limbs: [3, 0, 0, 0],
        };
        let result = a.inv_mod(&ZERO);
        assert_eq!(result, Err(CryptoError::ModInverseFailed));
    }

    #[test]
    fn test_inv_mod_fails_on_non_coprime() {
        // gcd(6, 9) = 3 != 1
        let a = U256 {
            limbs: [6, 0, 0, 0],
        };
        let m = U256 {
            limbs: [9, 0, 0, 0],
        };
        let result = a.inv_mod(&m);
        assert_eq!(result, Err(CryptoError::ModInverseFailed));
    }

    #[test]
    fn test_inv_mod_fails_on_even_even() {
        // gcd(4, 6) = 2 != 1
        let a = U256 {
            limbs: [4, 0, 0, 0],
        };
        let m = U256 {
            limbs: [6, 0, 0, 0],
        };
        let result = a.inv_mod(&m);
        assert_eq!(result, Err(CryptoError::ModInverseFailed));
    }

    #[test]
    fn test_inv_mod_self_inverse() {
        // (p-1)^(-1) mod p = p-1 (因 (p-1)^2 = p^2-2p+1 ≡ 1 mod p)
        let p = U256 {
            limbs: [7, 0, 0, 0],
        };
        let a = p.sub_mod(&ONE, &p); // p - 1 = 6
        let inv = a.inv_mod(&p).unwrap();
        assert_eq!(inv, a); // 6^(-1) mod 7 = 6
        assert_eq!(a.mul_mod(&inv, &p), ONE);
    }

    #[test]
    fn test_inv_mod_reduce_first() {
        // self >= m: 先归约再求逆
        // 10 mod 7 = 3, 3^(-1) mod 7 = 5
        let a = U256 {
            limbs: [10, 0, 0, 0],
        };
        let m = U256 {
            limbs: [7, 0, 0, 0],
        };
        let inv = a.inv_mod(&m).unwrap();
        assert_eq!(inv.limbs[0], 5);
    }

    // ============================================================
    // Edge case tests
    // ============================================================

    #[test]
    fn test_edge_zero_add() {
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        assert_eq!(ZERO.add_mod(&ONE, &m), ONE);
        assert_eq!(ONE.add_mod(&ZERO, &m), ONE);
    }

    #[test]
    fn test_edge_zero_mul() {
        let m = U256 {
            limbs: [10, 0, 0, 0],
        };
        assert_eq!(ZERO.mul_mod(&ONE, &m), ZERO);
        assert_eq!(ONE.mul_mod(&ZERO, &m), ZERO);
    }

    #[test]
    fn test_edge_max_value_cmp() {
        let max = U256 {
            limbs: [u64::MAX, u64::MAX, u64::MAX, u64::MAX],
        };
        assert_eq!(max.cmp_u256(&max), Ordering::Equal);
        assert!(max > ZERO);
        assert!(max > ONE);
        assert_eq!(max.bit_len(), 256);
    }

    #[test]
    fn test_zeroize() {
        let mut val = U256::from_hex(SM2_P).unwrap();
        val.zeroize();
        assert!(val.is_zero());
    }

    #[test]
    fn test_div_rem_basic() {
        // 17 / 5 = 3 rem 2
        let a = U256 {
            limbs: [17, 0, 0, 0],
        };
        let b = U256 {
            limbs: [5, 0, 0, 0],
        };
        let (q, r) = a.div_rem(&b);
        assert_eq!(q.limbs[0], 3);
        assert_eq!(r.limbs[0], 2);
    }

    #[test]
    fn test_div_rem_exact() {
        // 20 / 5 = 4 rem 0
        let a = U256 {
            limbs: [20, 0, 0, 0],
        };
        let b = U256 {
            limbs: [5, 0, 0, 0],
        };
        let (q, r) = a.div_rem(&b);
        assert_eq!(q.limbs[0], 4);
        assert!(r.is_zero());
    }

    #[test]
    fn test_div_rem_large() {
        let n = U256::from_hex(SM2_N).unwrap();
        let two = U256 {
            limbs: [2, 0, 0, 0],
        };
        let (q, r) = n.div_rem(&two);
        // n is odd, so r = 1
        assert_eq!(r, ONE);
        // q = (n - 1) / 2
        let n_minus_1 = n.sub_mod(&ONE, &n);
        let expected_q = n_minus_1.div_rem(&two).0;
        assert_eq!(q, expected_q);
    }

    #[test]
    fn test_shl1() {
        let val = U256 {
            limbs: [1, 0, 0, 0],
        };
        let shifted = val.shl1();
        assert_eq!(shifted.limbs[0], 2);

        // 跨 limb 进位
        let val2 = U256 {
            limbs: [0, u64::MAX, 0, 0],
        };
        let shifted2 = val2.shl1();
        assert_eq!(shifted2.limbs[0], 0);
        assert_eq!(shifted2.limbs[1], u64::MAX - 1);
        assert_eq!(shifted2.limbs[2], 1);
    }

    #[test]
    fn test_mul_full_consistency() {
        // mul_full 的一致性：a * b = b * a
        let a = U256::from_hex("1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF")
            .unwrap();
        let b = U256::from_hex("FEDCBA0987654321FEDCBA0987654321FEDCBA0987654321FEDCBA0987654321")
            .unwrap();
        let prod1 = a.mul_full(&b);
        let prod2 = b.mul_full(&a);
        assert_eq!(prod1, prod2);
    }

    #[test]
    fn test_add_mod_associativity() {
        // (a + b + c) mod m == ((a + b) mod m + c) mod m
        let m = U256 {
            limbs: [97, 0, 0, 0],
        };
        let a = U256 {
            limbs: [50, 0, 0, 0],
        };
        let b = U256 {
            limbs: [60, 0, 0, 0],
        };
        let c = U256 {
            limbs: [70, 0, 0, 0],
        };

        let left = a.add_mod(&b, &m).add_mod(&c, &m);
        let right = b.add_mod(&c, &m).add_mod(&a, &m);
        assert_eq!(left, right);
    }

    #[test]
    fn test_mul_mod_distributivity() {
        // a * (b + c) mod m == (a*b + a*c) mod m
        let m = U256 {
            limbs: [97, 0, 0, 0],
        };
        let a = U256 {
            limbs: [50, 0, 0, 0],
        };
        let b = U256 {
            limbs: [60, 0, 0, 0],
        };
        let c = U256 {
            limbs: [70, 0, 0, 0],
        };

        let left = a.mul_mod(&b.add_mod(&c, &m), &m);
        let right = a.mul_mod(&b, &m).add_mod(&a.mul_mod(&c, &m), &m);
        assert_eq!(left, right);
    }
}
