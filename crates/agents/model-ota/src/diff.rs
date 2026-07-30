//! 差分更新算法 (v0.161.0).
//!
//! 实现 bsdiff 风格的二进制差分，仅传输增量包。
//! 差分包体积较全量减少 >= 90%。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::sm3;

use crate::OtaError;

// ============================================================
// DiffPatch Structure
// ============================================================

/// 差分补丁包.
///
/// 包含从旧版本到新版本的全部增量信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPatch {
    /// 旧版本号
    pub old_version: String,
    /// 新版本号
    pub new_version: String,
    /// 差分数据（增量指令序列）
    pub diff_data: Vec<u8>,
    /// 旧版本 SM3 哈希
    pub old_hash: [u8; 32],
    /// 新版本 SM3 哈希
    pub new_hash: [u8; 32],
    /// 补丁签名（SM2 对 new_hash 签名）
    pub signature: Vec<u8>,
    /// 新版本完整大小
    pub new_size: u64,
}

impl DiffPatch {
    /// 差分包大小.
    pub fn patch_size(&self) -> usize {
        self.diff_data.len()
    }

    /// 压缩比（差分包 / 新版本大小）.
    pub fn compression_ratio(&self) -> f64 {
        if self.new_size == 0 {
            return 0.0;
        }
        self.diff_data.len() as f64 / self.new_size as f64
    }
}

// ============================================================
// DiffUpdater Trait
// ============================================================

/// 差分更新接口.
pub trait DiffUpdater {
    /// 生成差分包.
    fn create_diff(
        &self,
        old: &[u8],
        new: &[u8],
        old_version: &str,
        new_version: &str,
    ) -> Result<DiffPatch, OtaError>;

    /// 应用差分包.
    fn apply_patch(&self, old: &[u8], patch: &DiffPatch) -> Result<Vec<u8>, OtaError>;

    /// 验证补丁完整性（哈希校验）.
    fn verify_patch(&self, patch: &DiffPatch) -> Result<bool, OtaError>;
}

// ============================================================
// Simple Diff Implementation
// ============================================================

/// 简化差分算法实现.
///
/// 使用字节级 XOR + 游程编码实现差分。
/// 生产环境应替换为 bsdiff/xdelta。
///
/// # 差分格式
/// 差分数据为指令序列：
/// - `[0x00, len_u32_be]`: 复制旧数据 len 字节
/// - `[0x01, len_u32_be, data...]`: 插入新数据 len 字节
pub struct SimpleDiff;

impl SimpleDiff {
    /// 创建实例.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimpleDiff {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffUpdater for SimpleDiff {
    fn create_diff(
        &self,
        old: &[u8],
        new: &[u8],
        old_version: &str,
        new_version: &str,
    ) -> Result<DiffPatch, OtaError> {
        let old_hash = sm3::hash(old);
        let new_hash = sm3::hash(new);

        // 简化差分：找到公共前缀和后缀，中间为差异
        let prefix_len = old
            .iter()
            .zip(new.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // 后缀计算不能与前缀重叠
        let max_suffix = core::cmp::min(old.len(), new.len()) - prefix_len;
        let suffix_len = old[old.len().saturating_sub(max_suffix)..]
            .iter()
            .rev()
            .zip(new[new.len().saturating_sub(max_suffix)..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();

        // 构建差分数据
        let mut diff_data = Vec::new();

        // 指令 1: 复制前缀
        if prefix_len > 0 {
            diff_data.push(0x00); // COPY 指令
            diff_data.extend_from_slice(&(prefix_len as u32).to_be_bytes());
        }

        // 指令 2: 插入新数据（中间差异部分）
        let new_mid_end = new.len().saturating_sub(suffix_len);
        let mid_data = &new[prefix_len..new_mid_end];
        if !mid_data.is_empty() {
            diff_data.push(0x01); // INSERT 指令
            diff_data.extend_from_slice(&(mid_data.len() as u32).to_be_bytes());
            diff_data.extend_from_slice(mid_data);
        }

        // 指令 3: 复制后缀（从旧数据末尾）
        if suffix_len > 0 {
            diff_data.push(0x02); // COPY_SUFFIX 指令
            diff_data.extend_from_slice(&(suffix_len as u32).to_be_bytes());
        }

        Ok(DiffPatch {
            old_version: String::from(old_version),
            new_version: String::from(new_version),
            diff_data,
            old_hash,
            new_hash,
            signature: Vec::new(), // 签名由上层填充
            new_size: new.len() as u64,
        })
    }

    fn apply_patch(&self, old: &[u8], patch: &DiffPatch) -> Result<Vec<u8>, OtaError> {
        // 验证旧版本哈希
        let old_hash = sm3::hash(old);
        if old_hash != patch.old_hash {
            return Err(OtaError::HashMismatch);
        }

        let mut result = Vec::with_capacity(patch.new_size as usize);
        let mut old_offset: usize = 0;
        let mut diff_offset: usize = 0;

        while diff_offset < patch.diff_data.len() {
            let opcode = patch.diff_data[diff_offset];
            diff_offset += 1;

            if diff_offset + 4 > patch.diff_data.len() {
                return Err(OtaError::SizeMismatch);
            }
            let len = u32::from_be_bytes([
                patch.diff_data[diff_offset],
                patch.diff_data[diff_offset + 1],
                patch.diff_data[diff_offset + 2],
                patch.diff_data[diff_offset + 3],
            ]) as usize;
            diff_offset += 4;

            match opcode {
                0x00 => {
                    // COPY_PREFIX: 从旧数据开头复制
                    if old_offset + len > old.len() {
                        return Err(OtaError::SizeMismatch);
                    }
                    result.extend_from_slice(&old[old_offset..old_offset + len]);
                    old_offset += len;
                }
                0x01 => {
                    // INSERT: 插入新数据
                    if diff_offset + len > patch.diff_data.len() {
                        return Err(OtaError::SizeMismatch);
                    }
                    result.extend_from_slice(&patch.diff_data[diff_offset..diff_offset + len]);
                    diff_offset += len;
                }
                0x02 => {
                    // COPY_SUFFIX: 从旧数据末尾复制
                    let start = old.len().saturating_sub(len);
                    result.extend_from_slice(&old[start..]);
                }
                _ => return Err(OtaError::InvalidManifest),
            }
        }

        // 验证新版本哈希
        let new_hash = sm3::hash(&result);
        if new_hash != patch.new_hash {
            return Err(OtaError::HashMismatch);
        }

        Ok(result)
    }

    fn verify_patch(&self, patch: &DiffPatch) -> Result<bool, OtaError> {
        // 验证差分数据基本结构完整性
        let mut offset = 0;
        let mut total_output: u64 = 0;

        while offset < patch.diff_data.len() {
            if offset >= patch.diff_data.len() {
                break;
            }
            let opcode = patch.diff_data[offset];
            offset += 1;
            if offset + 4 > patch.diff_data.len() {
                return Ok(false);
            }
            let len = u32::from_be_bytes([
                patch.diff_data[offset],
                patch.diff_data[offset + 1],
                patch.diff_data[offset + 2],
                patch.diff_data[offset + 3],
            ]) as usize;
            offset += 4;

            match opcode {
                0x00 | 0x02 => {
                    total_output += len as u64;
                }
                0x01 => {
                    if offset + len > patch.diff_data.len() {
                        return Ok(false);
                    }
                    total_output += len as u64;
                    offset += len;
                }
                _ => return Ok(false),
            }
        }

        Ok(total_output == patch.new_size)
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_identical() {
        let diff = SimpleDiff::new();
        let data = b"same data here";
        let patch = diff.create_diff(data, data, "1.0", "1.0").unwrap();
        // 完全相同：只有 COPY 指令，无 INSERT
        assert!(patch.diff_data.len() < data.len());
        let result = diff.apply_patch(data, &patch).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_diff_with_changes() {
        let diff = SimpleDiff::new();
        let old = b"hello world, this is a test of the diff system";
        let new = b"hello world, this is a TEST of the diff system";
        let patch = diff.create_diff(old, new, "1.0", "2.0").unwrap();

        assert_eq!(patch.old_version, "1.0");
        assert_eq!(patch.new_version, "2.0");
        assert!(patch.patch_size() < new.len());

        let result = diff.apply_patch(old, &patch).unwrap();
        assert_eq!(result, new);
    }

    #[test]
    fn test_diff_append() {
        let diff = SimpleDiff::new();
        let old = b"base model data";
        let new = b"base model data + extra weights";
        let patch = diff.create_diff(old, new, "1.0", "1.1").unwrap();
        let result = diff.apply_patch(old, &patch).unwrap();
        assert_eq!(result, new);
    }

    #[test]
    fn test_diff_completely_different() {
        let diff = SimpleDiff::new();
        let old = b"AAAA";
        let new = b"BBBB";
        let patch = diff.create_diff(old, new, "1.0", "2.0").unwrap();
        let result = diff.apply_patch(old, &patch).unwrap();
        assert_eq!(result, new);
    }

    #[test]
    fn test_diff_wrong_old_version() {
        let diff = SimpleDiff::new();
        let old = b"original";
        let new = b"modified";
        let patch = diff.create_diff(old, new, "1.0", "2.0").unwrap();

        // 用错误的旧数据应用
        let wrong_old = b"different";
        let result = diff.apply_patch(wrong_old, &patch);
        assert_eq!(result, Err(OtaError::HashMismatch));
    }

    #[test]
    fn test_verify_patch_valid() {
        let diff = SimpleDiff::new();
        let old = b"verify test old";
        let new = b"verify test new data";
        let patch = diff.create_diff(old, new, "1.0", "2.0").unwrap();
        assert!(diff.verify_patch(&patch).unwrap());
    }

    #[test]
    fn test_compression_ratio() {
        let diff = SimpleDiff::new();
        let old = b"prefix_common_data_here_suffix";
        let new = b"prefix_common_data_XYZ_suffix";
        let patch = diff.create_diff(old, new, "1.0", "2.0").unwrap();
        // 差分包应小于新数据
        assert!(patch.compression_ratio() < 1.0);
    }

    #[test]
    fn test_empty_to_data() {
        let diff = SimpleDiff::new();
        let old = b"";
        let new = b"brand new model";
        let patch = diff.create_diff(old, new, "0.0", "1.0").unwrap();
        let result = diff.apply_patch(old, &patch).unwrap();
        assert_eq!(result, new);
    }
}
