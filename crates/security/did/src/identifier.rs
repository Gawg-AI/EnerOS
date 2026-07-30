//! DID 标识符生成与解析.
//!
//! 提供从 agent_id 生成 DID 标识符以及解析 DID 字符串的功能。
//!
//! # DID 格式
//! `did:eneros:<network>:<agent_id>`

use crate::types::Did;
use crate::DidError;

/// 从 agent_id 和 network 生成 DID 标识符.
///
/// # 参数
/// - `agent_id`: Agent 唯一数字标识
/// - `network`: 网络标识 (如 "main", "test")
///
/// # 示例
/// ```
/// use eneros_did::identifier::generate_did;
/// let did = generate_did(12345, "main");
/// assert_eq!(did.to_did_string(), "did:eneros:main:12345");
/// ```
pub fn generate_did(agent_id: u64, network: &str) -> Did {
    Did::new("eneros", network, &alloc::format!("{}", agent_id))
}

/// 解析 DID 字符串.
///
/// 支持格式: `did:eneros:<network>:<id>`
///
/// # 错误
/// - [`DidError::InvalidDid`][]: 格式不合法（段数不足、前缀错误、字段为空）
pub fn parse_did(did_str: &str) -> Result<Did, DidError> {
    let parts: alloc::vec::Vec<&str> = did_str.split(':').collect();
    if parts.len() != 4 {
        return Err(DidError::InvalidDid);
    }
    if parts[0] != "did" {
        return Err(DidError::InvalidDid);
    }
    let method = parts[1];
    let network = parts[2];
    let id = parts[3];
    if method.is_empty() || network.is_empty() || id.is_empty() {
        return Err(DidError::InvalidDid);
    }
    Ok(Did::new(method, network, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_did() {
        let did = generate_did(12345, "main");
        assert_eq!(did.method, "eneros");
        assert_eq!(did.network, "main");
        assert_eq!(did.id, "12345");
        assert_eq!(did.to_did_string(), "did:eneros:main:12345");
    }

    #[test]
    fn test_generate_did_test_network() {
        let did = generate_did(999, "test");
        assert_eq!(did.to_did_string(), "did:eneros:test:999");
    }

    #[test]
    fn test_parse_did_valid() {
        let did = parse_did("did:eneros:main:12345").unwrap();
        assert_eq!(did.method, "eneros");
        assert_eq!(did.network, "main");
        assert_eq!(did.id, "12345");
    }

    #[test]
    fn test_parse_did_roundtrip() {
        let original = generate_did(42, "main");
        let s = original.to_did_string();
        let parsed = parse_did(&s).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_parse_did_invalid_prefix() {
        assert_eq!(parse_did("urn:eneros:main:123"), Err(DidError::InvalidDid));
    }

    #[test]
    fn test_parse_did_too_few_parts() {
        assert_eq!(parse_did("did:eneros:main"), Err(DidError::InvalidDid));
    }

    #[test]
    fn test_parse_did_too_many_parts() {
        assert_eq!(
            parse_did("did:eneros:main:123:extra"),
            Err(DidError::InvalidDid)
        );
    }

    #[test]
    fn test_parse_did_empty_field() {
        assert_eq!(parse_did("did:eneros::123"), Err(DidError::InvalidDid));
        assert_eq!(parse_did("did::main:123"), Err(DidError::InvalidDid));
        assert_eq!(parse_did("did:eneros:main:"), Err(DidError::InvalidDid));
    }

    #[test]
    fn test_parse_did_empty_string() {
        assert_eq!(parse_did(""), Err(DidError::InvalidDid));
    }
}
