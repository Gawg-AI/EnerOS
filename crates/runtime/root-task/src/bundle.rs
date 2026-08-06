//! init bundle v2 解析器（P0-2，v0.199.0）.
//!
//! 解析由 `xtask` 的 `compose_init_bundle`（SVC-002 schema）打包的 init
//! bundle：`ENRINIT2` magic + 64 B header + 96 B/module 表 + ELF payload。
//! root-task 在 `_start` 中消费 BootInfo 后，从 `init_bundle_phys` 解析
//! bundle，得到服务清单（名称/类型/哈希），供 EL0 spawn 与心跳监控使用。
//!
//! 布局（与 compose 完全对齐）：
//!
//! ```text
//! 0x00  magic  "ENRINIT2"           (8 B)
//! 0x08  version u16 = 2             (2 B)
//! 0x0A  header_len u16 = 64         (2 B)
//! 0x0C  total_len u32               (4 B)
//! 0x10  source_date_epoch u64       (8 B)
//! 0x18  module_count u32            (4 B)
//! 0x20  payload_sha256              (32 B)
//! 0x40  module table (96 B × n)
//!       0x00 kind u32 | 0x04 flags u32 | 0x08 offset u64 | 0x10 len u64
//!       0x18 name (32 B) | 0x38 sha256 (32 B)
//! 0x40 + 96n  payload
//! ```

use alloc::string::String;
use alloc::vec::Vec;

extern crate alloc;

/// init bundle magic。
pub const INIT_BUNDLE_MAGIC: &[u8; 8] = b"ENRINIT2";
/// bundle 版本。
pub const INIT_BUNDLE_VERSION: u16 = 2;
/// header 长度。
pub const INIT_BUNDLE_HEADER_LEN: usize = 64;
/// 每模块表项长度。
pub const INIT_BUNDLE_ENTRY_LEN: usize = 96;
/// 模块名最大长度。
pub const INIT_BUNDLE_NAME_MAX: usize = 32;

/// 模块 kind（与 compose 的 `(kind, flags, ...)` 对应）。
pub const MODULE_KIND_ROOT: u32 = 1;
pub const MODULE_KIND_SERVICE: u32 = 2;
pub const MODULE_KIND_DRIVER: u32 = 3;

/// init bundle 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleError {
    /// 长度不足（< header）。
    TooShort,
    /// magic 不匹配。
    BadMagic,
    /// 版本不支持。
    BadVersion,
    /// header 长度/表长度不一致。
    BadLayout,
    /// payload 总长越界。
    Truncated,
    /// 模块名非法（超长/含 NUL）。
    BadName,
}

/// bundle 中的一个模块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleModule {
    /// 模块类型（root/service/driver）。
    pub kind: u32,
    /// 标志。
    pub flags: u32,
    /// 模块名（如 "svc-hello"）。
    pub name: String,
    /// ELF 字节。
    pub blob: Vec<u8>,
    /// 模块 sha256（compose 写入的摘要）。
    pub sha256: [u8; 32],
}

/// 解析后的 init bundle。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitBundle {
    /// bundle 总长。
    pub total_len: u32,
    /// 构建时间戳。
    pub source_date_epoch: u64,
    /// payload 整体 sha256。
    pub payload_sha256: [u8; 32],
    /// 模块清单。
    pub modules: Vec<BundleModule>,
}

impl InitBundle {
    /// 从原始字节解析 init bundle v2。
    pub fn parse(raw: &[u8]) -> Result<Self, BundleError> {
        if raw.len() < INIT_BUNDLE_HEADER_LEN {
            return Err(BundleError::TooShort);
        }
        if &raw[..8] != INIT_BUNDLE_MAGIC {
            return Err(BundleError::BadMagic);
        }
        let version = u16::from_le_bytes([raw[8], raw[9]]);
        if version != INIT_BUNDLE_VERSION {
            return Err(BundleError::BadVersion);
        }
        let header_len = u16::from_le_bytes([raw[10], raw[11]]) as usize;
        if header_len != INIT_BUNDLE_HEADER_LEN {
            return Err(BundleError::BadLayout);
        }
        let total_len = u32::from_le_bytes(raw[12..16].try_into().unwrap()) as usize;
        let source_date_epoch = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        let module_count = u32::from_le_bytes(raw[24..28].try_into().unwrap()) as usize;
        let mut payload_sha256 = [0u8; 32];
        payload_sha256.copy_from_slice(&raw[32..64]);

        let table_len = INIT_BUNDLE_HEADER_LEN + module_count * INIT_BUNDLE_ENTRY_LEN;
        if table_len > raw.len() || total_len > raw.len() || total_len < table_len {
            return Err(BundleError::Truncated);
        }

        let mut modules = Vec::with_capacity(module_count);
        for index in 0..module_count {
            let entry = INIT_BUNDLE_HEADER_LEN + index * INIT_BUNDLE_ENTRY_LEN;
            let kind = u32::from_le_bytes(raw[entry..entry + 4].try_into().unwrap());
            let flags = u32::from_le_bytes(raw[entry + 4..entry + 8].try_into().unwrap());
            let offset =
                u64::from_le_bytes(raw[entry + 8..entry + 16].try_into().unwrap()) as usize;
            let len = u64::from_le_bytes(raw[entry + 16..entry + 24].try_into().unwrap()) as usize;
            let name_raw = &raw[entry + 24..entry + 24 + INIT_BUNDLE_NAME_MAX];
            let name_end = name_raw.iter().position(|&b| b == 0).unwrap_or(name_raw.len());
            if name_end == 0 {
                return Err(BundleError::BadName);
            }
            let name = core::str::from_utf8(&name_raw[..name_end])
                .map_err(|_| BundleError::BadName)?
                .to_string();
            let mut sha256 = [0u8; 32];
            sha256.copy_from_slice(&raw[entry + 56..entry + 88]);

            let blob_start = offset;
            let blob_end = offset.checked_add(len).ok_or(BundleError::Truncated)?;
            if blob_end > raw.len() {
                return Err(BundleError::Truncated);
            }
            modules.push(BundleModule {
                kind,
                flags,
                name,
                blob: raw[blob_start..blob_end].to_vec(),
                sha256,
            });
        }

        Ok(Self {
            total_len: total_len as u32,
            source_date_epoch,
            payload_sha256,
            modules,
        })
    }

    /// 返回指定 kind 的模块名列表。
    pub fn names_of_kind(&self, kind: u32) -> Vec<&str> {
        self.modules
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.name.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// 构造一个最小 bundle（对齐 compose 布局）。
    fn build_bundle(modules: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
        let table_len = INIT_BUNDLE_HEADER_LEN + modules.len() * INIT_BUNDLE_ENTRY_LEN;
        let mut payload_len = 0usize;
        for (_, _, _, blob) in modules {
            payload_len += blob.len();
        }
        let total = table_len + payload_len;
        let mut bytes = vec![0u8; total];
        bytes[..8].copy_from_slice(INIT_BUNDLE_MAGIC);
        bytes[8..10].copy_from_slice(&INIT_BUNDLE_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&(INIT_BUNDLE_HEADER_LEN as u16).to_le_bytes());
        bytes[12..16].copy_from_slice(&(total as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&1700000000u64.to_le_bytes());
        bytes[24..28].copy_from_slice(&(modules.len() as u32).to_le_bytes());
        let mut cursor = table_len;
        for (index, (name, kind, flags, blob)) in modules.iter().enumerate() {
            let entry = INIT_BUNDLE_HEADER_LEN + index * INIT_BUNDLE_ENTRY_LEN;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[entry + 8..entry + 16].copy_from_slice(&(cursor as u64).to_le_bytes());
            bytes[entry + 16..entry + 24].copy_from_slice(&(blob.len() as u64).to_le_bytes());
            let nb = name.as_bytes();
            bytes[entry + 24..entry + 24 + nb.len()].copy_from_slice(nb);
            // sha256 占位（全零；解析器不校验摘要）。
            bytes[cursor..cursor + blob.len()].copy_from_slice(blob);
            cursor += blob.len();
        }
        bytes
    }

    #[test]
    fn test_parse_valid_bundle() {
        let raw = build_bundle(&[
            ("root-task", MODULE_KIND_ROOT, 31, &[0x7F, b'E', b'L', b'F']),
            ("svc-hello", MODULE_KIND_SERVICE, 20, &[1, 2, 3, 4]),
            ("svc-drv-rng", MODULE_KIND_DRIVER, 12, &[9, 9]),
        ]);
        let bundle = InitBundle::parse(&raw).expect("parse");
        assert_eq!(bundle.total_len, raw.len() as u32);
        assert_eq!(bundle.source_date_epoch, 1700000000);
        assert_eq!(bundle.modules.len(), 3);
        assert_eq!(bundle.modules[0].name, "root-task");
        assert_eq!(bundle.modules[1].blob, vec![1, 2, 3, 4]);
        assert_eq!(bundle.names_of_kind(MODULE_KIND_SERVICE), vec!["svc-hello"]);
        assert_eq!(bundle.names_of_kind(MODULE_KIND_DRIVER), vec!["svc-drv-rng"]);
    }

    #[test]
    fn test_parse_bad_magic() {
        let mut raw = build_bundle(&[("svc-hello", MODULE_KIND_SERVICE, 20, &[1])]);
        raw[0] = b'X';
        assert_eq!(InitBundle::parse(&raw), Err(BundleError::BadMagic));
    }

    #[test]
    fn test_parse_truncated() {
        let raw = build_bundle(&[("svc-hello", MODULE_KIND_SERVICE, 20, &[1, 2, 3])]);
        let truncated = &raw[..raw.len() - 2];
        assert_eq!(InitBundle::parse(truncated), Err(BundleError::Truncated));
    }

    #[test]
    fn test_parse_too_short() {
        assert_eq!(InitBundle::parse(&[0u8; 10]), Err(BundleError::TooShort));
    }
}
