//! GGUF 张量信息解析（D6）.
//!
//! 解析元数据 KV 之后的 `tensor_count` 个张量描述符。每个张量布局：
//! name（gguf_string）+ `n_dims`（u32）+ dims（u64`[n_dims]`）+ dtype（u32）+ offset（u64）。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::dtype::GgufDtype;
use crate::error::GgufError;

/// GGUF 张量描述符（仅元数据，不含权重数据）.
#[derive(Debug, Clone)]
pub struct GgufTensorInfo {
    /// 张量名称（如 "token_embd.weight"）.
    pub name: String,
    /// 维度列表（GGUF 中每维为 u64，此处截断为 u32）.
    pub dimensions: Vec<u32>,
    /// 张量数据类型.
    pub dtype: GgufDtype,
    /// 权重数据相对于数据段起始的偏移（字节）.
    pub offset: u64,
}

impl GgufTensorInfo {
    /// 从 `offset` 处解析 `tensor_count` 个张量描述符，返回 `(tensors, 已消费字节数)`.
    ///
    /// 每个张量布局：name（gguf_string）+ `n_dims`（u32）+ dims（u64`[n_dims]`）
    /// + dtype（u32）+ offset（u64）。
    pub fn parse(
        bytes: &[u8],
        offset: usize,
        tensor_count: u64,
    ) -> Result<(Vec<GgufTensorInfo>, usize), GgufError> {
        let mut tensors = Vec::new();
        let mut pos = offset;

        for _ in 0..tensor_count {
            // 解析 name（gguf_string）
            let name_len = {
                if pos + 8 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                u64::from_le_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                    bytes[pos + 4],
                    bytes[pos + 5],
                    bytes[pos + 6],
                    bytes[pos + 7],
                ]) as usize
            };
            let name_start = pos + 8;
            if name_start + name_len > bytes.len() {
                return Err(GgufError::TruncatedFile);
            }
            let name = core::str::from_utf8(&bytes[name_start..name_start + name_len])
                .map_err(GgufError::from)?
                .to_string();
            pos = name_start + name_len;

            // 解析 n_dims
            if pos + 4 > bytes.len() {
                return Err(GgufError::TruncatedFile);
            }
            let n_dims =
                u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                    as usize;
            pos += 4;

            // 解析 dimensions（GGUF 中每维为 u64，此处存储为 u32）
            let mut dimensions = Vec::new();
            for _ in 0..n_dims {
                if pos + 8 > bytes.len() {
                    return Err(GgufError::TruncatedFile);
                }
                let dim = u64::from_le_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                    bytes[pos + 4],
                    bytes[pos + 5],
                    bytes[pos + 6],
                    bytes[pos + 7],
                ]) as u32;
                dimensions.push(dim);
                pos += 8;
            }

            // 解析 dtype
            if pos + 4 > bytes.len() {
                return Err(GgufError::TruncatedFile);
            }
            let dtype_u32 =
                u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
            pos += 4;
            let dtype = GgufDtype::from_u32(dtype_u32).ok_or(GgufError::InvalidDtype(dtype_u32))?;

            // 解析 offset
            if pos + 8 > bytes.len() {
                return Err(GgufError::TruncatedFile);
            }
            let tensor_offset = u64::from_le_bytes([
                bytes[pos],
                bytes[pos + 1],
                bytes[pos + 2],
                bytes[pos + 3],
                bytes[pos + 4],
                bytes[pos + 5],
                bytes[pos + 6],
                bytes[pos + 7],
            ]);
            pos += 8;

            // AI-001：GGUF 规范要求张量数据 32 字节对齐；未对齐即拒绝，
            // 防止后续映射读取时发生未对齐访问（真实 llama.cpp 文件满足）。
            if tensor_offset % 32 != 0 {
                return Err(GgufError::MisalignedTensor {
                    name,
                    offset: tensor_offset,
                });
            }

            tensors.push(GgufTensorInfo {
                name,
                dimensions,
                dtype,
                offset: tensor_offset,
            });
        }

        Ok((tensors, pos - offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造单个 tensor 描述符（name + n_dims + dims + dtype + offset）。
    fn tensor_bytes(name: &[u8], offset: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(name.len() as u64).to_le_bytes());
        b.extend_from_slice(name);
        b.extend_from_slice(&1u32.to_le_bytes()); // n_dims
        b.extend_from_slice(&4096u64.to_le_bytes()); // dim[0]
        b.extend_from_slice(&12u32.to_le_bytes()); // Q4_K
        b.extend_from_slice(&offset.to_le_bytes());
        b
    }

    /// AI-001：32 字节对齐的 offset 通过。
    #[test]
    fn test_alignment_ok() {
        let bytes = tensor_bytes(b"t.weight", 0);
        let (tensors, _) = GgufTensorInfo::parse(&bytes, 0, 1).expect("parse");
        assert_eq!(tensors[0].offset, 0);
        let bytes2 = tensor_bytes(b"t.weight", 64);
        let (tensors2, _) = GgufTensorInfo::parse(&bytes2, 0, 1).expect("parse 64");
        assert_eq!(tensors2[0].offset, 64);
    }

    /// AI-001：未对齐 offset（31/33）→ MisalignedTensor（旧代码不检查）。
    #[test]
    fn test_alignment_rejects_unaligned() {
        for bad in [31u64, 33u64, 1u64] {
            let bytes = tensor_bytes(b"t.weight", bad);
            let r = GgufTensorInfo::parse(&bytes, 0, 1);
            assert!(
                matches!(r, Err(GgufError::MisalignedTensor { .. })),
                "offset {bad} must be rejected"
            );
        }
    }
}
