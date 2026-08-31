//! Phase 7 能力闭包证据：联邦 crate 不得引用站内写面（蓝图 61 §12 §2.8
//! 退出 gate + 16 §4 svc-federation 无 writer；也覆盖 §5 禁止反向依赖）。

#![allow(clippy::disallowed_macros)]
use std::path::Path;

/// 站内写面符号白名单——出现在联邦 crate 的源码中即失败。
const FORBIDDEN_SYMBOLS: &[&str] = &[
    "ControlGateway",
    "AuthorizedWriter",
    "AuthorizedCommand",
    "authorized_command",
    "UnavailableTransport",
];

/// 剥离 `//` 行注释与 `/* */` 块注释后的代码文本（文档注释声明
/// "禁止依赖 ControlGateway" 不构成实际引用）。
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn walk_rs(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name.starts_with('.') {
                continue;
            }
            walk_rs(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            // 只扫库与生产二进制源码；测试文件自身声明符号属反例定义。
            let rel = p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(&p);
            if !rel.starts_with("src") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let code = strip_comments(&text);
            for sym in FORBIDDEN_SYMBOLS {
                if code.contains(sym) {
                    out.push((p.display().to_string(), sym.to_string()));
                }
            }
        }
    }
}

#[test]
fn federation_has_no_writer_symbols() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits: Vec<(String, String)> = Vec::new();
    walk_rs(root, &mut hits);
    assert!(
        hits.is_empty(),
        "联邦 crate 代码出现站内写面符号（依赖图违反）: {hits:?}"
    );
}

#[test]
fn federation_depends_only_on_crypto() {
    // 依赖闭包：eneros-federation 生产依赖只能是 eneros-crypto
    // （+ tls feature 下的 eneros-tls，二者均为非写面 crate）。
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = std::fs::read_to_string(&cargo).expect("Cargo.toml");
    let deps: Vec<&str> = text
        .lines()
        .filter(|l| l.trim().starts_with("eneros-") && !l.trim().starts_with("#"))
        .map(|l| l.trim().split('=').next().unwrap_or("").trim())
        .collect();
    for d in deps {
        assert!(
            d == "eneros-crypto" || d == "eneros-tls",
            "依赖 {d} 不属联邦允许闭包（禁止写面 crate 依赖）"
        );
    }
}
