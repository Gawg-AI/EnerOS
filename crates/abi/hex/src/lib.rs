//! # eneros-hex —— hex 编解码单一实现（蓝图 64 卷 64.3 WP-64.3.4）
//!
//! 此前 13+ 份生产抄本散布各 crate（大小写/返回形状/校验策略三向漂移：
//! `hex16_into`/`hex_encode`/`mac_hex_string`/`hex_string`/`hex_decode`/
//! `parse_hex16`/`hex_val`/`hex_nibble`）——归一于此。rg 守卫
//! （`hex_copies_rg_guard`，本 crate 测试）锁白名单外零抄本。
//!
//! 语义裁决（卷册 WP-64.3.4，执行卡登记）：
//! - encode 显式两版：小写（默认）/大写（crypto pki serial 显示保持
//!   大写行为不变）；
//! - decode **接受大小写混合**（各抄本最宽语义的并集——qos_store 收
//!   大写、其余仅小写；放宽为兼容超集，逐处行为兼容）；
//! - 叶 crate 零依赖（R-64.a）：只 core/alloc；不加 base64 等其他编码
//!   （YAGNI）。
//!
//! 豁免白名单（守卫内逐条登记）：beidou const 上下文 `hex_val`
//! （input.rs，const fn 不可替换）与 beidou nmea 字节级 nibble 判据
//! （返回 `SyncError` 类型化错误，语义不同）。

#![no_std]

extern crate alloc;
// 守卫测试在 host std 下跑（no_std crate 的测试面；workspace 先例）。
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

/// nibble → ASCII hex 字符（`upper` 选择 'A'..'F' 或 'a'..'f' 段）。
#[inline]
fn nibble_to_ascii(nibble: u8, upper: bool) -> u8 {
    let base = if upper { b'A' } else { b'a' };
    if nibble < 10 {
        b'0' + nibble
    } else {
        base + (nibble - 10)
    }
}

/// ASCII hex（大小写皆收）→ nibble。
#[inline]
fn ascii_to_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// 小写 hex 编码（分配版）。
pub fn encode_lower(bytes: &[u8]) -> String {
    encode_with_case(bytes, false)
}

/// 大写 hex 编码（分配版；crypto pki serial 显示等既有大写行为保持）。
pub fn encode_upper(bytes: &[u8]) -> String {
    encode_with_case(bytes, true)
}

fn encode_with_case(bytes: &[u8], upper: bool) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    let mut buf = [0u8; 2];
    for &b in bytes {
        buf[0] = nibble_to_ascii(b >> 4, upper);
        buf[1] = nibble_to_ascii(b & 0x0F, upper);
        out.push_str(core::str::from_utf8(&buf).expect("ASCII"));
    }
    out
}

/// 无分配形态（`hex16_into` 先例）：`bytes` 前 `out.len()/2` 字节编码进
/// `out`（ASCII，小写）；`out` 为奇数长度时末位未定义写入为 0（调用方
/// 传偶数长度——`hex16_into` 契约：16B 出 32B 摘要前 8B）。
pub fn encode_into_lower(bytes: &[u8], out: &mut [u8]) {
    let pairs = out.len() / 2;
    let pairs = pairs.min(bytes.len());
    for i in 0..pairs {
        let b = bytes[i];
        out[i * 2] = nibble_to_ascii(b >> 4, false);
        out[i * 2 + 1] = nibble_to_ascii(b & 0x0F, false);
    }
}

/// 定长解码（大小写混合皆收；长度/非法字符 → `None`）。
pub fn decode_fixed<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in b.chunks_exact(2).enumerate() {
        let hi = ascii_to_nibble(pair[0])?;
        let lo = ascii_to_nibble(pair[1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

/// 变长解码（大小写混合皆收；奇数长度/非法字符 → `None`）。
pub fn decode_vec(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        let hi = ascii_to_nibble(pair[0])?;
        let lo = ascii_to_nibble(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::vec;

    use super::*;

    #[test]
    fn encode_case_and_roundtrip() {
        assert_eq!(encode_lower(&[0xDE, 0xAD, 0xBE, 0xEF]), "deadbeef");
        assert_eq!(encode_upper(&[0xDE, 0xAD, 0xBE, 0xEF]), "DEADBEEF");
        assert_eq!(encode_lower(&[]), "");
        // decode 大小写混合皆收（最宽语义）。
        assert_eq!(decode_vec("deadbeef"), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(decode_vec("DEADBEEF"), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(decode_vec("DeAdBeEf"), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(
            decode_vec("deadbeef").as_deref(),
            Some(&[0xDEu8, 0xAD, 0xBE, 0xEF][..])
        );
        // 往返。
        let bytes: alloc::vec::Vec<u8> = (0u8..=255).collect();
        assert_eq!(decode_vec(&encode_lower(&bytes)), Some(bytes.clone()));
        assert_eq!(decode_vec(&encode_upper(&bytes)), Some(bytes));
    }

    #[test]
    fn decode_rejects() {
        assert_eq!(decode_vec("abc"), None); // 奇数长度
        assert_eq!(decode_vec("zz"), None); // 非法字符
        assert_eq!(
            decode_fixed::<4>("deadbeef"),
            Some([0xDE, 0xAD, 0xBE, 0xEF])
        );
        assert_eq!(decode_fixed::<4>("deadbee"), None); // 长度
        assert_eq!(decode_fixed::<0>(""), Some([]));
    }

    #[test]
    fn encode_into_matches_hex16_into_semantics() {
        // svc-modbus-tcp hex16_into 原语义：32B 摘要前 8B → 16B ASCII。
        let digest = [0xABu8; 32];
        let mut out = [0u8; 16];
        encode_into_lower(&digest, &mut out);
        assert_eq!(&out, b"abababababababab");
        // 短出缓冲不越界（写满即止）。
        let mut small = [0u8; 4];
        encode_into_lower(&[0x12, 0x34, 0x56], &mut small);
        assert_eq!(&small, b"1234");
    }

    /// RFC 风格 KAT（SHA-256 空串/“abc”向量的小写 hex 面）。
    #[test]
    fn known_answer_vectors() {
        assert_eq!(
            encode_lower(&[
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            encode_lower(&[
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

/// hex 抄本 rg 守卫（WP-64.3.4.3）：白名单外全仓 src 零生产抄本。
///
/// 遍历 `workspace/crates/**.rs`（跳过 `*/tests/` 目录与本 crate），
/// 命中禁写签名即失败。变异验证：任一替换点改回本地抄本 → 本守卫红。
#[cfg(test)]
mod rg_guard {
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use std::path::Path;

    /// 禁写 fn 签名（卷册守卫字面 + 左括号精确定位定义行）。
    const FORBIDDEN: [&str; 8] = [
        "fn hex_encode(",
        "fn hex_decode(",
        "fn hex16_into(",
        "fn mac_hex_string(",
        "fn hex_val(",
        "fn hex_nibble(",
        "fn parse_hex16(",
        "fn hex_string(",
    ];

    /// 豁免白名单（路径后缀匹配；逐条登记 ID）：
    /// - REG-HEX-BEIDOU-CONST：beidou/input.rs const fn hex_val（const
    ///   上下文必需，叶 crate 无 const 面——保留局部 const 版）；
    /// - REG-HEX-BEIDOU-NMEA：beidou/nmea.rs hex_val（字节级 nibble 判据，
    ///   返回 SyncError 类型化错误——语义不同非抄本）；
    /// - REG-HEX-PAYLOAD-VALIDATOR：config-bundle/payload.rs pub
    ///   hex_decode（pub 校验面——错误携带 ConfigId/字段词形，内部解码
    ///   已单源 eneros_hex::decode_vec；卷册"pub 面保留 re-export 防
    ///   break"裁决）。
    const WHITELIST: [&str; 3] = [
        "drivers/time/src/beidou/input.rs",
        "drivers/time/src/beidou/nmea.rs",
        "runtime/config-bundle/src/payload.rs",
    ];

    fn walk(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("workspace crates/") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let as_str = path.to_string_lossy().to_string();
                // tests/ 目录豁免（测试夹具抄本卷册已登记）；本 crate 豁免。
                if as_str.contains("/tests/") || as_str.contains("/abi/hex/") {
                    continue;
                }
                if WHITELIST.iter().any(|w| as_str.contains(w)) {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("read src");
                for line in src.lines() {
                    for f in FORBIDDEN {
                        if line.contains(f) {
                            hits.push(alloc::format!("{as_str}: {line}"));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn hex_copies_rg_guard() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("workspace root")
            .to_path_buf();
        let mut hits = Vec::new();
        walk(&workspace.join("crates"), &mut hits);
        assert!(
            hits.is_empty(),
            "白名单外 hex 生产抄本残存（须改用 eneros_hex）:\n{}",
            hits.join("\n")
        );
    }
}
