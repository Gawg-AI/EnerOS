//! libcrypto FFI（薄封装）——全部 unsafe 集中在 C 帮助层
//! （`src/gmssl_helpers.c`，经 cc 编译链接）。
//!
//! 本模块只声明 gmssl_helpers 导出的 5 个 C 函数并做参数校验，
//! 不在 Rust 侧直接触碰 OpenSSL API（调用序列与内存管理全部在 C 层，
//! 消除 FFI 面过宽带来的 ABI 风险——2026-08-09 实测定位）。
//!
//! # unsafe 纪律
//! 每个 unsafe 块附 SAFETY 注释；C 函数契约见 gmssl_helpers.c 顶部注释。

use core::ffi::c_int;

extern "C" {
    /// 0 成功，-1 失败。`siglen` 入 = 缓冲容量，出 = DER 长度。
    fn gmssl_sm2_sign(
        d: *const u8,
        point: *const u8,
        msg: *const u8,
        msglen: usize,
        sig: *mut u8,
        siglen: *mut usize,
    ) -> c_int;
    /// 1 有效，0 无效，-1 错误。
    fn gmssl_sm2_verify(
        point: *const u8,
        msg: *const u8,
        msglen: usize,
        sig: *const u8,
        siglen: usize,
    ) -> c_int;
    /// 0 成功，-1 失败。
    fn gmssl_sm3(data: *const u8, len: usize, md: *mut u8) -> c_int;
    /// 0 成功，-1 失败。
    fn gmssl_sm4_ecb_block(key: *const u8, block: *const u8, encrypt: c_int, out: *mut u8)
        -> c_int;
}

/// SM2 签名：`message` + 私钥标量 `d` + 未压缩公钥点 → DER ECDSA-Sig-Value。
///
/// 公钥点必须与 `d` 匹配（SM2 keymgmt 不从 priv 自动派生 pub，Z 值
/// 计算需要公钥）。distid 固定为国标默认（C 层设置）。
pub fn sm2_sign(message: &[u8], d: &[u8; 32], public_point: &[u8; 65]) -> Result<Vec<u8>, String> {
    let mut sig = vec![0u8; 128];
    let mut siglen: usize = sig.len();
    // SAFETY:
    // - `d`/`public_point` 为定长数组，`message` 为切片（指针+长度有效）。
    // - `sig` 为 128 字节缓冲（SM2 DER 签名 ≤72 字节），容量经 `siglen` 传入，
    //   C 层只写至多该容量并回写实际长度。
    let rc = unsafe {
        gmssl_sm2_sign(
            d.as_ptr(),
            public_point.as_ptr(),
            message.as_ptr(),
            message.len(),
            sig.as_mut_ptr(),
            &mut siglen,
        )
    };
    if rc != 0 {
        return Err("gmssl_sm2_sign failed".into());
    }
    if siglen > sig.len() {
        return Err("gmssl_sm2_sign returned an out-of-bounds length".into());
    }
    sig.truncate(siglen);
    Ok(sig)
}

/// SM2 验签：`message` + 65 字节未压缩公钥点 + DER 签名 → 是否有效。
pub fn sm2_verify(message: &[u8], point: &[u8; 65], sig_der: &[u8]) -> Result<bool, String> {
    // SAFETY: `point` 为 65 字节数组，`message`/`sig_der` 为切片；
    // 返回码 1 = 有效，0 = 无效，负值 = 错误。
    let rc = unsafe {
        gmssl_sm2_verify(
            point.as_ptr(),
            message.as_ptr(),
            message.len(),
            sig_der.as_ptr(),
            sig_der.len(),
        )
    };
    match rc {
        1 => Ok(true),
        0 => Ok(false),
        other => Err(format!("gmssl_sm2_verify error: rc={other}")),
    }
}

/// SM3 摘要（GB/T 32905-2016）：`data` → 32 字节哈希。
pub fn sm3_digest(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // SAFETY: `data` 为切片；`out` 为 32 字节栈缓冲（SM3 摘要长度固定）。
    let rc = unsafe { gmssl_sm3(data.as_ptr(), data.len(), out.as_mut_ptr()) };
    assert!(rc == 0, "gmssl_sm3 failed: rc={rc}");
    out
}

/// SM4-ECB 单块加解密（16 字节块，无填充）。
pub fn sm4_ecb_block(key: &[u8; 16], block: &[u8; 16], encrypt: bool) -> Result<[u8; 16], String> {
    let mut out = [0u8; 16];
    // SAFETY: `key`/`block`/`out` 均为 16 字节数组；encrypt 1/0 由 bool 映射。
    let rc = unsafe {
        gmssl_sm4_ecb_block(
            key.as_ptr(),
            block.as_ptr(),
            if encrypt { 1 } else { 0 },
            out.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(format!("gmssl_sm4_ecb_block failed: rc={rc}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定测试密钥（openssl 生成并导出于 2026-08-09）。
    const D_TEST: [u8; 32] = [
        0x4c, 0x7f, 0xa9, 0xa1, 0x5d, 0xe9, 0xf2, 0x49, 0x0d, 0x84, 0x28, 0xe0, 0xb9, 0xbe, 0x30,
        0xd4, 0x2c, 0x88, 0xf5, 0x75, 0xea, 0x8e, 0xbb, 0x07, 0x2d, 0x12, 0x13, 0x8f, 0x03, 0x80,
        0x93, 0xab,
    ];
    /// 对应未压缩公钥点（04||X||Y）。
    const POINT_D_TEST: [u8; 65] = [
        0x04, 0x91, 0x15, 0xd8, 0x8a, 0xf5, 0x66, 0x6e, 0x02, 0xac, 0x28, 0x31, 0xd1, 0xf9, 0x2a,
        0xe4, 0x97, 0xb5, 0x3f, 0x62, 0x7c, 0xa6, 0x74, 0x8d, 0xb5, 0xb4, 0x9f, 0x49, 0x9d, 0x2b,
        0x3b, 0xd3, 0x60, 0xd7, 0x43, 0xab, 0xa1, 0xe8, 0xa8, 0xe5, 0x4c, 0x8e, 0x56, 0x73, 0x58,
        0xe3, 0x25, 0x3e, 0xbd, 0xe4, 0xd5, 0xca, 0x35, 0xbb, 0xdb, 0xf8, 0x76, 0xe4, 0x35, 0x70,
        0xa8, 0x12, 0x1a, 0xcf, 0xaf,
    ];

    #[test]
    fn sm3_known_vector() {
        // GB/T 32905-2016 附录 A.1：SM3("abc") = 66c7f0f4 62eeedd9 ...
        let hash = sm3_digest(b"abc");
        let expect: [u8; 32] = [
            0x66, 0xc7, 0xf0, 0xf4, 0x62, 0xee, 0xed, 0xd9, 0xd1, 0xf2, 0xd4, 0x6b, 0xdc, 0x10,
            0xe4, 0xe2, 0x41, 0x67, 0xc4, 0x87, 0x5c, 0xf2, 0xf7, 0xa2, 0x29, 0x7d, 0xa0, 0x2b,
            0x8f, 0x4b, 0xa8, 0xe0,
        ];
        assert_eq!(hash, expect);
    }

    #[test]
    fn sm4_ecb_round_trip() {
        let key = [0x01; 16];
        let block = [0xab; 16];
        let cipher = sm4_ecb_block(&key, &block, true).unwrap();
        assert_ne!(cipher, block);
        let plain = sm4_ecb_block(&key, &cipher, false).unwrap();
        assert_eq!(plain, block);
    }

    #[test]
    fn sm4_ecb_known_vector() {
        // GB/T 32907-2016 附录 A.1 首块。
        let key: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let block: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let cipher = sm4_ecb_block(&key, &block, true).unwrap();
        let expect: [u8; 16] = [
            0x68, 0x1e, 0xdf, 0x34, 0xd2, 0x06, 0x96, 0x5e, 0x86, 0xb3, 0xe9, 0x4f, 0x53, 0x6e,
            0x42, 0x46,
        ];
        assert_eq!(cipher, expect);
    }

    #[test]
    fn sm2_sign_verify_round_trip() {
        let message = b"eneros sm2 interop round trip";
        let sig = sm2_sign(message, &D_TEST, &POINT_D_TEST).unwrap();
        assert!(sig[0] == 0x30, "DER SEQUENCE prefix");
        assert!(sm2_verify(message, &POINT_D_TEST, &sig).unwrap());
        // 篡改消息 → 验签失败
        assert!(!sm2_verify(b"tampered message", &POINT_D_TEST, &sig).unwrap());
        // 篡改签名 → 验签失败
        let mut bad_sig = sig.clone();
        let last = bad_sig.len() - 1;
        bad_sig[last] ^= 0x01;
        assert!(!sm2_verify(message, &POINT_D_TEST, &bad_sig).unwrap());
        // DER → r||s → DER 往返一致
        let (r, s) = crate::der::der_to_rs(&sig).unwrap();
        let re = crate::der::rs_to_der(&r, &s).unwrap();
        assert_eq!(re, sig);
    }
}
