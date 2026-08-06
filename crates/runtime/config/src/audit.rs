//! Append-only audit log for the configuration service (SVC-008).
//!
//! Every mutating operation (`set`, `compare_and_swap`, transaction
//! `commit`/`abort`, `rollback`) records an [`AuditEntry`]. The log is
//! persisted to a single file on the config filesystem and appended to in
//! JSON-lines format (one compact JSON object per line) so that a corrupted
//! tail never invalidates earlier records.
//!
//! # Design
//!
//! - **Append-only**: records are never edited or deleted; the file is opened
//!   with `APPEND` and each entry is a self-contained line.
//! - **No unbounded growth in RAM**: the in-memory ring only retains the last
//!   `MEMORY_RING_CAP` entries for fast inspection; the on-disk file is the
//!   durable source of truth.
//! - **Best-effort persistence**: a write failure is surfaced via the returned
//!   `Result`, but the in-memory entry is still recorded so the caller can
//!   decide whether to fail the operation.
//! - **no_std**: only `alloc::*` / `core::*`; serialization is a hand-rolled
//!   compact JSON to avoid pulling a JSON writer dependency into this crate.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use eneros_fs::{FileSystem, Lfs, OpenFlags};

use crate::error::ConfigError;
use crate::schema::ConfigValue;

/// 序列化 ConfigValue 供审计日志记录（ARCH-G01：从 ConfigManager 迁出的辅助）。
///
/// 标量输出裸值字符串（`Int(502)` → `"502"`），容器输出 Debug 形式。
pub(crate) fn serialize_value_for_audit(v: &ConfigValue) -> String {
    match v {
        ConfigValue::Bool(b) => b.to_string(),
        ConfigValue::Int(i) => i.to_string(),
        ConfigValue::Float(f) => f.to_string(),
        ConfigValue::String(s) => s.clone(),
        ConfigValue::Array(a) => format!("{a:?}"),
        ConfigValue::Table(t) => format!("{t:?}"),
    }
}

/// A tiny `format!`-like helper that avoids `alloc::format!` allocation churn
/// for fixed concatenations. Returns a `String`. Defined at the top because
/// `macro_rules!` is textually scoped.
macro_rules! format_str {
    ($($part:expr),*) => {{
        let mut s = String::new();
        $( s.push_str($part.as_ref()); )*
        s
    }};
}
#[allow(unused_imports)]
use format_str;

/// Maximum number of entries retained in RAM for fast inspection. The on-disk
/// file is unbounded (append-only).
const MEMORY_RING_CAP: usize = 256;

/// The mutating operation that produced an audit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOp {
    /// `ConfigManager::set`.
    Set,
    /// `ConfigManager::compare_and_swap`.
    Cas,
    /// `Transaction::commit` (one entry per committed transaction).
    Commit,
    /// `Transaction::abort`.
    Abort,
    /// `ConfigManager::rollback`.
    Rollback,
}

impl AuditOp {
    fn as_str(self) -> &'static str {
        match self {
            AuditOp::Set => "set",
            AuditOp::Cas => "cas",
            AuditOp::Commit => "commit",
            AuditOp::Abort => "abort",
            AuditOp::Rollback => "rollback",
        }
    }
}

/// A single audit record. `old_value`/`new_value` are compact serializations
/// (or `None` when not applicable, e.g. `Abort`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// Monotonic-ish timestamp (epoch seconds, injected by the caller).
    pub timestamp: u64,
    /// Who performed the operation (service id, agent id, ...).
    pub actor: String,
    /// The operation kind.
    pub op: AuditOp,
    /// Dotted config path, or `"*"` for whole-config operations.
    pub path: String,
    /// Serialized previous value, if any.
    pub old_value: Option<String>,
    /// Serialized new value, if any.
    pub new_value: Option<String>,
    /// Whether the operation succeeded.
    pub success: bool,
}

impl AuditEntry {
    /// Serializes this entry to a single compact JSON line (no trailing
    /// newline). Strings are escaped minimally.
    pub fn to_json_line(&self) -> String {
        let mut s = String::from("{\"ts\":");
        s.push_str(&u64_to_string(self.timestamp));
        s.push_str(",\"actor\":\"");
        push_escaped(&mut s, &self.actor);
        s.push_str("\",\"op\":\"");
        s.push_str(self.op.as_str());
        s.push_str("\",\"path\":\"");
        push_escaped(&mut s, &self.path);
        s.push('"');
        match &self.old_value {
            Some(v) => {
                s.push_str(",\"old\":\"");
                push_escaped(&mut s, v);
                s.push('"');
            }
            None => s.push_str(",\"old\":null"),
        }
        match &self.new_value {
            Some(v) => {
                s.push_str(",\"new\":\"");
                push_escaped(&mut s, v);
                s.push('"');
            }
            None => s.push_str(",\"new\":null"),
        }
        s.push_str(",\"ok\":");
        s.push_str(if self.success { "true" } else { "false" });
        s.push('}');
        s
    }
}

/// Append-only audit log. Holds a bounded in-memory ring plus a durable
/// JSON-lines file on the config filesystem.
pub struct AuditLog {
    /// Bounded ring of the most recent entries (oldest at index 0).
    ring: Vec<AuditEntry>,
    /// Filesystem path of the durable audit file.
    path: String,
}

impl AuditLog {
    /// Creates a new audit log backed by the given file path. The file is
    /// created on first append if it does not exist.
    pub fn new(path: String) -> Self {
        Self {
            ring: Vec::new(),
            path,
        }
    }

    /// Appends an entry to the in-memory ring and the durable file.
    ///
    /// The in-memory entry is always recorded; a persistence failure is
    /// surfaced as `Err` so the caller can decide whether to fail the
    /// operation (recommended for write operations).
    pub fn append(&mut self, fs: &mut Lfs, entry: AuditEntry) -> Result<(), ConfigError> {
        // Bounded ring: evict oldest when full.
        if self.ring.len() >= MEMORY_RING_CAP {
            self.ring.remove(0);
        }
        self.ring.push(entry.clone());

        let line = entry.to_json_line();
        let mut buf = line.into_bytes();
        buf.push(b'\n');

        let mut file = fs.open(
            &self.path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::APPEND,
        )?;
        file.write(fs, &buf)?;
        Ok(())
    }

    /// Returns the most recent in-memory entries (oldest first).
    pub fn recent(&self) -> &[AuditEntry] {
        &self.ring
    }

    /// Number of in-memory entries (bounded; not the total on-disk count).
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Whether the in-memory ring is empty.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Reads back the durable audit file and returns all parseable entries.
    ///
    /// Lines that fail to parse are skipped (a truncated tail must not
    /// invalidate the earlier records). This is used by tests and admin
    /// inspection tools.
    ///
    /// CFG-001：`path` 参数化——旧实现硬编码 `Self::default_path()`，
    /// 无法读取自定义路径的审计文件。
    pub fn read_all(fs: &mut Lfs, path: &str) -> Result<Vec<AuditEntry>, ConfigError> {
        let size = match fs.stat(path) {
            Ok(s) => s.size,
            Err(eneros_fs::FsError::NotFound { .. }) => return Ok(Vec::new()),
            Err(e) => return Err(ConfigError::Fs(e)),
        };
        let mut file = fs.open(path, OpenFlags::READ)?;
        let mut buf = vec![0u8; size as usize];
        let mut filled = 0usize;
        loop {
            if filled >= buf.len() {
                break;
            }
            let n = file.read(fs, &mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(parse_json_lines(&buf))
    }

    /// Default audit file path under the config root.
    pub fn default_path() -> String {
        String::from("/config/.audit.log")
    }
}

/// Parses a JSON-lines buffer into entries, skipping malformed lines.
fn parse_json_lines(buf: &[u8]) -> Vec<AuditEntry> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..buf.len() {
        if buf[i] == b'\n' {
            let line = &buf[start..i];
            if !line.is_empty() {
                if let Some(e) = parse_one(line) {
                    out.push(e);
                }
            }
            start = i + 1;
        }
    }
    if start < buf.len() {
        if let Some(e) = parse_one(&buf[start..]) {
            out.push(e);
        }
    }
    out
}

/// Minimal single-line JSON parser for our compact audit format. Returns
/// `None` on any structural error (the line is then skipped).
fn parse_one(line: &[u8]) -> Option<AuditEntry> {
    let s = core::str::from_utf8(line).ok()?;
    let ts = json_field_u64(s, "ts")?;
    let actor = json_field_str(s, "actor")?;
    let op_str = json_field_str(s, "op")?;
    let op = match op_str.as_str() {
        "set" => AuditOp::Set,
        "cas" => AuditOp::Cas,
        "commit" => AuditOp::Commit,
        "abort" => AuditOp::Abort,
        "rollback" => AuditOp::Rollback,
        _ => return None,
    };
    let path = json_field_str(s, "path")?;
    let old_value = json_field_opt_str(s, "old");
    let new_value = json_field_opt_str(s, "new");
    let success = json_field_bool(s, "ok")?;
    Some(AuditEntry {
        timestamp: ts,
        actor,
        op,
        path,
        old_value,
        new_value,
        success,
    })
}

fn json_field_str(s: &str, key: &str) -> Option<String> {
    let pat = format_str!("\"", key, "\":\"");
    let idx = s.find(pat.as_str())? + pat.len();
    let rest = &s[idx..];
    // CFG-001：结束引号必须是**未转义**的 `"`（旧实现 `find('"')` 在转义
    // 引号 `\"` 处提前截断，含引号的值无法回读）。
    let mut escaped = false;
    let mut end = None;
    for (i, c) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    Some(unescape(&rest[..end?]))
}

fn json_field_opt_str(s: &str, key: &str) -> Option<String> {
    let null_pat = format_str!("\"", key, "\":null");
    if s.find(null_pat.as_str()).is_some() {
        return None;
    }
    json_field_str(s, key)
}

fn json_field_u64(s: &str, key: &str) -> Option<u64> {
    let pat = format_str!("\"", key, "\":");
    let idx = s.find(pat.as_str())? + pat.len();
    let rest = &s[idx..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse::<u64>().ok()
}

/// Extracts a boolean field `,"key":true` / `,"key":false`. Returns `None` if
/// the field is absent or not a boolean literal.
fn json_field_bool(s: &str, key: &str) -> Option<bool> {
    let true_pat = format_str!("\"", key, "\":true");
    if s.find(true_pat.as_str()).is_some() {
        return Some(true);
    }
    let false_pat = format_str!("\"", key, "\":false");
    if s.find(false_pat.as_str()).is_some() {
        return Some(false);
    }
    None
}

/// Pushes a minimally-escaped string into `out` (escapes `"`, `\`, and
/// control chars `\n`/`\r`/`\t`).
fn push_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // CFG-001：\u 转义必须是 4 位小写十六进制（旧代码用十进制
                // u32_to_string，与解析端 from_str_radix(hex,16) 不一致——
                // 例如 0x1B 写成 \u27，回读变成 0x27 字符）。
                out.push_str(&format_str!("\\u", u32_to_hex4(c as u32)));
            }
            c => out.push(c),
        }
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(n) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(n) {
                            out.push(ch);
                        }
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn u64_to_string(v: u64) -> String {
    let mut buf = [0u8; 20];
    let mut n = buf.len();
    let mut v = v;
    if v == 0 {
        return String::from("0");
    }
    while v > 0 {
        n -= 1;
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    core::str::from_utf8(&buf[n..]).unwrap_or("0").into()
}

/// CFG-001：4 位小写十六进制（`\u00xx` 转义用）。
fn u32_to_hex4(v: u32) -> String {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 4];
    let mut v = v & 0xFFFF;
    for i in (0..4).rev() {
        buf[i] = digits[(v % 16) as usize];
        v /= 16;
    }
    core::str::from_utf8(&buf).unwrap_or("0000").into()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn dummy_entry(op: AuditOp, success: bool) -> AuditEntry {
        AuditEntry {
            timestamp: 12345,
            actor: String::from("svc-008"),
            op,
            path: String::from("device.port"),
            old_value: Some(String::from("502")),
            new_value: Some(String::from("503")),
            success,
        }
    }

    #[test]
    fn test_op_as_str_roundtrip() {
        for op in [
            AuditOp::Set,
            AuditOp::Cas,
            AuditOp::Commit,
            AuditOp::Abort,
            AuditOp::Rollback,
        ] {
            let s = op.as_str();
            let back = match s {
                "set" => AuditOp::Set,
                "cas" => AuditOp::Cas,
                "commit" => AuditOp::Commit,
                "abort" => AuditOp::Abort,
                "rollback" => AuditOp::Rollback,
                _ => panic!("unknown op"),
            };
            assert_eq!(op, back);
        }
    }

    #[test]
    fn test_entry_json_line_roundtrip() {
        let e = dummy_entry(AuditOp::Cas, true);
        let line = e.to_json_line();
        assert!(line.contains("\"ts\":12345"));
        assert!(line.contains("\"actor\":\"svc-008\""));
        assert!(line.contains("\"op\":\"cas\""));
        assert!(line.contains("\"path\":\"device.port\""));
        assert!(line.contains("\"old\":\"502\""));
        assert!(line.contains("\"new\":\"503\""));
        assert!(line.contains("\"ok\":true"));

        let parsed = parse_one(line.as_bytes()).expect("parse");
        assert_eq!(parsed, e);
    }

    #[test]
    fn test_entry_json_line_null_values() {
        let e = AuditEntry {
            timestamp: 1,
            actor: String::from("a"),
            op: AuditOp::Abort,
            path: String::from("*"),
            old_value: None,
            new_value: None,
            success: false,
        };
        let line = e.to_json_line();
        assert!(line.contains("\"old\":null"));
        assert!(line.contains("\"new\":null"));
        assert!(line.contains("\"ok\":false"));
        let parsed = parse_one(line.as_bytes()).expect("parse");
        assert_eq!(parsed, e);
    }

    #[test]
    fn test_escape_and_unescape_roundtrip() {
        let cases = [
            "plain",
            "with\"quote",
            "with\\backslash",
            "with\nnewline",
            "tab\there",
            "unicode: 中文 🦀",
        ];
        for c in cases {
            let mut escaped = String::new();
            push_escaped(&mut escaped, c);
            let back = unescape(&escaped);
            assert_eq!(back, c, "roundtrip failed for {:?}", c);
        }
    }

    #[test]
    fn test_parse_skips_malformed_lines() {
        let buf = b"{\"ts\":1,\"actor\":\"a\",\"op\":\"set\",\"path\":\"x\",\"old\":null,\"new\":\"1\",\"ok\":true}\n\
                   GARBAGE LINE\n\
                   {\"ts\":2,\"actor\":\"b\",\"op\":\"cas\",\"path\":\"y\",\"old\":\"1\",\"new\":\"2\",\"ok\":false}\n";
        let entries = parse_json_lines(buf);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].op, AuditOp::Set);
        assert_eq!(entries[1].op, AuditOp::Cas);
        assert!(!entries[1].success);
    }

    #[test]
    fn test_u64_to_string() {
        assert_eq!(u64_to_string(0), "0");
        assert_eq!(u64_to_string(1), "1");
        assert_eq!(u64_to_string(42), "42");
        assert_eq!(u64_to_string(u64::MAX), "18446744073709551615");
    }

    #[test]
    fn test_ring_bounded_eviction() {
        // Build a log with cap+5 entries; only the last cap should remain.
        let mut log = AuditLog::new(String::from("/config/.audit.log"));
        // We cannot easily call append without an FS here, so test the ring
        // eviction logic directly by simulating.
        for i in 0..(MEMORY_RING_CAP + 5) {
            let e = AuditEntry {
                timestamp: i as u64,
                actor: String::from("a"),
                op: AuditOp::Set,
                path: String::from("p"),
                old_value: None,
                new_value: None,
                success: true,
            };
            if log.ring.len() >= MEMORY_RING_CAP {
                log.ring.remove(0);
            }
            log.ring.push(e);
        }
        assert_eq!(log.ring.len(), MEMORY_RING_CAP);
        // First retained entry should be timestamp = 5 (evicted 0..4).
        assert_eq!(log.ring[0].timestamp, 5);
        assert_eq!(
            log.ring.last().unwrap().timestamp,
            (MEMORY_RING_CAP + 4) as u64
        );
    }

    // ===== CFG-001: JSON 编解码 round-trip =====

    fn sample_entry(actor: &str, path: &str, old: Option<&str>, new: Option<&str>) -> AuditEntry {
        AuditEntry {
            timestamp: 1700000000,
            actor: String::from(actor),
            op: AuditOp::Set,
            path: String::from(path),
            old_value: old.map(String::from),
            new_value: new.map(String::from),
            success: true,
        }
    }

    fn roundtrip(e: &AuditEntry) -> AuditEntry {
        let line = e.to_json_line();
        let parsed = parse_one(line.as_bytes()).expect("parse");
        assert_eq!(&parsed, e, "round-trip mismatch for {line:?}");
        parsed
    }

    /// CFG-001：actor/path/值含引号、反斜杠、换行 → round-trip 一致。
    /// 旧实现 `json_field_str` 用 `find('"')` 在转义引号处提前截断（旧代码失败证据）。
    #[test]
    fn test_cfg001_roundtrip_escaped_quotes() {
        let e = sample_entry("op\"er\"ator", "a\"b", Some("x\"y"), Some("z"));
        roundtrip(&e);
    }

    /// CFG-001：控制字符（0x1B ESC 等）→ `\u001b` 4 位 hex，round-trip 一致。
    /// 旧实现写十进制 `\u27`，解析端按 hex 读 → 0x27 ≠ 0x1B（旧代码失败证据）。
    #[test]
    fn test_cfg001_roundtrip_control_chars() {
        let e = sample_entry("esc\x1b", "c\x00tl", Some("a\x1b\x00z"), None);
        let line = e.to_json_line();
        assert!(line.contains("\\u001b"), "must emit 4-hex \\u escape: {line}");
        assert!(line.contains("\\u0000"), "must emit 4-hex \\u0000: {line}");
        roundtrip(&e);
    }

    /// CFG-001：反斜杠与 tab/CR/LF 混合 → round-trip 一致。
    #[test]
    fn test_cfg001_roundtrip_backslash_and_whitespace() {
        let e = sample_entry("a\\b\tc\rd\ne", "p", Some("q\\r"), Some("s\tt"));
        roundtrip(&e);
    }

    /// CFG-001：`read_all` 自定义路径（旧实现硬编码 default_path）。
    #[test]
    fn test_cfg001_read_all_custom_path() {
        use eneros_fs::FileSystem;
        use eneros_storage::{BlockDevice, MockBlockDevice};

        let dev: Box<dyn BlockDevice> = Box::new(MockBlockDevice::new(64, 4096));
        let mut fs = Lfs::format(dev).expect("format failed");
        let _ = fs.mkdir("/custom");
        let path = "/custom/.audit.log";
        // 通过 append 写入自定义路径
        let mut log = AuditLog::new(String::from(path));
        let e = sample_entry("custom", "k", Some("1"), Some("2"));
        log.append(&mut fs, e.clone()).expect("append");
        // read_all 用自定义路径读取
        let entries = AuditLog::read_all(&mut fs, path).expect("read_all");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], e);
    }
}
