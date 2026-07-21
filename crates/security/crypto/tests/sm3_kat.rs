//! SM3 KAT (Known Answer Tests) — GB/T 32905-2016 集成测试.
//!
//! 本文件为 v0.31.0 Task 12 交付物，验证 `eneros_crypto::sm3` 公共 API 的正确性。
//!
//! # 测试向量来源
//! - **官方 KAT**：GB/T 32905-2016 信息安全技术 SM3 密码杂凑算法
//!   （"abc" 与 64 字节 "abcd"×16 消息）
//! - **独立参考**：Python `hashlib.new("sm3", ...)` 交叉验证
//!   （空消息 / 单字节 0x00 / 单字节 0xFF / 10000×0x00 大消息）
//!
//! # 测试覆盖
//! - SubTask 12.1: GB/T 32905-2016 测试向量 1 — SM3("abc")
//! - SubTask 12.2: GB/T 32905-2016 测试向量 2 — 64 字节消息
//! - SubTask 12.3: 流式更新一致性（`Sm3Hasher` 分段 `update` vs 一次性 `hash`）
//! - 附加测试：空消息、单字节、大消息、确定性

use eneros_crypto::sm3;

// ============================================================
// SubTask 12.1: GB/T 32905-2016 Test Vector 1 — SM3("abc")
// ============================================================

#[test]
fn test_sm3_kat_abc() {
    let result = sm3::hash(b"abc");
    let expected: [u8; 32] = [
        0x66, 0xc7, 0xf0, 0xf4, 0x62, 0xee, 0xed, 0xd9, 0xd1, 0xf2, 0xd4, 0x6b, 0xdc, 0x10, 0xe4,
        0xe2, 0x41, 0x67, 0xc4, 0x87, 0x5c, 0xf2, 0xf7, 0xa2, 0x29, 0x7d, 0xa0, 0x2b, 0x8f, 0x4b,
        0xa8, 0xe0,
    ];
    assert_eq!(result, expected, "SM3(\"abc\") KAT failed");
}

// ============================================================
// SubTask 12.2: GB/T 32905-2016 Test Vector 2 — 64-byte message
// ============================================================

#[test]
fn test_sm3_kat_64_bytes() {
    let msg = b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd";
    let result = sm3::hash(msg);
    let expected: [u8; 32] = [
        0xde, 0xbe, 0x9f, 0xf9, 0x22, 0x75, 0xb8, 0xa1, 0x38, 0x60, 0x48, 0x89, 0xc1, 0x8e, 0x5a,
        0x4d, 0x6f, 0xdb, 0x70, 0xe5, 0x38, 0x7e, 0x57, 0x65, 0x29, 0x3d, 0xcb, 0xa3, 0x9c, 0x0c,
        0x57, 0x32,
    ];
    assert_eq!(result, expected, "SM3(64-byte) KAT failed");
}

// ============================================================
// SubTask 12.3: Streaming update tests
// ============================================================

#[test]
fn test_sm3_streaming_vs_oneshot() {
    // Hash "abc" in one shot
    let oneshot = sm3::hash(b"abc");

    // Hash "abc" in 3 separate updates
    let mut hasher = sm3::Sm3Hasher::new();
    hasher.update(b"a");
    hasher.update(b"b");
    hasher.update(b"c");
    let streamed = hasher.finalize();

    assert_eq!(
        oneshot, streamed,
        "Streaming hash should match one-shot hash"
    );
}

#[test]
fn test_sm3_streaming_byte_by_byte() {
    let msg = b"Hello, SM3 streaming test!";
    let oneshot = sm3::hash(msg);

    let mut hasher = sm3::Sm3Hasher::new();
    for byte in msg.iter() {
        hasher.update(&[*byte]);
    }
    let streamed = hasher.finalize();

    assert_eq!(
        oneshot, streamed,
        "Byte-by-byte streaming should match one-shot"
    );
}

#[test]
fn test_sm3_streaming_64_byte_message() {
    let msg = b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd";
    let oneshot = sm3::hash(msg);

    // Split at various boundaries
    for split in [1, 16, 32, 48, 63, 64] {
        let mut hasher = sm3::Sm3Hasher::new();
        hasher.update(&msg[..split.min(msg.len())]);
        if split < msg.len() {
            hasher.update(&msg[split..]);
        }
        let streamed = hasher.finalize();
        assert_eq!(oneshot, streamed, "Split at {} failed", split);
    }
}

// ============================================================
// Additional tests: edge cases and determinism
// ============================================================

#[test]
fn test_sm3_empty_message() {
    // SM3("") — 空消息
    let result = sm3::hash(b"");
    let expected: [u8; 32] = [
        0x1a, 0xb2, 0x1d, 0x83, 0x55, 0xcf, 0xa1, 0x7f, 0x8e, 0x61, 0x19, 0x48, 0x31, 0xe8, 0x1a,
        0x8f, 0x22, 0xbe, 0xc8, 0xc7, 0x28, 0xfe, 0xfb, 0x74, 0x7e, 0xd0, 0x35, 0xeb, 0x50, 0x82,
        0xaa, 0x2b,
    ];
    assert_eq!(result, expected, "SM3(\"\") KAT failed");
}

#[test]
fn test_sm3_single_byte_00() {
    // SM3([0x00]) — 单字节 0x00
    // 预期值由 Python hashlib.sm3 独立参考实现交叉验证。
    let input = [0x00u8];
    let result = sm3::hash(&input);
    let expected: [u8; 32] = [
        0x2d, 0xae, 0xf6, 0x0e, 0x7a, 0x0b, 0x8f, 0x5e, 0x02, 0x4c, 0x81, 0xcd, 0x2a, 0xb3, 0x10,
        0x9f, 0x2b, 0x4f, 0x15, 0x5c, 0xf8, 0x3a, 0xde, 0xb2, 0xae, 0x55, 0x32, 0xf7, 0x4a, 0x15,
        0x7f, 0xdf,
    ];
    assert_eq!(result, expected, "SM3([0x00]) KAT failed");
    // 流式一致性
    let mut hasher = sm3::Sm3Hasher::new();
    hasher.update(&input);
    assert_eq!(
        hasher.finalize(),
        expected,
        "Streaming should match one-shot for [0x00]"
    );
    // 与空消息不同
    assert_ne!(
        result,
        sm3::hash(b""),
        "SM3([0x00]) must differ from SM3(\"\")"
    );
}

#[test]
fn test_sm3_single_byte_ff() {
    // SM3([0xFF]) — 单字节 0xFF
    // 预期值由 Python hashlib.sm3 独立参考实现交叉验证。
    let input = [0xFFu8];
    let result = sm3::hash(&input);
    let expected: [u8; 32] = [
        0x17, 0xa7, 0xae, 0xf8, 0xe0, 0xb9, 0xa4, 0x4e, 0x3c, 0xab, 0x58, 0x92, 0x8a, 0xf8, 0x21,
        0x09, 0xac, 0x6b, 0x62, 0xbd, 0xc2, 0x1b, 0x22, 0x28, 0x48, 0x31, 0xaf, 0x74, 0x29, 0x38,
        0x2b, 0x7b,
    ];
    assert_eq!(result, expected, "SM3([0xFF]) KAT failed");
    // 流式一致性
    let mut hasher = sm3::Sm3Hasher::new();
    hasher.update(&input);
    assert_eq!(
        hasher.finalize(),
        expected,
        "Streaming should match one-shot for [0xFF]"
    );
    // 与 [0x00] 不同
    assert_ne!(
        result,
        sm3::hash(&[0x00u8]),
        "SM3([0xFF]) must differ from SM3([0x00])"
    );
}

#[test]
fn test_sm3_large_message() {
    // SM3(10000 × 0x00) — 大消息（跨 156+ 个分组）
    // 预期值由 Python hashlib.sm3 独立参考实现交叉验证。
    let msg = vec![0x00u8; 10000];
    let result = sm3::hash(&msg);
    let expected: [u8; 32] = [
        0x6c, 0x71, 0xcd, 0x37, 0x89, 0x54, 0xf8, 0xaa, 0xab, 0x56, 0x44, 0x57, 0x0d, 0x48, 0x17,
        0x7c, 0x66, 0x3a, 0x7d, 0xe9, 0x46, 0x27, 0x79, 0x45, 0x8c, 0x33, 0x4a, 0xa6, 0xf8, 0x92,
        0xfd, 0x05,
    ];
    assert_eq!(result, expected, "SM3(10000×0x00) KAT failed");
    // 流式一致性：非对齐步长分块（37 字节步长，跨分组边界）
    let mut hasher = sm3::Sm3Hasher::new();
    let mut offset = 0;
    while offset < msg.len() {
        let end = (offset + 37).min(msg.len());
        hasher.update(&msg[offset..end]);
        offset = end;
    }
    assert_eq!(
        hasher.finalize(),
        expected,
        "Streaming should match one-shot for large message"
    );
}

#[test]
fn test_sm3_deterministic() {
    // 相同输入产生相同输出
    let h1 = sm3::hash(b"determinism test input");
    let h2 = sm3::hash(b"determinism test input");
    assert_eq!(h1, h2, "SM3 should be deterministic");
    // 不同输入产生不同输出
    let h3 = sm3::hash(b"determinism test inpuX");
    assert_ne!(h1, h3, "Different inputs should produce different hashes");
}
