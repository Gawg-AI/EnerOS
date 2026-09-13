//! 冻结记录与平台描述的摘要单源（WP-70.12.1「冻结记录带版本和来源 digest」/
//! WP-70.12.2「运行时核对 SKU 与描述 digest」）。
//!
//! 摘要口径（域分隔、逐字段、与 JSON 序列化无关——**不依赖 serde**，
//! 因此同一函数可在 no_std 目标侧与 xtask 侧共用，避免"两侧各算一遍"
//! 产生不可比摘要）：
//!
//! ```text
//! line := <field> '=' <len> ':' <bytes> '\n'     // len = 十进制字节数
//! digest := sha256( "eneros.board-facts.v1\0" || Σ line )   // 大写 16 进制
//! ```
//!
//! 采用长度前缀而不是 `k=v\n` 裸拼接：值里出现 `=`/换行/空串都不会造成
//! 字段边界歧义（单源摘要公式的常见坑）。

use alloc::string::String;
use alloc::vec::Vec;

use crate::fact::{Fact, UNIT_NONE};

/// 域分隔标记（版本化；换公式必须换标记）。
pub const LEDGER_DOMAIN: &[u8] = b"eneros.board-facts.v1\0";

/// 记录级摘要（不含摘要自身）。
pub fn ledger_digest(sku_id: &str, revision: &str, facts: &[Fact]) -> String {
    let mut fields: Vec<(String, String)> = Vec::new();
    fields.push(("sku_id".into(), sku_id.into()));
    fields.push(("revision".into(), revision.into()));
    for f in sorted_facts(facts) {
        let id = alloc::format!("{}#{}", f.subsystem.json_name(), f.key);
        fields.push((
            id,
            alloc::format!(
                "v={}|u={}|ver={}|scope={}|src={}|ev={}|dg={}",
                f.value,
                f.unit,
                f.verified,
                f.hardware_scope.json_name(),
                f.source,
                f.evidence,
                f.digest.as_deref().unwrap_or("")
            ),
        ));
    }
    join(fields)
}

/// 平台描述摘要（运行时 DTB / 描述字节）。
pub fn description_digest(bytes: &[u8]) -> String {
    join(Vec::from([(
        "description".into(),
        hex_lower(&sha256(bytes)),
    )]))
}

/// 字段规范化拼接 + sha256。
fn join(fields: Vec<(String, String)>) -> String {
    let mut buf = Vec::new();
    buf.extend_from_slice(LEDGER_DOMAIN);
    for (k, v) in fields {
        let len = v.len();
        buf.extend_from_slice(k.as_bytes());
        buf.push(b'=');
        buf.extend_from_slice(alloc::format!("{len}:").as_bytes());
        buf.extend_from_slice(v.as_bytes());
        buf.push(b'\n');
    }
    hex_lower(&sha256(&buf))
}

/// 事实按 (能力族, 键) 排序——与调用方给的顺序无关。
fn sorted_facts(facts: &[Fact]) -> Vec<&Fact> {
    let mut out: Vec<&Fact> = facts.iter().collect();
    out.sort_by(|a, b| (a.subsystem, a.key.as_str()).cmp(&(b.subsystem, b.key.as_str())));
    out
}

/// 单条事实的稳定标识（报告/证据绑定用）。
pub fn fact_id(f: &Fact) -> String {
    alloc::format!("{}#{}", f.subsystem.json_name(), f.key)
}

/// 无单位事实的单位展示。
pub fn unit_or_none(f: &Fact) -> &str {
    if f.unit.is_empty() {
        UNIT_NONE
    } else {
        f.unit.as_str()
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

// ============================================================
// SHA-256（no_std 自持实现，避免给目标侧 crate 引入依赖）
// ============================================================

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256（FIPS 180-4）。公开供证据绑定复用（xtask 侧亦引用同一实现）。
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut block = [0u8; 64];
    let mut filled = 0usize;

    let mut consume = |chunk: &[u8], filled: &mut usize, h: &mut [u32; 8]| {
        for &b in chunk {
            block[*filled] = b;
            *filled += 1;
            if *filled == 64 {
                let mut w = [0u32; 64];
                for (i, word) in block.chunks_exact(4).enumerate() {
                    w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
                }
                compress(&mut w, h);
                *filled = 0;
            }
        }
    };

    consume(data, &mut filled, &mut h);
    let total_len = data.len() as u64;
    // 填充（0x80 + 0 填充至 ≡ 56 mod 64 + 64 位大端长度）。
    let mut tail: Vec<u8> = Vec::new();
    tail.push(0x80);
    while (total_len + tail.len() as u64 + 8) % 64 != 0 {
        tail.push(0);
    }
    tail.extend_from_slice(&(total_len * 8).to_be_bytes());
    consume(&tail, &mut filled, &mut h);
    debug_assert_eq!(filled, 0);

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn compress(w: &mut [u32; 64], h: &mut [u32; 8]) {
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Subsystem;

    #[test]
    fn sha256_matches_fips_vectors() {
        // FIPS 180-4 / NIST 已知向量（单源摘要必须与通用实现一致）。
        assert_eq!(
            hex_lower(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_lower(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_lower(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // 跨块边界（55/56/64 字节）。
        assert_eq!(
            hex_lower(&sha256(&[b'a'; 64])),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
    }

    #[test]
    fn digest_is_field_boundary_safe() {
        // 旧坑：k=v 裸拼接时 "a=1" + "b=2" 与 "a=1\nb=2" 可能同摘要。
        let mut f1 = vec![
            Fact::repo(Subsystem::Cpu, "cores", "8", "count"),
            Fact::repo(Subsystem::Ram, "base", "0x0", "addr"),
        ];
        let d1 = ledger_digest("sku", "rev1", &f1);
        // 把值挪到另一个字段（键不变、内容搬移）必须改变摘要。
        f1[0].value = "8b".into();
        f1[1].value = "0x00".into();
        let d2 = ledger_digest("sku", "rev1", &f1);
        assert_ne!(d1, d2);
        // 值里含 '='/'\n' 不造成歧义。
        let f2 = vec![Fact::repo(Subsystem::Cpu, "cores", "8=9\nx", "count")];
        let d3 = ledger_digest("sku", "rev1", &f2);
        let mut f3 = vec![Fact::repo(Subsystem::Cpu, "cores", "8", "count")];
        f3.push(Fact::repo(Subsystem::Ram, "x", "9", "count"));
        assert_ne!(d3, ledger_digest("sku", "rev1", &f3));
    }

    #[test]
    fn digest_is_order_independent_but_content_sensitive() {
        let a = vec![
            Fact::repo(Subsystem::Cpu, "cores", "8", "count"),
            Fact::repo(Subsystem::Ram, "base", "0x0", "addr"),
        ];
        let b = {
            let mut v = a.clone();
            v.reverse();
            v
        };
        assert_eq!(ledger_digest("s", "r", &a), ledger_digest("s", "r", &b));
        let mut c = a.clone();
        c[0].verified = false;
        assert_ne!(ledger_digest("s", "r", &a), ledger_digest("s", "r", &c));
        let mut d = a.clone();
        d[1].key = "base2".into();
        assert_ne!(ledger_digest("s", "r", &a), ledger_digest("s", "r", &d));
        // sku/revision 进摘要（换板修订 = 新组合）。
        assert_ne!(ledger_digest("s", "r", &a), ledger_digest("s", "r2", &a));
        assert_ne!(ledger_digest("s", "r", &a), ledger_digest("s2", "r", &a));
    }

    #[test]
    fn description_digest_is_hex_and_sensitive() {
        let d = description_digest(b"dtb-bytes");
        assert_eq!(d.len(), 64);
        assert_ne!(d, description_digest(b"dtb-bytes2"));
    }
}
