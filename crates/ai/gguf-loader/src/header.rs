//! GGUF 文件头解析（D6）.
//!
//! GGUF 头部为 24 字节定长结构（小端序）：
//! magic(4) + version(4) + tensor_count(8) + metadata_kv_count(8)。

use crate::error::GgufError;

/// GGUF 魔数 "GGUF"（小端序：0x46554747）.
pub const GGUF_MAGIC: u32 = 0x46554747;

/// GGUF 文件头（24 字节定长）.
#[derive(Debug, Clone, Copy)]
pub struct GgufHeader {
    /// 魔数，必须等于 [`GGUF_MAGIC`].
    pub magic: u32,
    /// GGUF 版本号.
    pub version: u32,
    /// 张量数量.
    pub tensor_count: u64,
    /// 元数据 KV 对数量.
    pub metadata_kv_count: u64,
}

impl GgufHeader {
    /// 从字节切片解析文件头，返回 `(header, 已消费字节数)`.
    ///
    /// GGUF 头部布局（小端序）：
    /// - magic: 4 字节 (u32)
    /// - version: 4 字节 (u32)
    /// - tensor_count: 8 字节 (u64)
    /// - metadata_kv_count: 8 字节 (u64)
    ///
    /// 共 24 字节。长度不足返回 `TruncatedFile`，魔数不匹配返回 `InvalidMagic`。
    pub fn parse(bytes: &[u8]) -> Result<(GgufHeader, usize), GgufError> {
        if bytes.len() < 24 {
            return Err(GgufError::TruncatedFile);
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != GGUF_MAGIC {
            return Err(GgufError::InvalidMagic);
        }
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let tensor_count = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        let metadata_kv_count = u64::from_le_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]);
        Ok((
            GgufHeader {
                magic,
                version,
                tensor_count,
                metadata_kv_count,
            },
            24,
        ))
    }
}
