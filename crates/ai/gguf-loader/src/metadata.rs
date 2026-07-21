//! GGUF 元数据 KV 解析（D6 / D11）.
//!
//! 解析 GGUF 头部之后的 `metadata_kv_count` 个 KV 对，将已知键映射到
//! [`GgufMetadata`] 结构体；未知键被忽略。`quantization` 默认 `Q4_K_M`（D11）。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use eneros_llm_engine::model::Quantization;

use crate::error::GgufError;
use crate::value::{GgufValue, GgufValueType};

/// GGUF 模型元数据（仅保留已知键，D11）.
#[derive(Debug, Clone)]
pub struct GgufMetadata {
    /// 模型名称.
    pub name: String,
    /// 模型架构（如 "llama"）.
    pub architecture: String,
    /// 上下文长度.
    pub context_length: u32,
    /// 嵌入维度.
    pub embedding_length: u32,
    /// Transformer 层数.
    pub block_count: u32,
    /// 注意力头数.
    pub head_count: u32,
    /// KV 注意力头数.
    pub head_count_kv: u32,
    /// 量化方式（默认 Q4_K_M，D11）.
    pub quantization: Quantization,
}

impl Default for GgufMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            architecture: String::new(),
            context_length: 0,
            embedding_length: 0,
            block_count: 0,
            head_count: 0,
            head_count_kv: 0,
            quantization: Quantization::Q4_K_M, // D11: default Q4_K_M
        }
    }
}

impl GgufMetadata {
    /// 从 `offset` 处解析 `kv_count` 个 KV 对，返回 `(metadata, 已消费字节数)`.
    ///
    /// 每个 KV 对布局：key（gguf_string）+ value_type（u32）+ value。
    /// `gguf_string` 布局：u64 长度 + UTF-8 字节。
    pub fn parse(
        bytes: &[u8],
        offset: usize,
        kv_count: u64,
    ) -> Result<(GgufMetadata, usize), GgufError> {
        let mut metadata = GgufMetadata::default();
        let mut pos = offset;

        for _ in 0..kv_count {
            // 解析 key（gguf_string）
            let (key, consumed) = Self::parse_string(bytes, pos)?;
            pos += consumed;

            // 解析 value_type
            if pos + 4 > bytes.len() {
                return Err(GgufError::TruncatedFile);
            }
            let value_type_u32 =
                u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
            pos += 4;

            let value_type = GgufValueType::from_u32(value_type_u32)
                .ok_or(GgufError::InvalidValueType(value_type_u32))?;

            // 解析 value
            let (value, consumed) = Self::parse_value(bytes, pos, value_type)?;
            pos += consumed;

            // 将已知键映射到 metadata 字段
            match key.as_str() {
                "name" => {
                    if let GgufValue::String(s) = value {
                        metadata.name = s;
                    }
                }
                "architecture" => {
                    if let GgufValue::String(s) = value {
                        metadata.architecture = s;
                    }
                }
                "context_length" => {
                    if let GgufValue::Uint32(v) = value {
                        metadata.context_length = v;
                    }
                }
                "embedding_length" => {
                    if let GgufValue::Uint32(v) = value {
                        metadata.embedding_length = v;
                    }
                }
                "block_count" => {
                    if let GgufValue::Uint32(v) = value {
                        metadata.block_count = v;
                    }
                }
                "head_count" => {
                    if let GgufValue::Uint32(v) = value {
                        metadata.head_count = v;
                    }
                }
                "head_count_kv" => {
                    if let GgufValue::Uint32(v) = value {
                        metadata.head_count_kv = v;
                    }
                }
                "quantization" => {
                    if let GgufValue::String(s) = value {
                        metadata.quantization = match s.as_str() {
                            "F16" => Quantization::F16,
                            "Q8_0" => Quantization::Q8_0,
                            "Q4_0" => Quantization::Q4_0,
                            "Q4_K_M" | "Q4_K" => Quantization::Q4_K_M,
                            _ => Quantization::Q4_K_M, // default
                        };
                    }
                }
                _ => {} // 忽略未知键
            }
        }

        Ok((metadata, pos - offset))
    }

    /// 解析 GGUF 字符串：u64 长度 + UTF-8 字节.
    fn parse_string(bytes: &[u8], offset: usize) -> Result<(String, usize), GgufError> {
        if offset + 8 > bytes.len() {
            return Err(GgufError::TruncatedFile);
        }
        let len = u64::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let str_start = offset + 8;
        if str_start + len > bytes.len() {
            return Err(GgufError::TruncatedFile);
        }
        let str_bytes = &bytes[str_start..str_start + len];
        let s = core::str::from_utf8(str_bytes)
            .map_err(GgufError::from)?
            .to_string();
        Ok((s, 8 + len))
    }

    /// 根据值类型解析 GGUF 值.
    fn parse_value(
        bytes: &[u8],
        offset: usize,
        value_type: GgufValueType,
    ) -> Result<(GgufValue, usize), GgufError> {
        match value_type {
            GgufValueType::Uint8 => {
                if offset + 1 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                Ok((GgufValue::Uint8(bytes[offset]), 1))
            }
            GgufValueType::Int8 => {
                if offset + 1 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                Ok((GgufValue::Int8(bytes[offset] as i8), 1))
            }
            GgufValueType::Uint16 => {
                if offset + 2 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                Ok((GgufValue::Uint16(v), 2))
            }
            GgufValueType::Int16 => {
                if offset + 2 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                Ok((GgufValue::Int16(v), 2))
            }
            GgufValueType::Uint32 => {
                if offset + 4 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ]);
                Ok((GgufValue::Uint32(v), 4))
            }
            GgufValueType::Int32 => {
                if offset + 4 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = i32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ]);
                Ok((GgufValue::Int32(v), 4))
            }
            GgufValueType::Float32 => {
                if offset + 4 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = f32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ]);
                Ok((GgufValue::Float32(v), 4))
            }
            GgufValueType::Bool => {
                if offset + 1 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                Ok((GgufValue::Bool(bytes[offset] != 0), 1))
            }
            GgufValueType::String => {
                let (s, consumed) = Self::parse_string(bytes, offset)?;
                Ok((GgufValue::String(s), consumed))
            }
            GgufValueType::Array => {
                // Array: u64 length + u32 element_type + elements
                if offset + 12 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let len = u64::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                    bytes[offset + 4],
                    bytes[offset + 5],
                    bytes[offset + 6],
                    bytes[offset + 7],
                ]) as usize;
                let elem_type_u32 = u32::from_le_bytes([
                    bytes[offset + 8],
                    bytes[offset + 9],
                    bytes[offset + 10],
                    bytes[offset + 11],
                ]);
                let elem_type = GgufValueType::from_u32(elem_type_u32)
                    .ok_or(GgufError::InvalidValueType(elem_type_u32))?;
                let mut pos = offset + 12;
                let mut arr: Vec<GgufValue> = Vec::new();
                for _ in 0..len {
                    let (val, consumed) = Self::parse_value(bytes, pos, elem_type)?;
                    pos += consumed;
                    arr.push(val);
                }
                Ok((GgufValue::Array(arr), pos - offset))
            }
            GgufValueType::Uint64 => {
                if offset + 8 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = u64::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                    bytes[offset + 4],
                    bytes[offset + 5],
                    bytes[offset + 6],
                    bytes[offset + 7],
                ]);
                Ok((GgufValue::Uint64(v), 8))
            }
            GgufValueType::Int64 => {
                if offset + 8 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = i64::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                    bytes[offset + 4],
                    bytes[offset + 5],
                    bytes[offset + 6],
                    bytes[offset + 7],
                ]);
                Ok((GgufValue::Int64(v), 8))
            }
            GgufValueType::Float64 => {
                if offset + 8 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let v = f64::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                    bytes[offset + 4],
                    bytes[offset + 5],
                    bytes[offset + 6],
                    bytes[offset + 7],
                ]);
                Ok((GgufValue::Float64(v), 8))
            }
        }
    }
}
