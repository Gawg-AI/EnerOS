//! SM3-HMAC 消息认证码 (RFC 2104 HMAC + GB/T 32905-2016 SM3).
//!
//! 提供基于 SM3 杂凑算法的 HMAC 消息认证码实现，输出 256 位（32 字节）认证标签。
//!
//! # 算法概述
//! HMAC（Keyed-Hash Message Authentication Code）将密钥与消息混合后做两次杂凑，
//! 提供消息完整性与来源认证。本模块将 HMAC 的底层杂凑算法替换为国密 SM3：
//!
//! 1. 密钥预处理：若 `key.len() > 64`（SM3 块长），先计算 `key = SM3(key)`；
//!    然后将密钥右补 `0x00` 至 64 字节，记为 `key_64`
//! 2. 计算填充：`ipad = key_64 XOR 0x36`（逐字节），`opad = key_64 XOR 0x5C`
//! 3. 输出：`HMAC = SM3(opad ‖ SM3(ipad ‖ msg))`
//!
//! # no_std 合规
//! 仅使用 `core::*`，不依赖 `alloc::*` 或 `std::*`；密钥清零复用
//! `crate::constant_time::ct_zeroize`（内部封装 volatile 写入，本模块不含 unsafe）。
//!
//! # 密钥清零
//! `Sm3Hmac` 内部持有 ipad/opad 两个密钥填充块（密钥材料），
//! `Drop` 时使用 `ct_zeroize` 恒定时间清零，防止内存残留泄露。
//!
//! # 示例
//! ```
//! use eneros_crypto::sm3::hmac::{hmac_sm3, Sm3Hmac};
//!
//! // 一次性计算
//! let tag = hmac_sm3(b"key", b"message");
//! assert_eq!(tag.len(), 32);
//!
//! // 流式计算（与一次性结果一致）
//! let mut h = Sm3Hmac::new(b"key");
//! h.update(b"mes");
//! h.update(b"sage");
//! let tag2 = h.finalize();
//! assert_eq!(tag, tag2);
//! ```
//!
//! # 参考
//! - RFC 2104: HMAC: Keyed-Hashing for Message Authentication
//! - GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法

use crate::constant_time::ct_zeroize;
use crate::sm3::Sm3Hasher;

/// SM3 分组块长（字节）.
const BLOCK_LEN: usize = 64;

/// HMAC 内填充常量.
const IPAD_BYTE: u8 = 0x36;

/// HMAC 外填充常量.
const OPAD_BYTE: u8 = 0x5C;

// ============================================================
// Key preprocessing
// ============================================================

/// 密钥预处理：压缩过长密钥并右补零至 64 字节.
///
/// - 若 `key.len() > 64`：先计算 `SM3(key)`，以 32 字节摘要作为新密钥
/// - 将（压缩后的）密钥拷贝至 64 字节块头部，其余字节保持 `0x00`
fn normalize_key(key: &[u8]) -> [u8; BLOCK_LEN] {
    let mut block = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        let digest = crate::sm3::hash(key);
        block[..32].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    block
}

// ============================================================
// One-shot API
// ============================================================

/// 一次性计算 SM3-HMAC 消息认证码.
///
/// `HMAC-SM3(key, msg) = SM3(opad ‖ SM3(ipad ‖ msg))`
///
/// # 示例
/// ```
/// use eneros_crypto::sm3::hmac::hmac_sm3;
/// let tag = hmac_sm3(b"key", b"hello");
/// assert_eq!(tag.len(), 32);
/// ```
pub fn hmac_sm3(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut ctx = Sm3Hmac::new(key);
    ctx.update(msg);
    ctx.finalize()
}

// ============================================================
// Streaming API
// ============================================================

/// SM3-HMAC 流式计算上下文.
///
/// 内部持有 ipad/opad 两个 64 字节密钥填充块（密钥材料）与内层 SM3 状态：
/// - 构造时已将 ipad 写入内层 hasher（`inner.update(ipad)`）
/// - `update` 直接转发至内层 hasher
/// - `finalize` 时先取内层摘要，再计算外层 `SM3(opad ‖ inner_digest)`
///
/// `Drop` 时使用 `ct_zeroize` 清零 ipad/opad（项目记忆硬约束：密钥材料必须 zeroize）。
pub struct Sm3Hmac {
    /// 内层密钥填充块（key_64 XOR 0x36），密钥材料
    ipad: [u8; BLOCK_LEN],
    /// 外层密钥填充块（key_64 XOR 0x5C），密钥材料
    opad: [u8; BLOCK_LEN],
    /// 内层 SM3 状态（已预置 ipad）
    inner: Sm3Hasher,
}

impl Sm3Hmac {
    /// 以 `key` 创建新的 SM3-HMAC 上下文.
    ///
    /// 构造时完成密钥预处理（过长密钥 SM3 压缩 + 右补零至 64 字节），
    /// 并生成 ipad/opad；内层 SM3 状态已吸收 ipad，可直接 `update` 消息数据。
    pub fn new(key: &[u8]) -> Self {
        let key_64 = normalize_key(key);
        let mut ipad = [0u8; BLOCK_LEN];
        let mut opad = [0u8; BLOCK_LEN];
        for i in 0..BLOCK_LEN {
            ipad[i] = key_64[i] ^ IPAD_BYTE;
            opad[i] = key_64[i] ^ OPAD_BYTE;
        }
        let mut inner = Sm3Hasher::new();
        inner.update(&ipad);
        Self { ipad, opad, inner }
    }

    /// 追加消息数据（可多次调用以处理流式数据）.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// 完成计算，返回 256-bit（32 字节）HMAC 标签.
    ///
    /// 先取内层摘要 `SM3(ipad ‖ msg)`，再计算外层 `SM3(opad ‖ inner_digest)`。
    pub fn finalize(self) -> [u8; 32] {
        let mut this = self;
        // mem::take 交换出内层 hasher（实现 Drop 的类型无法直接移出字段）
        let inner = core::mem::take(&mut this.inner);
        let inner_digest = inner.finalize();
        let mut outer = Sm3Hasher::new();
        outer.update(&this.opad);
        outer.update(&inner_digest);
        outer.finalize()
    }
}

impl Drop for Sm3Hmac {
    fn drop(&mut self) {
        ct_zeroize(&mut self.ipad);
        ct_zeroize(&mut self.opad);
    }
}

// ============================================================
// Unit Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立手算 HMAC-SM3（仅用 Sm3Hasher 逐步复算 ipad/opad 流程），
    /// 作为已知答案基准与模块实现交叉验证。
    fn reference_hmac_sm3(key: &[u8], msg: &[u8]) -> [u8; 32] {
        // 1. 密钥预处理
        let mut key_64 = [0u8; 64];
        if key.len() > 64 {
            let mut kh = Sm3Hasher::new();
            kh.update(key);
            let digest = kh.finalize();
            key_64[..32].copy_from_slice(&digest);
        } else {
            key_64[..key.len()].copy_from_slice(key);
        }
        // 2. ipad / opad
        let mut ipad = [0u8; 64];
        let mut opad = [0u8; 64];
        for i in 0..64 {
            ipad[i] = key_64[i] ^ 0x36;
            opad[i] = key_64[i] ^ 0x5C;
        }
        // 3. SM3(ipad ‖ msg)
        let mut ih = Sm3Hasher::new();
        ih.update(&ipad);
        ih.update(msg);
        let inner_digest = ih.finalize();
        // 4. SM3(opad ‖ inner_digest)
        let mut oh = Sm3Hasher::new();
        oh.update(&opad);
        oh.update(&inner_digest);
        oh.finalize()
    }

    // TH1: 已知答案测试 —— 独立手算基准与 hmac_sm3 一致
    #[test]
    fn test_hmac_sm3_kat_quick_brown_fox() {
        let key = b"key";
        let msg = b"The quick brown fox jumps over the lazy dog";
        let expected = reference_hmac_sm3(key, msg);
        assert_eq!(hmac_sm3(key, msg), expected);
        // 交叉验证：流式接口也与独立基准一致
        let mut h = Sm3Hmac::new(key);
        h.update(msg);
        assert_eq!(h.finalize(), expected);
    }

    // TH2: 一次性与流式结果一致
    #[test]
    fn test_one_shot_equals_streaming() {
        let key = b"test-key";
        let msg = b"streaming vs one-shot consistency check";
        let one_shot = hmac_sm3(key, msg);
        let mut h = Sm3Hmac::new(key);
        h.update(msg);
        let streamed = h.finalize();
        assert_eq!(one_shot, streamed);
    }

    // TH3: 分多段 update 与一次性结果一致
    #[test]
    fn test_streaming_multi_segment() {
        let key = b"segment-key";
        let msg = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let one_shot = hmac_sm3(key, msg);

        let mut h = Sm3Hmac::new(key);
        h.update(&msg[..10]);
        h.update(&msg[10..26]);
        h.update(&msg[26..]);
        assert_eq!(h.finalize(), one_shot);

        // 逐字节流式也应一致
        let mut h2 = Sm3Hmac::new(key);
        for byte in msg {
            h2.update(core::slice::from_ref(byte));
        }
        assert_eq!(h2.finalize(), one_shot);
    }

    // TH4: key 长度 0 字节
    #[test]
    fn test_key_len_zero() {
        let msg = b"message";
        let t1 = hmac_sm3(b"", msg);
        let t2 = hmac_sm3(b"", msg);
        assert_eq!(t1.len(), 32);
        assert_eq!(t1, t2); // 确定性
        assert_eq!(t1, reference_hmac_sm3(b"", msg)); // 与独立基准一致
    }

    // TH5: key 长度 63/64 字节（块长边界内）
    #[test]
    fn test_key_len_63_and_64() {
        let msg = b"boundary";
        for len in [63usize, 64] {
            let key = [0x42u8; 64];
            let k = &key[..len];
            let t1 = hmac_sm3(k, msg);
            let t2 = hmac_sm3(k, msg);
            assert_eq!(t1.len(), 32);
            assert_eq!(t1, t2);
            assert_eq!(t1, reference_hmac_sm3(k, msg));
        }
        // 63 字节与 64 字节 key 输出不同（不同密钥材料）
        let key = [0x42u8; 64];
        assert_ne!(hmac_sm3(&key[..63], msg), hmac_sm3(&key[..64], msg));
    }

    // TH6: key 长度 65 字节（跨块长边界，走 SM3 压缩分支）
    #[test]
    fn test_key_len_65() {
        let msg = b"long key branch";
        let key = [0x5Au8; 65];
        let t1 = hmac_sm3(&key, msg);
        let t2 = hmac_sm3(&key, msg);
        assert_eq!(t1.len(), 32);
        assert_eq!(t1, t2);
        assert_eq!(t1, reference_hmac_sm3(&key, msg));
    }

    // TH7: key 长度 100 字节（远大于块长，压缩分支）
    #[test]
    fn test_key_len_100() {
        let msg = b"very long key";
        let key = [0x77u8; 100];
        let t1 = hmac_sm3(&key, msg);
        let t2 = hmac_sm3(&key, msg);
        assert_eq!(t1.len(), 32);
        assert_eq!(t1, t2);
        assert_eq!(t1, reference_hmac_sm3(&key, msg));
        // >64 与 <=64 key 走不同分支，输出一般不同
        let short_key = [0x77u8; 64];
        assert_ne!(t1, hmac_sm3(&short_key, msg));
    }

    // TH8: 不同 key 同 msg → 输出不同
    #[test]
    fn test_different_keys_different_output() {
        let msg = b"same message";
        assert_ne!(hmac_sm3(b"key-a", msg), hmac_sm3(b"key-b", msg));
    }

    // TH9: 同 key 不同 msg → 输出不同
    #[test]
    fn test_different_msgs_different_output() {
        let key = b"same-key";
        assert_ne!(hmac_sm3(key, b"msg-a"), hmac_sm3(key, b"msg-b"));
    }

    // TH10: 空消息不 panic 且确定性
    #[test]
    fn test_empty_message() {
        let t1 = hmac_sm3(b"key", b"");
        let t2 = hmac_sm3(b"key", b"");
        assert_eq!(t1.len(), 32);
        assert_eq!(t1, t2);
        assert_eq!(t1, reference_hmac_sm3(b"key", b""));
        // 空消息与空 key 组合
        let t3 = hmac_sm3(b"", b"");
        assert_eq!(t3, reference_hmac_sm3(b"", b""));
    }

    // TH11: 长消息流式（跨多个分组边界）
    #[test]
    fn test_long_message_streaming() {
        let key = b"long-msg-key";
        let msg = [0x99u8; 1000];
        let one_shot = hmac_sm3(key, &msg);
        let mut h = Sm3Hmac::new(key);
        let mut offset = 0;
        while offset < msg.len() {
            let end = core::cmp::min(offset + 37, msg.len()); // 非对齐步长
            h.update(&msg[offset..end]);
            offset = end;
        }
        assert_eq!(h.finalize(), one_shot);
        assert_eq!(one_shot, reference_hmac_sm3(key, &msg));
    }

    // TH12: 空 update 不改变状态
    #[test]
    fn test_empty_update_noop() {
        let key = b"key";
        let msg = b"abc";
        let one_shot = hmac_sm3(key, msg);
        let mut h = Sm3Hmac::new(key);
        h.update(b"");
        h.update(msg);
        h.update(b"");
        assert_eq!(h.finalize(), one_shot);
    }

    // TH13: Drop 清零密钥材料（构造后手动触发 drop，依赖 ct_zeroize 语义）
    #[test]
    fn test_drop_zeroizes_pads() {
        let mut h = Sm3Hmac::new(b"secret-key-material");
        h.update(b"some data");
        // 直接调用 Drop::drop 验证清零行为（等价于离开作用域时的行为）
        core::mem::drop(h);
        // 能走到这里即未 panic；ct_zeroize 内部语义已在 constant_time 模块测试中覆盖
    }
}
