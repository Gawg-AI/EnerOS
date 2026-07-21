//! 恒定时间工具.
//!
//! 提供抗侧信道攻击的恒定时间操作：
//! - `ct_eq`: 恒定时间字节比较（无论输入如何，比较时间一致）
//! - `ct_zeroize`: 恒定时间清零（防编译器优化删除清零操作）
//!
//! # 安全说明
//! 恒定时间实现是密码学库的核心安全特性，用于防止时序侧信道攻击。
//! 所有密钥/tag/签名比较必须使用 `ct_eq`，所有敏感数据清零必须使用 `ct_zeroize`。

use core::sync::atomic::{compiler_fence, Ordering};

/// 恒定时间字节比较.
///
/// 无论输入内容如何，比较时间仅取决于输入长度。
/// 长度不同的输入立即返回 false（长度信息不视为敏感）。
///
/// # 示例
/// ```
/// use eneros_crypto::constant_time::ct_eq;
/// assert!(ct_eq(b"hello", b"hello"));
/// assert!(!ct_eq(b"hello", b"world"));
/// assert!(!ct_eq(b"hello", b"hi"));
/// ```
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 恒定时间清零.
///
/// 使用 `write_volatile` 防止编译器优化删除清零操作，
/// 并在清零后插入编译器屏障确保顺序。
///
/// # 安全说明
/// 用于清零密钥、随机数种子等敏感数据，防止内存残留被攻击者读取。
///
/// # 示例
/// ```
/// use eneros_crypto::constant_time::ct_zeroize;
/// let mut key = [0x42u8; 32];
/// ct_zeroize(&mut key);
/// assert!(key.iter().all(|&b| b == 0));
/// ```
pub fn ct_zeroize(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        // SAFETY: write_volatile 防止编译器优化删除写入
        unsafe {
            core::ptr::write_volatile(byte, 0);
        }
    }
    // 编译器屏障确保清零操作不会被重排到变量生命周期结束之后
    compiler_fence(Ordering::SeqCst);
}

/// 恒定时间选择.
///
/// 基于 `mask` (0x00 或 0xFF) 选择 `a` 或 `b`：
/// - mask = 0xFF → 返回 a
/// - mask = 0x00 → 返回 b
///
/// 用于条件赋值而不分支，防止分支预测侧信道。
#[allow(dead_code)]
pub(crate) fn ct_select(a: u8, b: u8, mask: u8) -> u8 {
    (a & mask) | (b & !mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ct_eq_equal() {
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"hello", b"hello"));
        assert!(ct_eq(&[0u8; 32], &[0u8; 32]));
        assert!(ct_eq(&[0xFFu8; 64], &[0xFFu8; 64]));
    }

    #[test]
    fn test_ct_eq_not_equal() {
        assert!(!ct_eq(b"hello", b"world"));
        assert!(!ct_eq(&[0u8; 32], &[1u8; 32]));
        assert!(!ct_eq(&[0u8; 32], &[0u8; 33])); // 长度不同
    }

    #[test]
    fn test_ct_eq_single_byte_diff() {
        let a = [0u8; 32];
        let mut b = [0u8; 32];
        b[31] = 1;
        assert!(!ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_zeroize() {
        let mut buf = [0x42u8; 32];
        ct_zeroize(&mut buf);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_ct_zeroize_various_sizes() {
        for size in [0, 1, 16, 32, 64, 256, 1024] {
            let mut buf = vec![0xABu8; size];
            ct_zeroize(&mut buf);
            assert!(buf.iter().all(|&b| b == 0), "size {} failed", size);
        }
    }

    #[test]
    fn test_ct_select() {
        assert_eq!(ct_select(0xAA, 0xBB, 0xFF), 0xAA);
        assert_eq!(ct_select(0xAA, 0xBB, 0x00), 0xBB);
    }
}
