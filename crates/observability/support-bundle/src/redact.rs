//! 密钥脱敏引擎（OBS-008）。
//!
//! 基于模式匹配的密钥/凭证脱敏。在加密前扫描所有条目内容，将匹配到的
//! 密钥模式替换为 `[REDACTED:TYPE]`，确保支持包不泄露 secret。
//!
//! # 支持的模式
//!
//! - PEM 私钥块（`-----BEGIN ... PRIVATE KEY-----` ... `-----END ...`）
//! - password=/passwd=/pwd= 字段
//! - password:/passwd: 字段
//! - Bearer token
//! - token=/api_key=/apikey=/secret= 字段
//! - SM2/EC 私钥 hex（`priv=` 前缀 + 64 hex 字符）
//! - Authorization header

use alloc::format;
use alloc::string::String;

/// 密钥模式类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretPattern {
    /// PEM 私钥块。
    PemPrivateKey,
    /// `password=...` / `passwd=...` / `pwd=...`
    PasswordField,
    /// `password:...` / `passwd:...`
    PasswordColon,
    /// `Bearer ...`
    BearerToken,
    /// `token=...` / `api_key=...` / `apikey=...` / `secret=...`
    CredentialField,
    /// `priv=<64 hex chars>` — SM2/EC 私钥
    Sm2PrivateKeyHex,
    /// `Authorization: ...`
    AuthorizationHeader,
}

impl SecretPattern {
    pub fn label(self) -> &'static str {
        match self {
            Self::PemPrivateKey => "PEM_PRIVATE_KEY",
            Self::PasswordField => "PASSWORD",
            Self::PasswordColon => "PASSWORD",
            Self::BearerToken => "BEARER_TOKEN",
            Self::CredentialField => "CREDENTIAL",
            Self::Sm2PrivateKeyHex => "SM2_PRIVATE_KEY",
            Self::AuthorizationHeader => "AUTH_HEADER",
        }
    }
}

/// 脱敏引擎。
pub struct Redactor {
    /// 是否启用各模式（全部默认启用）。
    enabled: [bool; 7],
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    /// 创建默认脱敏器（全部模式启用）。
    pub fn new() -> Self {
        Self { enabled: [true; 7] }
    }

    /// 禁用某个模式。
    pub fn disable(&mut self, pattern: SecretPattern) -> &mut Self {
        self.enabled[pattern as usize] = false;
        self
    }

    /// 对输入文本执行脱敏，返回脱敏后的文本和匹配到的模式数。
    pub fn redact(&self, input: &str) -> (String, usize) {
        let mut result = String::with_capacity(input.len());
        let mut count = 0usize;
        let bytes = input.as_bytes();
        let mut i = 0usize;

        while i < bytes.len() {
            let remaining = &bytes[i..];
            // 尝试每个模式
            if let Some((_pattern, match_len, replacement)) = self.try_match(remaining) {
                count += 1;
                result.push_str(&replacement);
                i += match_len;
            } else {
                // 复制一个字节
                let ch = bytes[i];
                result.push(ch as char);
                i += 1;
            }
        }
        (result, count)
    }

    /// 尝试在当前位置匹配某个模式。返回 (模式, 匹配长度, 替换文本)。
    fn try_match(&self, data: &[u8]) -> Option<(SecretPattern, usize, String)> {
        // PEM 私钥块
        if self.enabled[SecretPattern::PemPrivateKey as usize] {
            if let Some(len) = match_pem_block(data) {
                return Some((
                    SecretPattern::PemPrivateKey,
                    len,
                    format!("[REDACTED:{}]", SecretPattern::PemPrivateKey.label()),
                ));
            }
        }

        // 检查不区分大小写的前缀匹配
        let lower = to_lower_prefix(data, 32);

        // password=/passwd=/pwd=
        if self.enabled[SecretPattern::PasswordField as usize] {
            for kw in ["password=", "passwd=", "pwd="] {
                if lower.starts_with(kw) {
                    let value_len = scan_value_len(data, kw.len());
                    let total = kw.len() + value_len;
                    return Some((
                        SecretPattern::PasswordField,
                        total,
                        format!("[REDACTED:{}]", SecretPattern::PasswordField.label()),
                    ));
                }
            }
        }

        // password:/passwd:
        if self.enabled[SecretPattern::PasswordColon as usize] {
            for kw in ["password:", "passwd:"] {
                if lower.starts_with(kw) {
                    let value_len = scan_value_len(data, kw.len());
                    let total = kw.len() + value_len;
                    return Some((
                        SecretPattern::PasswordColon,
                        total,
                        format!("[REDACTED:{}]", SecretPattern::PasswordColon.label()),
                    ));
                }
            }
        }

        // Bearer token
        if self.enabled[SecretPattern::BearerToken as usize] && lower.starts_with("bearer ") {
            let value_len = scan_token_len(data, 7);
            let total = 7 + value_len;
            return Some((
                SecretPattern::BearerToken,
                total,
                format!("[REDACTED:{}]", SecretPattern::BearerToken.label()),
            ));
        }

        // token=/api_key=/apikey=/secret=
        if self.enabled[SecretPattern::CredentialField as usize] {
            for kw in ["api_key=", "apikey=", "token=", "secret="] {
                if lower.starts_with(kw) {
                    let value_len = scan_value_len(data, kw.len());
                    let total = kw.len() + value_len;
                    return Some((
                        SecretPattern::CredentialField,
                        total,
                        format!("[REDACTED:{}]", SecretPattern::CredentialField.label()),
                    ));
                }
            }
        }

        // SM2 私钥 hex: priv=<64 hex>
        if self.enabled[SecretPattern::Sm2PrivateKeyHex as usize] && lower.starts_with("priv=") {
            let hex_len = scan_hex_len(data, 5);
            if hex_len >= 32 {
                let total = 5 + hex_len;
                return Some((
                    SecretPattern::Sm2PrivateKeyHex,
                    total,
                    format!("[REDACTED:{}]", SecretPattern::Sm2PrivateKeyHex.label()),
                ));
            }
        }

        // Authorization header — 扫描到行尾（值可能含空格，如 "Basic dXNlcjpwYXNz"）
        if self.enabled[SecretPattern::AuthorizationHeader as usize]
            && lower.starts_with("authorization:")
        {
            let value_len = scan_line_len(data, 14);
            let total = 14 + value_len;
            return Some((
                SecretPattern::AuthorizationHeader,
                total,
                format!("[REDACTED:{}]", SecretPattern::AuthorizationHeader.label()),
            ));
        }

        None
    }
}

/// 匹配 PEM 私钥块，返回块总长度（包含 END 行）。
fn match_pem_block(data: &[u8]) -> Option<usize> {
    let begin = b"-----BEGIN ";
    if data.len() < begin.len() || &data[..begin.len()] != begin {
        return None;
    }
    // 查找 " PRIVATE KEY-----"
    let marker = b" PRIVATE KEY-----";
    let pos = find_subslice(data, marker)?;
    // 查找换行后的 -----END
    let end_marker = b"-----END ";
    let end_pos = find_subslice(&data[pos..], end_marker)?;
    // 查找 END 行的结束（换行或文件尾）
    let after_end = pos + end_pos + end_marker.len();
    let line_end = find_subslice(&data[after_end..], b"-----")?;
    let block_end = after_end + line_end + 5; // 包含 "-----"
                                              // 可选的换行
    if block_end < data.len() && data[block_end] == b'\n' {
        Some(block_end + 1)
    } else {
        Some(block_end)
    }
}

/// 扫描 = 后的值长度（直到空白、换行或行尾）。
fn scan_value_len(data: &[u8], offset: usize) -> usize {
    let mut len = 0usize;
    for &b in &data[offset..] {
        if b == b' '
            || b == b'\n'
            || b == b'\r'
            || b == b'\t'
            || b == b','
            || b == b'}'
            || b == b'"'
        {
            break;
        }
        len += 1;
    }
    len
}

/// 扫描 token 值长度（直到空白或换行）。
fn scan_token_len(data: &[u8], offset: usize) -> usize {
    scan_value_len(data, offset)
}

/// 扫描 hex 字符长度。
fn scan_hex_len(data: &[u8], offset: usize) -> usize {
    let mut len = 0usize;
    for &b in &data[offset..] {
        if b.is_ascii_hexdigit() {
            len += 1;
        } else {
            break;
        }
    }
    len
}

/// 扫描到行尾的长度（直到 `\n` 或 `\r` 或数据结束）。
/// 用于 Authorization 等可能包含空格的 header 值。
fn scan_line_len(data: &[u8], offset: usize) -> usize {
    let mut len = 0usize;
    for &b in &data[offset..] {
        if b == b'\n' || b == b'\r' {
            break;
        }
        len += 1;
    }
    len
}

/// 查找子切片位置。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

/// 返回 data 前 n 字节的小写副本（用于不区分大小写匹配）。
fn to_lower_prefix(data: &[u8], n: usize) -> String {
    let len = data.len().min(n);
    let mut s = String::with_capacity(len);
    for &b in &data[..len] {
        s.push((b as char).to_ascii_lowercase());
    }
    s
}

/// 验证已脱敏的文本中不再包含任何密钥模式。
///
/// OBS-008 核心要求：redaction 验证。在加密前对每个条目执行此检查，
/// 任何残留密钥模式导致导出失败（fail-closed）。
///
/// 返回 `Ok(())` 表示无残留密钥；`Err(pattern)` 表示发现残留。
pub fn verify_no_secrets(text: &str) -> Result<(), SecretPattern> {
    let redactor = Redactor::new();
    let (_, count) = redactor.redact(text);
    if count > 0 {
        // 找到第一个匹配的模式
        let bytes = text.as_bytes();
        for i in 0..bytes.len() {
            if let Some((pattern, _, _)) = redactor.try_match(&bytes[i..]) {
                return Err(pattern);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_pem_private_key() {
        let input = "config:\n-----BEGIN EC PRIVATE KEY-----\nMHQCAQEE...\n-----END EC PRIVATE KEY-----\ndone";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 1);
        assert!(output.contains("[REDACTED:PEM_PRIVATE_KEY]"));
        assert!(!output.contains("MHQCAQEE"));
    }

    #[test]
    fn redact_password_field() {
        let input = "db_conn=password=s3cr3t host=db";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 1);
        // 整个 key=value 被替换为 [REDACTED:PASSWORD]（保留前缀会导致
        // verify_no_secrets 误判，因为 "password=" 本身是匹配模式）
        assert!(output.contains("[REDACTED:PASSWORD]"));
        assert!(!output.contains("s3cr3t"));
        assert!(!output.contains("password="));
    }

    #[test]
    fn redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let (output, count) = Redactor::new().redact(input);
        assert!(count >= 1);
        assert!(!output.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn redact_sm2_private_key_hex() {
        let input =
            "signing_key=priv=ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 1);
        // 整个 priv=<hex> 被替换为 [REDACTED:SM2_PRIVATE_KEY]
        assert!(output.contains("[REDACTED:SM2_PRIVATE_KEY]"));
        assert!(!output.contains("ABCDEF0123456789"));
        assert!(!output.contains("priv="));
    }

    #[test]
    fn redact_multiple_secrets() {
        let input = "password=secret1 token=abc123\n-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----";
        let (output, count) = Redactor::new().redact(input);
        assert!(count >= 2);
        assert!(!output.contains("secret1"));
        assert!(!output.contains("abc123"));
        assert!(!output.contains("MIIE"));
    }

    #[test]
    fn verify_no_secrets_passes_clean_text() {
        assert!(verify_no_secrets("normal log line without secrets").is_ok());
    }

    #[test]
    fn verify_no_secrets_detects_residual() {
        let result = verify_no_secrets("password=leaked_value");
        assert!(result.is_err());
    }

    #[test]
    fn verify_no_secrets_passes_after_redaction() {
        let redactor = Redactor::new();
        let (redacted, _) = redactor.redact("password=secret123");
        assert!(verify_no_secrets(&redacted).is_ok());
    }

    #[test]
    fn redact_preserves_non_secret_content() {
        let input = "log: agent started, pid=42, status=ok";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 0);
        assert_eq!(output, input);
    }

    #[test]
    fn redact_api_key_field() {
        let input = "api_key=sk-1234567890abcdef";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 1);
        assert!(!output.contains("sk-1234567890abcdef"));
    }

    #[test]
    fn redact_authorization_header() {
        let input = "authorization: Basic dXNlcjpwYXNz";
        let (output, count) = Redactor::new().redact(input);
        assert_eq!(count, 1);
        assert!(!output.contains("dXNlcjpwYXNz"));
    }
}
