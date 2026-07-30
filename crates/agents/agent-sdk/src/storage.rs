//! 存储 API（SDK v1.0 稳定 API）。
//!
//! 提供 Agent 本地键值存储接口。SDK 层提供 Mock 实现，
//! 真实存储由 Agent Runtime 注入（基于 littlefs2）。

use alloc::vec::Vec;

use heapless::String as HString;

use crate::agent::AgentError;

/// 存储键最大长度。
pub const MAX_KEY_LEN: usize = 128;
/// 存储值最大长度（64KB）。
pub const MAX_VALUE_LEN: usize = 65536;
/// 最大键值对数。
pub const MAX_ENTRIES: usize = 64;

/// 存储 API。
///
/// Agent 通过此结构读写本地持久化数据。
pub struct Storage {
    /// 存储条目（Mock 实现：内存键值对）
    entries: [Option<(HString<128>, Vec<u8>)>; MAX_ENTRIES],
    /// 读计数
    read_count: u64,
    /// 写计数
    write_count: u64,
}

impl Storage {
    /// 创建新的存储实例。
    pub fn new() -> Self {
        const NONE: Option<(HString<128>, Vec<u8>)> = None;
        Self {
            entries: [NONE; MAX_ENTRIES],
            read_count: 0,
            write_count: 0,
        }
    }

    /// 读取键对应的值。
    ///
    /// 键不存在时返回 `StorageError`。
    pub fn read(&self, key: &str) -> Result<Vec<u8>, AgentError> {
        if key.len() > MAX_KEY_LEN {
            return Err(AgentError::StorageError(
                HString::<128>::try_from("key too long").unwrap_or_default(),
            ));
        }
        for (k, v) in self.entries.iter().flatten() {
            if k.as_str() == key {
                return Ok(v.clone());
            }
        }
        Err(AgentError::StorageError(
            HString::<128>::try_from("key not found").unwrap_or_default(),
        ))
    }

    /// 写入键值对。
    ///
    /// 键已存在则覆盖，不存在则插入。
    pub fn write(&mut self, key: &str, data: &[u8]) -> Result<(), AgentError> {
        if key.len() > MAX_KEY_LEN {
            return Err(AgentError::StorageError(
                HString::<128>::try_from("key too long").unwrap_or_default(),
            ));
        }
        if data.len() > MAX_VALUE_LEN {
            return Err(AgentError::StorageError(
                HString::<128>::try_from("value too large").unwrap_or_default(),
            ));
        }
        let hkey = HString::<128>::try_from(key).map_err(|_| {
            AgentError::StorageError(
                HString::<128>::try_from("key conversion failed").unwrap_or_default(),
            )
        })?;

        // 查找已存在的键
        for entry in &mut self.entries {
            if let Some((k, _)) = entry {
                if k.as_str() == key {
                    *entry = Some((hkey, data.to_vec()));
                    self.write_count += 1;
                    return Ok(());
                }
            }
        }

        // 插入新键
        for entry in &mut self.entries {
            if entry.is_none() {
                *entry = Some((hkey, data.to_vec()));
                self.write_count += 1;
                return Ok(());
            }
        }

        Err(AgentError::StorageError(
            HString::<128>::try_from("storage full").unwrap_or_default(),
        ))
    }

    /// 删除键。
    ///
    /// 键不存在时返回 `StorageError`。
    pub fn delete(&mut self, key: &str) -> Result<(), AgentError> {
        for entry in &mut self.entries {
            if let Some((k, _)) = entry {
                if k.as_str() == key {
                    *entry = None;
                    return Ok(());
                }
            }
        }
        Err(AgentError::StorageError(
            HString::<128>::try_from("key not found").unwrap_or_default(),
        ))
    }

    /// 检查键是否存在。
    pub fn exists(&self, key: &str) -> bool {
        for (k, _) in self.entries.iter().flatten() {
            if k.as_str() == key {
                return true;
            }
        }
        false
    }

    /// 获取已用条目数。
    pub fn count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }

    /// 获取读计数。
    pub fn read_count(&self) -> u64 {
        self.read_count
    }

    /// 获取写计数。
    pub fn write_count(&self) -> u64 {
        self.write_count
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// STO1：写入和读取。
    #[test]
    fn sto1_write_and_read() {
        let mut storage = Storage::new();
        storage.write("key1", b"value1").expect("写入应成功");
        let val = storage.read("key1").expect("读取应成功");
        assert_eq!(val, b"value1");
    }

    /// STO2：键不存在。
    #[test]
    fn sto2_key_not_found() {
        let storage = Storage::new();
        let err = storage.read("nonexistent").expect_err("应失败");
        assert!(matches!(err, AgentError::StorageError(_)));
    }

    /// STO3：覆盖写入。
    #[test]
    fn sto3_overwrite() {
        let mut storage = Storage::new();
        storage.write("key1", b"v1").expect("写入应成功");
        storage.write("key1", b"v2").expect("覆盖应成功");
        let val = storage.read("key1").expect("读取应成功");
        assert_eq!(val, b"v2");
        assert_eq!(storage.count(), 1);
    }

    /// STO4：删除。
    #[test]
    fn sto4_delete() {
        let mut storage = Storage::new();
        storage.write("key1", b"v1").expect("写入应成功");
        storage.delete("key1").expect("删除应成功");
        assert!(!storage.exists("key1"));
        assert_eq!(storage.count(), 0);
    }

    /// STO5：删除不存在的键。
    #[test]
    fn sto5_delete_nonexistent() {
        let mut storage = Storage::new();
        let err = storage.delete("nonexistent").expect_err("应失败");
        assert!(matches!(err, AgentError::StorageError(_)));
    }

    /// STO6：exists 检查。
    #[test]
    fn sto6_exists() {
        let mut storage = Storage::new();
        assert!(!storage.exists("key1"));
        storage.write("key1", b"v1").expect("写入应成功");
        assert!(storage.exists("key1"));
    }

    /// STO7：值过大拒绝。
    #[test]
    fn sto7_value_too_large() {
        let mut storage = Storage::new();
        let big = alloc::vec![0u8; MAX_VALUE_LEN + 1];
        let err = storage.write("key1", &big).expect_err("应拒绝");
        assert!(matches!(err, AgentError::StorageError(_)));
    }

    /// STO8：存储满。
    #[test]
    fn sto8_storage_full() {
        let mut storage = Storage::new();
        for i in 0..MAX_ENTRIES {
            let key = alloc::format!("key{}", i);
            storage.write(&key, b"v").expect("写入应成功");
        }
        assert_eq!(storage.count(), MAX_ENTRIES);
        let err = storage.write("overflow", b"v").expect_err("应拒绝");
        assert!(matches!(err, AgentError::StorageError(_)));
    }
}
