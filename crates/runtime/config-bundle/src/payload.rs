//! canonical payload 编解码（蓝图 41：运行时零信任解码面）。
//!
//! 格式：连续 `field_id(1) | len(2, LE) | value` 序列。
//! - 编码端（host 编译 / 测试）拒绝未知字段名、重复字段、全零密钥；
//! - 解码端（target no_std）拒绝未知 field id、重复 field id、长度越界、
//!   截断——任何歧义返回 [`ConfigError`]，不得静默默认。
//!
//! `field_id` 从 1 起；0 保留。

use crate::{ConfigError, ConfigId};

/// 字段引用抽象：`(u8, &[u8])` / `(u8, Vec<u8>)` 及引用形式均可编码。
pub trait AsField {
    fn field_id(&self) -> u8;
    fn field_bytes(&self) -> &[u8];
}

impl AsField for (u8, &[u8]) {
    fn field_id(&self) -> u8 {
        self.0
    }
    fn field_bytes(&self) -> &[u8] {
        self.1
    }
}

impl AsField for (u8, alloc::vec::Vec<u8>) {
    fn field_id(&self) -> u8 {
        self.0
    }
    fn field_bytes(&self) -> &[u8] {
        &self.1
    }
}

impl AsField for &(u8, alloc::vec::Vec<u8>) {
    fn field_id(&self) -> u8 {
        self.0
    }
    fn field_bytes(&self) -> &[u8] {
        &self.1
    }
}

impl AsField for &(u8, &[u8]) {
    fn field_id(&self) -> u8 {
        self.0
    }
    fn field_bytes(&self) -> &[u8] {
        self.1
    }
}

impl<const N: usize> AsField for &(u8, &[u8; N]) {
    fn field_id(&self) -> u8 {
        self.0
    }
    fn field_bytes(&self) -> &[u8] {
        self.1
    }
}

/// 单条 payload 编码（fields 必须已按 field_id 升序且无重复）。
pub fn encode_payload<I>(fields: I) -> alloc::vec::Vec<u8>
where
    I: IntoIterator,
    I::Item: AsField,
{
    let mut out = alloc::vec::Vec::with_capacity(64);
    for field in fields {
        let id = field.field_id();
        let value = field.field_bytes();
        debug_assert!(id != 0, "field id 0 reserved");
        out.push(id);
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(value);
    }
    out
}

/// 严格解码游标：逐字段读取，拒绝截断/长度越界。
pub struct PayloadCursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> PayloadCursor<'a> {
    pub fn new(buf: &'a [u8]) -> Result<Self, ConfigError> {
        Ok(PayloadCursor { buf, pos: 0 })
    }

    /// 读取下一个字段；`Ok(None)` = 干净结束（无尾随字节）。
    pub fn next_field(&mut self) -> Result<Option<(u8, &'a [u8])>, ConfigError> {
        if self.pos == self.buf.len() {
            return Ok(None);
        }
        if self.pos + 3 > self.buf.len() {
            return Err(ConfigError::WireFormat("payload-truncated"));
        }
        let id = self.buf[self.pos];
        if id == 0 {
            return Err(ConfigError::WireFormat("payload-reserved-field-id"));
        }
        let len = u16::from_le_bytes([self.buf[self.pos + 1], self.buf[self.pos + 2]]) as usize;
        self.pos += 3;
        if self.pos + len > self.buf.len() {
            return Err(ConfigError::WireFormat("payload-field-overrun"));
        }
        let value = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(Some((id, value)))
    }
}

/// 严格解码为字段表：未知 id（> `max_field`）与重复 id 一律错误。
/// 调用方随后按 schema 逐字段取用（缺失字段由调用方判定必填/可选）。
pub fn decode_fields(
    buf: &[u8],
    config: &ConfigId,
    max_field: u8,
) -> Result<alloc::vec::Vec<(u8, alloc::vec::Vec<u8>)>, ConfigError> {
    let mut cur = PayloadCursor::new(buf)?;
    let mut out: alloc::vec::Vec<(u8, alloc::vec::Vec<u8>)> = alloc::vec::Vec::new();
    while let Some((id, value)) = cur.next_field()? {
        if id > max_field {
            return Err(ConfigError::UnknownField {
                config: config.clone(),
                field: id,
            });
        }
        if out.iter().any(|(f, _)| *f == id) {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: id,
            });
        }
        out.push((id, value.to_vec()));
    }
    Ok(out)
}

/// 字段取用辅助：取指定 id 的值（缺失返回 None）。
pub fn field(fields: &[(u8, alloc::vec::Vec<u8>)], id: u8) -> Option<&[u8]> {
    fields
        .iter()
        .find(|(f, _)| *f == id)
        .map(|(_, v)| v.as_slice())
}

/// u64 LE 读取（长度不足 → InvalidValue）。
pub fn field_u64(
    fields: &[(u8, alloc::vec::Vec<u8>)],
    config: &ConfigId,
    id: u8,
    name: &'static str,
) -> Result<Option<u64>, ConfigError> {
    match field(fields, id) {
        None => Ok(None),
        Some(bytes) => {
            if bytes.len() != 8 {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: name,
                    reason: "expected-u64-le".into(),
                });
            }
            Ok(Some(u64::from_le_bytes(bytes.try_into().unwrap())))
        }
    }
}

/// u32 LE 读取。
pub fn field_u32(
    fields: &[(u8, alloc::vec::Vec<u8>)],
    config: &ConfigId,
    id: u8,
    name: &'static str,
) -> Result<Option<u32>, ConfigError> {
    match field(fields, id) {
        None => Ok(None),
        Some(bytes) => {
            if bytes.len() != 4 {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: name,
                    reason: "expected-u32-le".into(),
                });
            }
            Ok(Some(u32::from_le_bytes(bytes.try_into().unwrap())))
        }
    }
}

/// u8 读取。
pub fn field_u8(
    fields: &[(u8, alloc::vec::Vec<u8>)],
    config: &ConfigId,
    id: u8,
    name: &'static str,
) -> Result<Option<u8>, ConfigError> {
    match field(fields, id) {
        None => Ok(None),
        Some(bytes) => {
            if bytes.len() != 1 {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: name,
                    reason: "expected-u8".into(),
                });
            }
            Ok(Some(bytes[0]))
        }
    }
}

/// 定长 32B 读取（digest/密钥）。
pub fn field_bytes32(
    fields: &[(u8, alloc::vec::Vec<u8>)],
    config: &ConfigId,
    id: u8,
    name: &'static str,
) -> Result<Option<[u8; 32]>, ConfigError> {
    match field(fields, id) {
        None => Ok(None),
        Some(bytes) => {
            if bytes.len() != 32 {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: name,
                    reason: "expected-32-bytes".into(),
                });
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(bytes);
            Ok(Some(out))
        }
    }
}

/// 十六进制解码（配置中 hex 字符串 → 字节）。
pub fn hex_decode(
    s: &str,
    config: &ConfigId,
    name: &'static str,
) -> Result<alloc::vec::Vec<u8>, ConfigError> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: name,
            reason: "expected-hex".into(),
        });
    }
    let mut out = alloc::vec::Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16).unwrap() as u8;
        let lo = (bytes[i + 1] as char).to_digit(16).unwrap() as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}
