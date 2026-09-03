//! 幂等键存储 (OBS-010).
//!
//! 防止重复执行副作用操作。调用方在请求中携带幂等键，
//! 服务端在窗口期内缓存响应，相同键的重复请求返回缓存结果。
//!
//! 窗口期：24 小时（86400 秒），与 HTTP 幂等键惯例一致。
//!
//! 存储：基于 heapless 环形缓冲区，容量固定。
//! 当缓冲区满时，最旧条目被驱逐（FIFO）。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::schema::StatusCode;

/// 幂等窗口期（秒）= 24 小时。
pub const IDEMPOTENCY_WINDOW_SECS: u64 = 24 * 60 * 60;

/// 幂等键最大长度。
pub const MAX_KEY_LEN: usize = 128;

/// 幂等条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyEntry {
    /// 幂等键。
    pub key: String,
    /// 缓存的状态码。
    pub status: StatusCode,
    /// 缓存的响应正文。
    pub body: String,
    /// 创建时间（Unix 秒）。
    pub created_at: u64,
}

/// 计算幂等键的内容哈希（v0.239.0，B5-02）。
///
/// 幂等键 = SM3(action ‖ payload) 的十六进制——相同内容重放命中缓存，
/// 不同内容使用相同显式键不命中（防错误重放命中）。
pub fn content_hash_key(action: &str, payload: &str) -> String {
    let mut buf = Vec::with_capacity(action.len() + payload.len() + 1);
    buf.extend_from_slice(action.as_bytes());
    buf.push(0xFF);
    buf.extend_from_slice(payload.as_bytes());
    let digest = eneros_crypto::sm3_hash(&buf);
    // WP-64.3.4：hex 编码单源。
    eneros_hex::encode_lower(&digest)
}

/// 幂等性错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyError {
    /// 键过长。
    KeyTooLong,
    /// 键为空。
    EmptyKey,
}

/// 幂等键存储。
///
/// 基于简单数组实现，容量固定。当容量满时，最旧条目被驱逐。
/// 查找是线性扫描（O(n)），适合管理 API 的低 QPS 场景。
pub struct IdempotencyStore {
    /// 存储槽位。
    entries: Vec<IdempotencyEntry>,
    /// 最大容量。
    capacity: usize,
}

impl IdempotencyStore {
    /// 创建指定容量的幂等存储。
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// 查找幂等键对应的缓存响应。
    ///
    /// 如果键存在且未过期（在窗口期内），返回缓存条目。
    /// 如果键存在但已过期，删除条目并返回 None。
    pub fn get(&mut self, key: &str, now_unix: u64) -> Option<IdempotencyEntry> {
        // 先清理过期条目
        self.evict_expired(now_unix);

        self.entries.iter().find(|e| e.key == key).cloned()
    }

    /// 存储幂等响应。
    ///
    /// 如果键已存在，覆盖旧条目。如果容量已满，驱逐最旧条目。
    pub fn put(&mut self, key: &str, status: StatusCode, body: &str, now_unix: u64) {
        if key.is_empty() {
            return;
        }

        // 如果键已存在，更新
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.status = status;
            entry.body = String::from(body);
            entry.created_at = now_unix;
            return;
        }

        // 容量满时驱逐最旧
        if self.entries.len() >= self.capacity {
            let mut oldest_idx = 0;
            let mut oldest_time = u64::MAX;
            for (i, entry) in self.entries.iter().enumerate() {
                if entry.created_at < oldest_time {
                    oldest_time = entry.created_at;
                    oldest_idx = i;
                }
            }
            self.entries.swap_remove(oldest_idx);
        }

        self.entries.push(IdempotencyEntry {
            key: key.to_string(),
            status,
            body: String::from(body),
            created_at: now_unix,
        });
    }

    /// 驱逐所有过期条目。
    fn evict_expired(&mut self, now_unix: u64) {
        let cutoff = now_unix.saturating_sub(IDEMPOTENCY_WINDOW_SECS);
        self.entries.retain(|e| e.created_at > cutoff);
    }

    /// 返回当前存储的条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 清空所有条目。
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get() {
        let mut store = IdempotencyStore::new(32);
        store.put("key-1", StatusCode::Ok, r#"{"result":"ok"}"#, 1000);

        let entry = store.get("key-1", 1001).expect("entry should exist");
        assert_eq!(entry.status, StatusCode::Ok);
        assert_eq!(entry.body, r#"{"result":"ok"}"#);
    }

    #[test]
    fn missing_key_returns_none() {
        let mut store = IdempotencyStore::new(32);
        assert!(store.get("nonexistent", 1000).is_none());
    }

    #[test]
    fn expired_entry_evicted() {
        let mut store = IdempotencyStore::new(32);
        store.put("key-1", StatusCode::Ok, "ok", 1000);

        // 窗口期内
        assert!(store
            .get("key-1", 1000 + IDEMPOTENCY_WINDOW_SECS - 1)
            .is_some());

        // 窗口期外
        assert!(store
            .get("key-1", 1000 + IDEMPOTENCY_WINDOW_SECS + 1)
            .is_none());
    }

    #[test]
    fn overwrite_existing_key() {
        let mut store = IdempotencyStore::new(32);
        store.put("key-1", StatusCode::Ok, "first", 1000);
        store.put("key-1", StatusCode::Conflict, "second", 1001);

        let entry = store.get("key-1", 1002).expect("entry should exist");
        assert_eq!(entry.status, StatusCode::Conflict);
        assert_eq!(entry.body, "second");
    }

    #[test]
    fn capacity_evicts_oldest() {
        let mut store = IdempotencyStore::new(3);
        store.put("k1", StatusCode::Ok, "1", 1000);
        store.put("k2", StatusCode::Ok, "2", 1001);
        store.put("k3", StatusCode::Ok, "3", 1002);

        // 插入第 4 个，应驱逐 k1（最旧）
        store.put("k4", StatusCode::Ok, "4", 1003);

        assert_eq!(store.len(), 3);
        assert!(store.get("k1", 1003).is_none());
        assert!(store.get("k2", 1003).is_some());
        assert!(store.get("k3", 1003).is_some());
        assert!(store.get("k4", 1003).is_some());
    }

    #[test]
    fn empty_key_ignored() {
        let mut store = IdempotencyStore::new(32);
        store.put("", StatusCode::Ok, "body", 1000);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn clear_empties_store() {
        let mut store = IdempotencyStore::new(32);
        store.put("k1", StatusCode::Ok, "1", 1000);
        store.put("k2", StatusCode::Ok, "2", 1001);
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
    }

    // ===== v0.239.0（B5-02）：幂等键内容哈希 =====

    #[test]
    fn test_content_hash_key_deterministic() {
        let k1 = content_hash_key("start", "kw=100");
        let k2 = content_hash_key("start", "kw=100");
        assert_eq!(k1, k2, "同内容同哈希");
        assert_eq!(k1.len(), 64, "SM3 hex 64 字符");
    }

    #[test]
    fn test_content_hash_key_differs_by_content() {
        let k1 = content_hash_key("start", "kw=100");
        let k2 = content_hash_key("start", "kw=200");
        let k3 = content_hash_key("stop", "kw=100");
        assert_ne!(k1, k2, "不同 payload 不同哈希");
        assert_ne!(k1, k3, "不同 action 不同哈希");
    }

    #[test]
    fn test_idempotency_with_content_hash_key() {
        // 相同内容重放 → 命中缓存；不同内容同显式键 → 不命中。
        let mut store = IdempotencyStore::new(16);
        let key1 = content_hash_key("start", "kw=100");
        store.put(&key1, StatusCode::Ok, "done", 1000);
        assert!(store.get(&key1, 1500).is_some(), "同内容命中");
        let key2 = content_hash_key("start", "kw=999");
        assert!(store.get(&key2, 1500).is_none(), "不同内容不命中");
    }
}
