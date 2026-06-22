//! 共享状态存储模块（v0.26.0 — Task 3 + Task 8）
//!
//! 应用级复制引擎：在双节点间复制共享状态（SCADA 缓存、Agent 状态、命令历史、配置）。
//! 提供冲突检测与解决策略。
//!
//! ## 架构
//!
//! - 主节点通过 [`SharedStore::put`] 写入数据，触发复制回调将数据发送到备节点
//! - 备节点通过 [`SharedStore::replicate`] 接收主节点的复制数据
//! - 当版本号相同但内容不同时检测到冲突，按 [`ConflictResolution`] 策略解决
//! - 存储配额（[`StorageQuota`]）限制最大条目数和字节数
//!
//! ## 冲突解决
//!
//! - `PrimaryWins`: 主节点数据获胜
//! - `TimestampWins`: 时间戳最新的获胜（平局用 node_id 字典序 tiebreaker）
//! - `VersionWins`: 版本号最高的获胜（版本相等回退到 TimestampWins + node_id tiebreaker）
//!
//! ## v0.26.0 新增
//!
//! - **服务降级模式**（[`SharedStore::is_readonly`] / [`SharedStore::set_readonly`]）：
//!   备节点只读保护，FailoverEngine 切换/回切时切换只读状态，防止双主冲突
//! - **持久化**（snapshot + WAL）：ha-daemon 重启后通过 [`SharedStore::load_from_disk`]
//!   恢复共享状态。WAL 记录数达到 `WAL_SNAPSHOT_THRESHOLD` 时自动触发快照

use crate::ha::NodeRole;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

/// WAL 记录数阈值，达到后自动触发快照（v0.26.0 — Task 8）
const WAL_SNAPSHOT_THRESHOLD: u64 = 1000;

/// 复制回调类型（写入时触发，用于将数据复制到备节点）
type ReplicateCallback = Arc<RwLock<Option<Box<dyn Fn(StorageEntry) + Send + Sync>>>>;

/// 存储条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    /// 键
    pub key: String,
    /// 值（JSON 格式）
    pub value: serde_json::Value,
    /// 写入时间戳
    pub timestamp: i64,
    /// 写入节点 ID
    pub node_id: String,
    /// 版本号（每次写入递增）
    pub version: u64,
}

/// 冲突解决策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// 主节点优先
    #[default]
    PrimaryWins,
    /// 时间戳优先（最新写入获胜）
    TimestampWins,
    /// 版本号优先（最高版本获胜）
    VersionWins,
}

/// 存储配额
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageQuota {
    /// 最大条目数
    pub max_entries: usize,
    /// 最大字节数
    pub max_bytes: usize,
}

impl Default for StorageQuota {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_bytes: 100 * 1024 * 1024, // 100MB
        }
    }
}

/// 存储错误
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// 配额超限
    #[error("quota exceeded")]
    QuotaExceeded,
    /// 序列化错误
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    /// 持久化 IO 错误（v0.26.0 — Task 8）
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// WAL 操作记录（v0.26.0 — Task 8）
///
/// 用于持久化 SharedStore 的写操作，重启后通过 [`SharedStore::load_from_disk`] 重放。
/// 序列化为 JSON Lines 追加到 `wal_path`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WalOperation {
    /// 写入操作（主节点 put）
    Put {
        key: String,
        value: serde_json::Value,
        timestamp: i64,
        version: u64,
        node_id: String,
    },
    /// 删除操作
    Delete {
        key: String,
        timestamp: i64,
    },
    /// 复制操作（备节点 replicate 接收）
    Replicate {
        key: String,
        value: serde_json::Value,
        timestamp: i64,
        version: u64,
        node_id: String,
    },
}

/// 共享状态存储
///
/// 在双节点高可用架构中维护共享状态（SCADA 缓存、Agent 状态、命令历史、配置）。
/// 主节点通过 [`SharedStore::put`] 写入并触发复制回调；备节点通过
/// [`SharedStore::replicate`] 接收复制数据，自动检测并解决冲突。
///
/// v0.26.0 新增：
/// - 只读模式（[`SharedStore::is_readonly`]）：备节点默认只读，FailoverEngine 接管后置为可写
/// - 持久化（[`SharedStore::with_persistence`]）：配置 snapshot/wal 路径后，写操作自动追加 WAL
pub struct SharedStore {
    /// 本地存储
    entries: Arc<RwLock<HashMap<String, StorageEntry>>>,
    /// 本节点 ID
    node_id: String,
    /// 本节点角色（运行时可变，例如主备切换后通过 [`SharedStore::update_role`] 更新）
    role: Arc<RwLock<NodeRole>>,
    /// 冲突解决策略
    conflict_resolution: ConflictResolution,
    /// 存储配额
    quota: StorageQuota,
    /// 当前总字节数（所有条目 JSON 序列化后字节数之和，用于 O(1) 配额检查）
    total_bytes: AtomicUsize,
    /// 复制回调（当写入时调用，用于将数据复制到备节点）
    replicate_callback: ReplicateCallback,
    /// 只读标志（v0.26.0 — Task 3）：备节点只读保护，防止双主冲突
    is_readonly: Arc<AtomicBool>,
    /// 快照文件路径（v0.26.0 — Task 8）：None 表示未启用持久化
    snapshot_path: Option<PathBuf>,
    /// WAL 文件路径（v0.26.0 — Task 8）：None 表示未启用持久化
    wal_path: Option<PathBuf>,
    /// WAL 记录数（达到 [`WAL_SNAPSHOT_THRESHOLD`] 触发自动快照）
    wal_count: AtomicU64,
}

impl SharedStore {
    /// 创建共享存储
    ///
    /// # 参数
    /// - `node_id`: 本节点 ID
    /// - `role`: 本节点角色（Primary/Secondary）
    /// - `conflict_resolution`: 冲突解决策略
    /// - `quota`: 存储配额
    ///
    /// v0.26.0：根据初始角色设置 `is_readonly`（Secondary → true，Primary → false）。
    /// 如需启用持久化，链式调用 [`SharedStore::with_persistence`]。
    pub fn new(
        node_id: impl Into<String>,
        role: NodeRole,
        conflict_resolution: ConflictResolution,
        quota: StorageQuota,
    ) -> Self {
        // v0.26.0 — Task 3：备节点默认只读
        let is_readonly = matches!(role, NodeRole::Secondary);
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            node_id: node_id.into(),
            role: Arc::new(RwLock::new(role)),
            conflict_resolution,
            quota,
            total_bytes: AtomicUsize::new(0),
            replicate_callback: Arc::new(RwLock::new(None)),
            is_readonly: Arc::new(AtomicBool::new(is_readonly)),
            snapshot_path: None,
            wal_path: None,
            wal_count: AtomicU64::new(0),
        }
    }

    /// 配置持久化路径（v0.26.0 — Task 8 builder 方法）
    ///
    /// 启用后，`put`/`delete`/`replicate` 操作会自动追加 WAL，
    /// WAL 记录数达到 `WAL_SNAPSHOT_THRESHOLD` 时自动触发快照。
    pub fn with_persistence(
        mut self,
        snapshot_path: impl AsRef<Path>,
        wal_path: impl AsRef<Path>,
    ) -> Self {
        self.snapshot_path = Some(snapshot_path.as_ref().to_path_buf());
        self.wal_path = Some(wal_path.as_ref().to_path_buf());
        self
    }

    /// 返回本节点 ID
    pub fn local_node_id(&self) -> &str {
        &self.node_id
    }

    /// 返回本节点角色
    pub fn local_role(&self) -> NodeRole {
        *self.role.read().unwrap_or_else(|e| e.into_inner())
    }

    /// 更新本节点角色（用于主备切换后更新冲突解决依据）
    pub fn update_role(&self, new_role: NodeRole) {
        *self.role.write().unwrap_or_else(|e| e.into_inner()) = new_role;
    }

    /// 返回冲突解决策略
    pub fn conflict_resolution(&self) -> ConflictResolution {
        self.conflict_resolution
    }

    /// 返回存储配额
    pub fn quota(&self) -> &StorageQuota {
        &self.quota
    }

    /// 是否处于只读模式（v0.26.0 — Task 3）
    ///
    /// 备节点默认只读（`is_readonly = true`），FailoverEngine 接管后置为 `false`。
    pub fn is_readonly(&self) -> bool {
        self.is_readonly.load(Ordering::SeqCst)
    }

    /// 设置只读模式（v0.26.0 — Task 3）
    ///
    /// FailoverEngine 切换成功后调用 `set_readonly(false)`，
    /// 回切为 Secondary 时调用 `set_readonly(true)`。
    pub fn set_readonly(&self, readonly: bool) {
        self.is_readonly.store(readonly, Ordering::SeqCst);
    }

    /// 写入数据（主节点写入，触发复制回调）
    ///
    /// 版本号自动递增（新键从 1 开始，已有键在原版本上加 1）。
    /// 写入前检查配额，超过配额返回 [`StorageError::QuotaExceeded`]。
    /// 写入成功后触发复制回调（如已设置），将条目复制到备节点。
    /// v0.26.0：写入成功后追加 WAL（如已配置持久化）。
    pub fn put(
        &self,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Result<(), StorageError> {
        let key = key.into();
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());

        let is_new = !entries.contains_key(&key);

        // 检查条目数配额（仅对新键）
        if is_new && entries.len() >= self.quota.max_entries {
            return Err(StorageError::QuotaExceeded);
        }

        // 构建新条目：版本号递增
        let version = entries.get(&key).map(|e| e.version + 1).unwrap_or(1);
        let entry = StorageEntry {
            key: key.clone(),
            value,
            timestamp: current_timestamp_millis(),
            node_id: self.node_id.clone(),
            version,
        };

        // 检查字节数配额（O(1)：基于 total_bytes 计数器）
        let new_bytes = serde_json::to_vec(&entry)?.len();
        let old_bytes = entries
            .get(&key)
            .and_then(|e| serde_json::to_vec(e).ok())
            .map(|v| v.len())
            .unwrap_or(0);
        let new_total = self.total_bytes.load(Ordering::SeqCst) - old_bytes + new_bytes;
        if new_total > self.quota.max_bytes {
            return Err(StorageError::QuotaExceeded);
        }

        entries.insert(key, entry.clone());
        // 增量更新 total_bytes（单次 store 避免中间不一致状态）
        self.total_bytes.store(new_total, Ordering::SeqCst);
        drop(entries);

        // 追加 WAL（v0.26.0 — Task 8）。失败仅记录日志，不影响 put 语义
        let wal_op = WalOperation::Put {
            key: entry.key.clone(),
            value: entry.value.clone(),
            timestamp: entry.timestamp,
            version: entry.version,
            node_id: entry.node_id.clone(),
        };
        if let Err(e) = self.append_wal(&wal_op) {
            tracing::warn!(error = %e, "SharedStore: append_wal Put failed");
        }

        // 触发复制回调
        let cb = self
            .replicate_callback
            .read()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ref callback) = *cb {
            callback(entry);
        }

        Ok(())
    }

    /// 读取数据
    ///
    /// 返回键对应的存储条目副本；键不存在时返回 `None`。
    pub fn get(&self, key: &str) -> Option<StorageEntry> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    /// 删除数据
    ///
    /// 返回 `Ok(true)` 表示键存在并已删除，`Ok(false)` 表示键不存在。
    /// 删除成功后触发复制回调（如已设置），构造 tombstone（version=0、value=Null）复制到备节点。
    /// v0.26.0：删除成功后追加 WAL（如已配置持久化）。
    pub fn delete(&self, key: &str) -> Result<bool, StorageError> {
        // 先在写锁内完成删除，释放锁后再触发回调，避免回调中再次访问存储导致死锁
        let old_entry = {
            let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
            entries.remove(key)
        };
        if let Some(old_entry) = old_entry {
            // 更新 total_bytes
            let old_bytes = serde_json::to_vec(&old_entry)
                .map(|v| v.len())
                .unwrap_or(0);
            self.total_bytes.fetch_sub(old_bytes, Ordering::SeqCst);

            // 追加 WAL（v0.26.0 — Task 8）
            let wal_op = WalOperation::Delete {
                key: key.to_string(),
                timestamp: current_timestamp_millis(),
            };
            if let Err(e) = self.append_wal(&wal_op) {
                tracing::warn!(error = %e, "SharedStore: append_wal Delete failed");
            }

            // 触发复制回调（tombstone）
            if let Some(cb) = self
                .replicate_callback
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                let tombstone = StorageEntry {
                    key: key.to_string(),
                    value: serde_json::Value::Null,
                    timestamp: current_timestamp_millis(),
                    node_id: self.node_id.clone(),
                    version: 0,
                };
                cb(tombstone);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 接收来自主节点的复制数据
    ///
    /// 冲突检测逻辑：
    /// 1. 如果 key 不存在 → 直接写入
    /// 2. 如果 key 存在且 version 更高 → 更新
    /// 3. 如果 key 存在且 version 相同但内容不同 → 冲突，按策略解决
    /// 4. 如果 key 存在且 version 相同且内容相同 → 无操作
    /// 5. 如果 key 存在且 version 更低 → 忽略（本地版本更新）
    ///
    /// v0.26.0：写入成功后追加 WAL（如已配置持久化）。
    pub fn replicate(&self, entry: StorageEntry) -> Result<(), StorageError> {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());

        // 先确定要插入的条目（避免与 entries 写锁的借用冲突）
        let to_insert: Option<StorageEntry> = match entries.get(&entry.key) {
            None => Some(entry),
            Some(local) => {
                if entry.version > local.version {
                    // version 更高 → 更新
                    Some(entry)
                } else if entry.version == local.version && local.value != entry.value {
                    // version 相同但内容不同 → 冲突，按策略解决
                    Some(Self::resolve_conflict_inner(
                        self.conflict_resolution,
                        *self.role.read().unwrap_or_else(|e| e.into_inner()),
                        local,
                        &entry,
                    ))
                } else {
                    // version 相同且内容相同，或 version 更低 → 无操作
                    None
                }
            }
        };

        let inserted: Option<StorageEntry> = if let Some(resolved) = to_insert {
            // 配额检查（与 put 一致）
            let is_new = !entries.contains_key(&resolved.key);
            if is_new && entries.len() >= self.quota.max_entries {
                return Err(StorageError::QuotaExceeded);
            }
            let new_bytes = serde_json::to_vec(&resolved)?.len();
            let old_bytes = entries
                .get(&resolved.key)
                .and_then(|e| serde_json::to_vec(e).ok())
                .map(|v| v.len())
                .unwrap_or(0);
            let new_total = self.total_bytes.load(Ordering::SeqCst) - old_bytes + new_bytes;
            if new_total > self.quota.max_bytes {
                return Err(StorageError::QuotaExceeded);
            }
            // 更新 total_bytes
            self.total_bytes.store(new_total, Ordering::SeqCst);
            entries.insert(resolved.key.clone(), resolved.clone());
            Some(resolved)
        } else {
            None
        };
        drop(entries);

        // 追加 WAL（v0.26.0 — Task 8）
        if let Some(resolved) = inserted {
            let wal_op = WalOperation::Replicate {
                key: resolved.key.clone(),
                value: resolved.value.clone(),
                timestamp: resolved.timestamp,
                version: resolved.version,
                node_id: resolved.node_id.clone(),
            };
            if let Err(e) = self.append_wal(&wal_op) {
                tracing::warn!(error = %e, "SharedStore: append_wal Replicate failed");
            }
        }

        Ok(())
    }

    /// 检测冲突
    ///
    /// 当本地与远程条目版本号相同但值不同时，判定为冲突。
    pub fn detect_conflict(&self, local: &StorageEntry, remote: &StorageEntry) -> bool {
        local.version == remote.version && local.value != remote.value
    }

    /// 按策略解决冲突
    ///
    /// - `PrimaryWins`: 主节点数据获胜（备节点上 remote 获胜，主节点上 local 获胜）
    /// - `TimestampWins`: 时间戳最新的获胜（平局用 node_id 字典序 tiebreaker）
    /// - `VersionWins`: 版本号最高的获胜（版本相等回退到 TimestampWins + node_id tiebreaker）
    pub fn resolve_conflict(&self, local: &StorageEntry, remote: &StorageEntry) -> StorageEntry {
        Self::resolve_conflict_inner(
            self.conflict_resolution,
            *self.role.read().unwrap_or_else(|e| e.into_inner()),
            local,
            remote,
        )
    }

    /// 冲突解决内部实现（独立函数，避免与 entries 写锁的借用冲突）
    fn resolve_conflict_inner(
        conflict_resolution: ConflictResolution,
        role: NodeRole,
        local: &StorageEntry,
        remote: &StorageEntry,
    ) -> StorageEntry {
        match conflict_resolution {
            ConflictResolution::PrimaryWins => {
                // 备节点上 remote（来自主节点）获胜；主节点上 local 获胜
                if role == NodeRole::Secondary {
                    remote.clone()
                } else {
                    local.clone()
                }
            }
            ConflictResolution::TimestampWins => {
                if remote.timestamp > local.timestamp {
                    remote.clone()
                } else if remote.timestamp < local.timestamp {
                    local.clone()
                } else {
                    // 时间戳相等，用 node_id 字典序 tiebreaker
                    if remote.node_id >= local.node_id {
                        remote.clone()
                    } else {
                        local.clone()
                    }
                }
            }
            ConflictResolution::VersionWins => {
                if remote.version > local.version {
                    remote.clone()
                } else if remote.version < local.version {
                    local.clone()
                } else {
                    // 版本相等，回退到 TimestampWins
                    if remote.timestamp > local.timestamp {
                        remote.clone()
                    } else if remote.timestamp < local.timestamp {
                        local.clone()
                    } else {
                        // 时间戳也相等，用 node_id 字典序 tiebreaker
                        if remote.node_id >= local.node_id {
                            remote.clone()
                        } else {
                            local.clone()
                        }
                    }
                }
            }
        }
    }

    /// 检查配额
    ///
    /// 返回 `Ok(())` 表示当前存储在配额范围内，
    /// `Err(StorageError::QuotaExceeded)` 表示已超限。
    pub fn check_quota(&self) -> Result<(), StorageError> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        if entries.len() > self.quota.max_entries {
            return Err(StorageError::QuotaExceeded);
        }
        let total: usize = entries
            .values()
            .map(|e| serde_json::to_vec(e).map(|v| v.len()).unwrap_or(0))
            .sum();
        if total > self.quota.max_bytes {
            return Err(StorageError::QuotaExceeded);
        }
        Ok(())
    }

    /// 当前条目数
    pub fn entry_count(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 当前总字节数（基于 total_bytes 计数器，O(1)）
    pub fn total_bytes(&self) -> usize {
        self.total_bytes.load(Ordering::SeqCst)
    }

    /// 设置复制回调
    ///
    /// 当 [`SharedStore::put`] 写入数据后，回调被调用以将条目复制到备节点。
    pub fn set_replicate_callback(&self, callback: Box<dyn Fn(StorageEntry) + Send + Sync>) {
        let mut cb = self
            .replicate_callback
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *cb = Some(callback);
    }

    /// 列出所有键
    pub fn list_keys(&self) -> Vec<String> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        entries.keys().cloned().collect()
    }

    /// 返回所有条目（v0.26.0 — Task 8，用于 checksum 计算和快照）
    pub fn entries(&self) -> Vec<StorageEntry> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    // ========================================================================
    // v0.26.0 — Task 8 持久化（snapshot + WAL）
    // ========================================================================

    /// 创建快照：序列化所有 entries 到 snapshot_path（JSON），成功后清空 WAL 文件
    ///
    /// - 未配置 `snapshot_path` 时直接返回 `Ok(())`
    /// - 原子写入：先写 `.tmp` 临时文件再 rename，避免崩溃导致快照损坏
    /// - 快照成功后清空 WAL 并重置 `wal_count`
    pub fn snapshot(&self) -> Result<(), StorageError> {
        let snapshot_path = match &self.snapshot_path {
            Some(p) => p.clone(),
            None => return Ok(()),
        };

        // 序列化所有条目
        let entries_vec: Vec<StorageEntry> = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        let json = serde_json::to_string(&entries_vec)?;

        // 原子写入：先写 .tmp 再 rename
        let tmp_path = snapshot_path.with_extension("tmp");
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &snapshot_path)?;

        // 清空 WAL 文件并重置计数器
        if let Some(wal_path) = &self.wal_path {
            std::fs::write(wal_path, "")?;
        }
        self.wal_count.store(0, Ordering::SeqCst);

        tracing::info!(
            entries = entries_vec.len(),
            "SharedStore: snapshot created at {:?}",
            snapshot_path
        );
        Ok(())
    }

    /// 追加 WAL 操作记录（JSON Lines）到 wal_path
    ///
    /// - 未配置 `wal_path` 时直接返回 `Ok(())`（跳过持久化）
    /// - 追加成功后递增 `wal_count`，达到 `WAL_SNAPSHOT_THRESHOLD` 时自动触发快照
    pub fn append_wal(&self, operation: &WalOperation) -> Result<(), StorageError> {
        let wal_path = match &self.wal_path {
            Some(p) => p.clone(),
            None => return Ok(()), // 未配置持久化，跳过
        };

        let json = serde_json::to_string(operation)?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)?;
        writeln!(file, "{}", json)?;

        // 递增计数并检查阈值
        let count = self.wal_count.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= WAL_SNAPSHOT_THRESHOLD {
            tracing::info!(
                count,
                "SharedStore: WAL count reached threshold, triggering snapshot"
            );
            self.snapshot()?;
        }
        Ok(())
    }

    /// 从磁盘加载：先加载 snapshot_path，再重放 wal_path
    ///
    /// - 快照文件不存在时跳过快照加载
    /// - WAL 文件不存在时跳过 WAL 重放
    /// - WAL 重放：按顺序应用 Put/Delete/Replicate，跳过已应用的（version <= 当前 version）
    /// - 重放不触发复制回调，也不追加 WAL（避免循环）
    pub fn load_from_disk(&self) -> Result<(), StorageError> {
        // 1. 加载快照
        if let Some(snapshot_path) = &self.snapshot_path {
            if snapshot_path.exists() {
                let content = std::fs::read_to_string(snapshot_path)?;
                if !content.is_empty() {
                    let snapshot_entries: Vec<StorageEntry> = serde_json::from_str(&content)?;
                    let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
                    let mut total: usize = 0;
                    for entry in snapshot_entries {
                        let bytes = serde_json::to_vec(&entry).map(|v| v.len()).unwrap_or(0);
                        total = total.saturating_add(bytes);
                        entries.insert(entry.key.clone(), entry);
                    }
                    self.total_bytes.store(total, Ordering::SeqCst);
                }
            }
        }

        // 2. 重放 WAL
        if let Some(wal_path) = &self.wal_path {
            if wal_path.exists() {
                let content = std::fs::read_to_string(wal_path)?;
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let op: WalOperation = serde_json::from_str(line)?;
                    self.replay_wal_op(&op)?;
                }
            }
        }

        Ok(())
    }

    /// 重放单条 WAL 操作（内部方法，不触发回调、不追加 WAL）
    ///
    /// 跳过已应用的（version <= 当前 version），仅当 version > 本地 version 时应用。
    fn replay_wal_op(&self, op: &WalOperation) -> Result<(), StorageError> {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        match op {
            WalOperation::Put {
                key,
                value,
                timestamp,
                version,
                node_id,
            } => {
                let should_apply = match entries.get(key) {
                    None => true,
                    Some(local) => *version > local.version,
                };
                if should_apply {
                    let entry = StorageEntry {
                        key: key.clone(),
                        value: value.clone(),
                        timestamp: *timestamp,
                        node_id: node_id.clone(),
                        version: *version,
                    };
                    let new_bytes = serde_json::to_vec(&entry)?.len();
                    let old_bytes = entries
                        .get(key)
                        .and_then(|e| serde_json::to_vec(e).ok())
                        .map(|v| v.len())
                        .unwrap_or(0);
                    let new_total =
                        self.total_bytes.load(Ordering::SeqCst) - old_bytes + new_bytes;
                    self.total_bytes.store(new_total, Ordering::SeqCst);
                    entries.insert(key.clone(), entry);
                }
            }
            WalOperation::Delete { key, .. } => {
                if let Some(old) = entries.remove(key) {
                    let old_bytes = serde_json::to_vec(&old).map(|v| v.len()).unwrap_or(0);
                    self.total_bytes.fetch_sub(old_bytes, Ordering::SeqCst);
                }
            }
            WalOperation::Replicate {
                key,
                value,
                timestamp,
                version,
                node_id,
            } => {
                // 重放 Replicate：按 version 判断是否应用（不触发冲突解决，保证幂等）
                let should_apply = match entries.get(key) {
                    None => true,
                    Some(local) => *version > local.version,
                };
                if should_apply {
                    let entry = StorageEntry {
                        key: key.clone(),
                        value: value.clone(),
                        timestamp: *timestamp,
                        node_id: node_id.clone(),
                        version: *version,
                    };
                    let new_bytes = serde_json::to_vec(&entry)?.len();
                    let old_bytes = entries
                        .get(key)
                        .and_then(|e| serde_json::to_vec(e).ok())
                        .map(|v| v.len())
                        .unwrap_or(0);
                    let new_total =
                        self.total_bytes.load(Ordering::SeqCst) - old_bytes + new_bytes;
                    self.total_bytes.store(new_total, Ordering::SeqCst);
                    entries.insert(key.clone(), entry);
                }
            }
        }
        Ok(())
    }

    /// 返回当前 WAL 记录数（主要用于测试）
    pub fn wal_count(&self) -> u64 {
        self.wal_count.load(Ordering::SeqCst)
    }
}

/// 获取当前 Unix 时间戳（毫秒）
fn current_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 构造测试用 Primary 共享存储（默认配额、PrimaryWins 策略）
    fn make_primary_store() -> SharedStore {
        SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
    }

    /// 构造测试用 Secondary 共享存储（默认配额、PrimaryWins 策略）
    fn make_secondary_store() -> SharedStore {
        SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
    }

    #[test]
    fn test_put_and_get() {
        let store = make_primary_store();

        // 写入新键
        store
            .put("key1", serde_json::json!({"value": 42}))
            .expect("put should succeed");

        // 读取验证
        let entry = store.get("key1").expect("entry should exist");
        assert_eq!(entry.key, "key1");
        assert_eq!(entry.value, serde_json::json!({"value": 42}));
        assert_eq!(entry.version, 1, "first write → version 1");
        assert_eq!(entry.node_id, "node-1");

        // 更新已有键 → 版本递增
        store
            .put("key1", serde_json::json!({"value": 43}))
            .expect("put update should succeed");
        let entry = store.get("key1").expect("entry should exist");
        assert_eq!(entry.value, serde_json::json!({"value": 43}));
        assert_eq!(entry.version, 2, "second write → version 2");

        // 不存在的键返回 None
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_delete() {
        let store = make_primary_store();
        store.put("key1", serde_json::json!(1)).unwrap();
        assert!(store.get("key1").is_some());

        // 删除存在的键
        assert!(store.delete("key1").unwrap(), "delete existing key → true");
        assert!(store.get("key1").is_none(), "deleted key → None");

        // 删除不存在的键
        assert!(
            !store.delete("key1").unwrap(),
            "delete non-existent key → false"
        );
    }

    #[test]
    fn test_replicate_new_key() {
        let store = make_secondary_store();
        let entry = StorageEntry {
            key: "replicated".to_string(),
            value: serde_json::json!("hello"),
            timestamp: 1000,
            node_id: "node-1".to_string(),
            version: 1,
        };

        store.replicate(entry).expect("replicate should succeed");

        let got = store.get("replicated").expect("entry should exist");
        assert_eq!(got.value, serde_json::json!("hello"));
        assert_eq!(got.version, 1);
        assert_eq!(got.node_id, "node-1");
    }

    #[test]
    fn test_replicate_higher_version() {
        let store = make_secondary_store();

        // 初始版本 1
        let entry_v1 = StorageEntry {
            key: "key1".to_string(),
            value: serde_json::json!("v1"),
            timestamp: 1000,
            node_id: "node-1".to_string(),
            version: 1,
        };
        store.replicate(entry_v1).unwrap();

        // 更高版本 → 更新
        let entry_v2 = StorageEntry {
            key: "key1".to_string(),
            value: serde_json::json!("v2"),
            timestamp: 2000,
            node_id: "node-1".to_string(),
            version: 2,
        };
        store.replicate(entry_v2).unwrap();

        let got = store.get("key1").expect("entry should exist");
        assert_eq!(got.value, serde_json::json!("v2"));
        assert_eq!(got.version, 2);

        // 更低版本 → 忽略（本地版本更新）
        let entry_v1_again = StorageEntry {
            key: "key1".to_string(),
            value: serde_json::json!("old"),
            timestamp: 500,
            node_id: "node-1".to_string(),
            version: 1,
        };
        store.replicate(entry_v1_again).unwrap();

        let got = store.get("key1").unwrap();
        assert_eq!(got.value, serde_json::json!("v2"), "lower version ignored");
        assert_eq!(got.version, 2);
    }

    #[test]
    fn test_conflict_detection() {
        let store = make_secondary_store();

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 1000,
            node_id: "node-2".to_string(),
            version: 1,
        };
        let remote_same_value = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"), // 相同值
            timestamp: 2000,
            node_id: "node-1".to_string(),
            version: 1,
        };
        let remote_diff_value = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"), // 不同值
            timestamp: 2000,
            node_id: "node-1".to_string(),
            version: 1,
        };
        let remote_higher_version = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 2000,
            node_id: "node-1".to_string(),
            version: 2,
        };

        // 相同版本 + 相同值 → 无冲突
        assert!(
            !store.detect_conflict(&local, &remote_same_value),
            "same version + same value → no conflict"
        );
        // 相同版本 + 不同值 → 冲突
        assert!(
            store.detect_conflict(&local, &remote_diff_value),
            "same version + different value → conflict"
        );
        // 不同版本 → 无冲突
        assert!(
            !store.detect_conflict(&local, &remote_higher_version),
            "different version → no conflict"
        );
    }

    #[test]
    fn test_conflict_resolution_primary_wins() {
        // 本节点为备节点，remote 来自主节点
        let store = make_secondary_store();

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 2000, // 更新
            node_id: "node-2".to_string(),
            version: 1,
        };
        let remote = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 1000, // 更旧
            node_id: "node-1".to_string(),
            version: 1,
        };

        // PrimaryWins: 备节点上 remote（来自主节点）获胜，即使时间戳更旧
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "PrimaryWins → remote (primary) wins on secondary"
        );
    }

    #[test]
    fn test_conflict_resolution_timestamp_wins() {
        let store = SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::TimestampWins,
            StorageQuota::default(),
        );

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 2000, // 更新
            node_id: "node-2".to_string(),
            version: 1,
        };
        let remote = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 1000, // 更旧
            node_id: "node-1".to_string(),
            version: 1,
        };

        // TimestampWins: local 时间戳更新 → local 获胜
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("local"),
            "TimestampWins → newer timestamp wins"
        );

        // 反过来：remote 时间戳更新 → remote 获胜
        let resolved = store.resolve_conflict(&remote, &local);
        assert_eq!(
            resolved.value,
            serde_json::json!("local"),
            "TimestampWins → newer timestamp wins (reversed)"
        );
    }

    #[test]
    fn test_quota_exceeded() {
        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota {
                max_entries: 2,
                max_bytes: 1024,
            },
        );

        // 前两个条目 OK
        store.put("k1", serde_json::json!(1)).unwrap();
        store.put("k2", serde_json::json!(2)).unwrap();
        assert_eq!(store.entry_count(), 2);

        // 第三个条目超过条目数配额
        let result = store.put("k3", serde_json::json!(3));
        assert!(
            matches!(result, Err(StorageError::QuotaExceeded)),
            "third entry should exceed quota"
        );
        assert_eq!(store.entry_count(), 2, "failed put should not add entry");

        // 更新已有键应仍然成功（不增加条目数）
        store
            .put("k1", serde_json::json!(10))
            .expect("update existing key should work");
        let entry = store.get("k1").unwrap();
        assert_eq!(entry.value, serde_json::json!(10));
        assert_eq!(entry.version, 2);
    }

    #[test]
    fn test_replicate_callback() {
        let store = make_primary_store();

        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        store.set_replicate_callback(Box::new(move |entry| {
            captured_clone.lock().unwrap().push(entry);
        }));

        store.put("key1", serde_json::json!(42)).unwrap();
        store.put("key2", serde_json::json!("hello")).unwrap();

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2, "callback should be called twice");
        assert_eq!(captured[0].key, "key1");
        assert_eq!(captured[0].value, serde_json::json!(42));
        assert_eq!(captured[0].version, 1);
        assert_eq!(captured[1].key, "key2");
        assert_eq!(captured[1].value, serde_json::json!("hello"));
    }

    #[test]
    fn test_list_keys() {
        let store = make_primary_store();
        assert!(store.list_keys().is_empty(), "no keys initially");

        store.put("a", serde_json::json!(1)).unwrap();
        store.put("b", serde_json::json!(2)).unwrap();
        store.put("c", serde_json::json!(3)).unwrap();

        let mut keys = store.list_keys();
        keys.sort();
        assert_eq!(keys, vec!["a", "b", "c"]);

        // 删除后列表更新
        store.delete("b").unwrap();
        let mut keys = store.list_keys();
        keys.sort();
        assert_eq!(keys, vec!["a", "c"]);
    }

    // ------------------------------------------------------------------------
    // v0.25.1 Task 4 新增测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_update_role_changes_conflict_resolution() {
        // 初始 role=Secondary，PrimaryWins 策略下 remote（来自主节点）获胜
        let store = SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::PrimaryWins,
            StorageQuota::default(),
        );

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 1000,
            node_id: "node-2".to_string(),
            version: 1,
        };
        let remote = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 1000,
            node_id: "node-1".to_string(),
            version: 1,
        };

        // Secondary → remote（主节点）获胜
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "Secondary → remote wins"
        );

        // 切换为 Primary → local 获胜
        store.update_role(NodeRole::Primary);
        assert_eq!(store.local_role(), NodeRole::Primary);
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("local"),
            "Primary → local wins after update_role"
        );
    }

    #[test]
    fn test_replicate_respects_quota() {
        let store = SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::default(),
            StorageQuota {
                max_entries: 2,
                max_bytes: 1024,
            },
        );

        // put 2 个条目（达到 max_entries 上限）
        store.put("k1", serde_json::json!(1)).unwrap();
        store.put("k2", serde_json::json!(2)).unwrap();
        assert_eq!(store.entry_count(), 2);

        // replicate 第 3 个新键 → 应返回 QuotaExceeded
        let e3 = StorageEntry {
            key: "k3".to_string(),
            value: serde_json::json!(3),
            timestamp: 1000,
            node_id: "node-1".to_string(),
            version: 1,
        };
        let result = store.replicate(e3);
        assert!(
            matches!(result, Err(StorageError::QuotaExceeded)),
            "replicate beyond max_entries should fail"
        );
        assert_eq!(
            store.entry_count(),
            2,
            "failed replicate should not add entry"
        );
    }

    #[test]
    fn test_delete_triggers_replicate_callback() {
        let store = make_primary_store();
        store.put("key1", serde_json::json!(42)).unwrap();

        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        store.set_replicate_callback(Box::new(move |entry| {
            captured_clone.lock().unwrap().push(entry);
        }));

        let deleted = store.delete("key1").unwrap();
        assert!(deleted, "delete should return true for existing key");

        let captured = captured.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "callback should be called once on delete"
        );
        let tombstone = &captured[0];
        assert_eq!(tombstone.key, "key1");
        assert_eq!(
            tombstone.value,
            serde_json::Value::Null,
            "tombstone value should be Null"
        );
        assert_eq!(tombstone.version, 0, "tombstone version should be 0");
    }

    #[test]
    fn test_put_o1_quota_check() {
        let store = make_primary_store();
        let mut expected_bytes: usize = 0;

        // 写入大量条目，验证 total_bytes 计数器与逐条目序列化字节数之和一致
        for i in 0..100u32 {
            let key = format!("key{}", i);
            store.put(key.clone(), serde_json::json!(i)).unwrap();
            let entry = store.get(&key).expect("entry should exist");
            let bytes = serde_json::to_vec(&entry).map(|v| v.len()).unwrap_or(0);
            expected_bytes += bytes;
        }

        assert_eq!(
            store.total_bytes(),
            expected_bytes,
            "total_bytes counter should match sum of entry bytes"
        );

        // 更新已有键后计数器仍应正确
        let old_entry = store.get("key0").unwrap();
        let old_bytes = serde_json::to_vec(&old_entry).map(|v| v.len()).unwrap_or(0);
        store.put("key0", serde_json::json!("updated")).unwrap();
        let new_entry = store.get("key0").unwrap();
        let new_bytes = serde_json::to_vec(&new_entry).map(|v| v.len()).unwrap_or(0);
        expected_bytes = expected_bytes - old_bytes + new_bytes;
        assert_eq!(
            store.total_bytes(),
            expected_bytes,
            "total_bytes should update correctly after key update"
        );

        // 删除后计数器应减少
        let removed_entry = store.get("key1").unwrap();
        let removed_bytes = serde_json::to_vec(&removed_entry)
            .map(|v| v.len())
            .unwrap_or(0);
        store.delete("key1").unwrap();
        expected_bytes -= removed_bytes;
        assert_eq!(
            store.total_bytes(),
            expected_bytes,
            "total_bytes should decrease after delete"
        );
    }

    #[test]
    fn test_version_wins_tiebreaker_by_node_id() {
        let store = SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::VersionWins,
            StorageQuota::default(),
        );

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 1000,
            node_id: "aaa".to_string(),
            version: 5,
        };
        let remote = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 1000,            // 与 local 相等
            node_id: "zzz".to_string(), // 字典序大于 local
            version: 5,                 // 与 local 相等
        };

        // 版本相等、时间戳相等 → node_id 字典序大的获胜（remote "zzz" > local "aaa"）
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "VersionWins tie → larger node_id wins"
        );

        // 交换参数：node_id "zzz" 仍在 remote 位置时应获胜，结果不变
        let resolved = store.resolve_conflict(&remote, &local);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "VersionWins tie → larger node_id wins (reversed)"
        );
    }

    #[test]
    fn test_timestamp_wins_tiebreaker_by_node_id() {
        let store = SharedStore::new(
            "node-2",
            NodeRole::Secondary,
            ConflictResolution::TimestampWins,
            StorageQuota::default(),
        );

        let local = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("local"),
            timestamp: 1000,
            node_id: "aaa".to_string(),
            version: 1,
        };
        let remote = StorageEntry {
            key: "k".to_string(),
            value: serde_json::json!("remote"),
            timestamp: 1000,            // 与 local 相等
            node_id: "zzz".to_string(), // 字典序大于 local
            version: 1,
        };

        // 时间戳相等 → node_id 字典序大的获胜（remote "zzz" > local "aaa"）
        let resolved = store.resolve_conflict(&local, &remote);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "TimestampWins tie → larger node_id wins"
        );

        // 交换参数：node_id "zzz" 仍在 remote 位置时应获胜，结果不变
        let resolved = store.resolve_conflict(&remote, &local);
        assert_eq!(
            resolved.value,
            serde_json::json!("remote"),
            "TimestampWins tie → larger node_id wins (reversed)"
        );
    }

    // ------------------------------------------------------------------------
    // v0.26.0 — Task 3 服务降级模式测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_is_readonly_default_by_role() {
        // Primary 默认可写
        let primary_store = make_primary_store();
        assert!(
            !primary_store.is_readonly(),
            "Primary should be writable by default"
        );

        // Secondary 默认只读
        let secondary_store = make_secondary_store();
        assert!(
            secondary_store.is_readonly(),
            "Secondary should be readonly by default"
        );
    }

    #[test]
    fn test_set_readonly_toggle() {
        let store = make_secondary_store();
        assert!(store.is_readonly(), "Secondary initially readonly");

        // FailoverEngine 接管后置为可写
        store.set_readonly(false);
        assert!(!store.is_readonly(), "after set_readonly(false)");

        // 回切为 Secondary 时置为只读
        store.set_readonly(true);
        assert!(store.is_readonly(), "after set_readonly(true)");
    }

    #[test]
    fn test_readonly_independent_of_role() {
        // is_readonly 与 role 独立：update_role 不改变 is_readonly
        let store = make_secondary_store();
        assert!(store.is_readonly());
        store.update_role(NodeRole::Primary);
        // update_role 不改变 is_readonly（需显式调用 set_readonly）
        assert!(
            store.is_readonly(),
            "update_role should not change is_readonly"
        );
        store.set_readonly(false);
        assert!(!store.is_readonly());
    }

    // ------------------------------------------------------------------------
    // v0.26.0 — Task 8 持久化测试
    // ------------------------------------------------------------------------

    /// 唯一临时目录包装，确保 drop 时清理
    fn temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("eneros-ha-test-{}-{}", pid, nanos));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_snapshot_create() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
        .with_persistence(&snapshot_path, &wal_path);

        store.put("k1", serde_json::json!(1)).unwrap();
        store.put("k2", serde_json::json!("hello")).unwrap();

        // 创建快照
        store.snapshot().expect("snapshot should succeed");

        // 快照文件应存在且非空
        assert!(snapshot_path.exists(), "snapshot file should exist");
        let content = std::fs::read_to_string(&snapshot_path).unwrap();
        assert!(!content.is_empty(), "snapshot file should not be empty");

        // 快照内容应能反序列化为 Vec<StorageEntry>
        let entries: Vec<StorageEntry> = serde_json::from_str(&content).unwrap();
        assert_eq!(entries.len(), 2, "snapshot should contain 2 entries");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_append() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
        .with_persistence(&snapshot_path, &wal_path);

        // put 操作应自动追加 WAL
        store.put("k1", serde_json::json!(42)).unwrap();
        store.put("k2", serde_json::json!("hi")).unwrap();

        // WAL 文件应存在且包含 2 行
        assert!(wal_path.exists(), "WAL file should exist");
        let content = std::fs::read_to_string(&wal_path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2, "WAL should have 2 records");

        // 每行应能反序列化为 WalOperation::Put
        for line in &lines {
            let op: WalOperation = serde_json::from_str(line).expect("WAL line should deserialize");
            assert!(matches!(op, WalOperation::Put { .. }));
        }

        // wal_count 应为 2
        assert_eq!(store.wal_count(), 2, "wal_count should be 2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_from_disk_recovery() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        // 第一个 store：写入数据并创建快照
        {
            let store = SharedStore::new(
                "node-1",
                NodeRole::Primary,
                ConflictResolution::default(),
                StorageQuota::default(),
            )
            .with_persistence(&snapshot_path, &wal_path);

            store.put("k1", serde_json::json!(1)).unwrap();
            store.put("k2", serde_json::json!("hello")).unwrap();
            store.snapshot().unwrap();
            // 快照后 WAL 被清空
            assert_eq!(store.wal_count(), 0);
        }

        // 第二个 store：从磁盘加载，应恢复数据
        {
            let store = SharedStore::new(
                "node-1",
                NodeRole::Primary,
                ConflictResolution::default(),
                StorageQuota::default(),
            )
            .with_persistence(&snapshot_path, &wal_path);

            store.load_from_disk().expect("load_from_disk should succeed");

            assert_eq!(store.entry_count(), 2, "should recover 2 entries");
            let k1 = store.get("k1").expect("k1 should exist");
            assert_eq!(k1.value, serde_json::json!(1));
            assert_eq!(k1.version, 1);
            let k2 = store.get("k2").expect("k2 should exist");
            assert_eq!(k2.value, serde_json::json!("hello"));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wal_replay_after_snapshot() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        // 第一个 store：写入数据 → 快照 → 再写入（产生 WAL）
        {
            let store = SharedStore::new(
                "node-1",
                NodeRole::Primary,
                ConflictResolution::default(),
                StorageQuota::default(),
            )
            .with_persistence(&snapshot_path, &wal_path);

            store.put("k1", serde_json::json!(1)).unwrap();
            store.snapshot().unwrap(); // 快照包含 k1，WAL 清空
            store.put("k2", serde_json::json!(2)).unwrap(); // WAL 包含 k2
            store.put("k1", serde_json::json!(10)).unwrap(); // WAL 包含 k1 更新 (version 2)
        }

        // 第二个 store：加载快照（k1=1）+ 重放 WAL（k2=2, k1=10）
        {
            let store = SharedStore::new(
                "node-1",
                NodeRole::Primary,
                ConflictResolution::default(),
                StorageQuota::default(),
            )
            .with_persistence(&snapshot_path, &wal_path);

            store.load_from_disk().unwrap();

            assert_eq!(store.entry_count(), 2);
            let k1 = store.get("k1").unwrap();
            assert_eq!(k1.value, serde_json::json!(10), "k1 should be updated by WAL replay");
            assert_eq!(k1.version, 2, "k1 version should be 2");
            let k2 = store.get("k2").unwrap();
            assert_eq!(k2.value, serde_json::json!(2));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_clears_wal() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
        .with_persistence(&snapshot_path, &wal_path);

        store.put("k1", serde_json::json!(1)).unwrap();
        store.put("k2", serde_json::json!(2)).unwrap();
        assert_eq!(store.wal_count(), 2);

        // 快照后 WAL 应被清空
        store.snapshot().unwrap();
        assert_eq!(store.wal_count(), 0, "wal_count should reset to 0");

        let wal_content = std::fs::read_to_string(&wal_path).unwrap();
        assert!(
            wal_content.is_empty(),
            "WAL file should be empty after snapshot"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_persistence_skips_wal() {
        // 未配置持久化路径时，append_wal 应跳过且不报错
        let store = make_primary_store(); // 无 with_persistence

        store.put("k1", serde_json::json!(1)).unwrap();
        store.put("k2", serde_json::json!(2)).unwrap();

        // wal_count 应保持 0（未配置持久化）
        assert_eq!(store.wal_count(), 0, "wal_count should be 0 without persistence");

        // snapshot 也应直接返回 Ok
        store.snapshot().unwrap();
    }

    #[test]
    fn test_wal_threshold_triggers_snapshot() {
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        // 使用极小配额便于测试，但 max_entries 要足够大以容纳 1000 条
        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota {
                max_entries: 10_000,
                max_bytes: 100 * 1024 * 1024,
            },
        )
        .with_persistence(&snapshot_path, &wal_path);

        // 写入 WAL_SNAPSHOT_THRESHOLD 条记录，应自动触发快照
        for i in 0..WAL_SNAPSHOT_THRESHOLD {
            let key = format!("k{}", i);
            store.put(key, serde_json::json!(i)).unwrap();
        }

        // 触发快照后 wal_count 应被重置为 0
        assert_eq!(
            store.wal_count(),
            0,
            "wal_count should reset after auto-snapshot"
        );
        // 快照文件应存在
        assert!(snapshot_path.exists(), "snapshot should be created");
        // WAL 文件应被清空
        let wal_content = std::fs::read_to_string(&wal_path).unwrap();
        assert!(
            wal_content.is_empty(),
            "WAL should be cleared after auto-snapshot"
        );
        // 快照应包含所有条目
        let snap_content = std::fs::read_to_string(&snapshot_path).unwrap();
        let entries: Vec<StorageEntry> = serde_json::from_str(&snap_content).unwrap();
        assert_eq!(
            entries.len(),
            WAL_SNAPSHOT_THRESHOLD as usize,
            "snapshot should contain all entries"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_concurrent_put_safety() {
        // 多线程并发 put，验证不 panic 且 total_bytes 与条目数一致
        let store = Arc::new(make_primary_store());
        let mut handles = Vec::new();
        let n_threads = 8u32;
        let n_per_thread = 50u32;

        for t in 0..n_threads {
            let store_clone = Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                for i in 0..n_per_thread {
                    let key = format!("t{}-k{}", t, i);
                    store_clone.put(key, serde_json::json!(i)).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().expect("thread should not panic");
        }

        // 所有条目都应存在
        let expected = n_threads * n_per_thread;
        assert_eq!(
            store.entry_count(),
            expected as usize,
            "should have {} entries",
            expected
        );

        // total_bytes 应等于所有条目序列化字节数之和
        let entries = store.entries();
        let sum: usize = entries
            .iter()
            .map(|e| serde_json::to_vec(e).map(|v| v.len()).unwrap_or(0))
            .sum();
        assert_eq!(
            store.total_bytes(),
            sum,
            "total_bytes should match sum after concurrent puts"
        );
    }

    #[test]
    fn test_wal_operation_serde() {
        // WalOperation 各变体序列化/反序列化往返
        let put = WalOperation::Put {
            key: "k1".to_string(),
            value: serde_json::json!(42),
            timestamp: 1000,
            version: 1,
            node_id: "node-1".to_string(),
        };
        let json = serde_json::to_string(&put).unwrap();
        let de: WalOperation = serde_json::from_str(&json).unwrap();
        assert!(matches!(de, WalOperation::Put { .. }));

        let delete = WalOperation::Delete {
            key: "k1".to_string(),
            timestamp: 2000,
        };
        let json = serde_json::to_string(&delete).unwrap();
        let de: WalOperation = serde_json::from_str(&json).unwrap();
        assert!(matches!(de, WalOperation::Delete { .. }));

        let replicate = WalOperation::Replicate {
            key: "k1".to_string(),
            value: serde_json::json!("v"),
            timestamp: 3000,
            version: 2,
            node_id: "node-2".to_string(),
        };
        let json = serde_json::to_string(&replicate).unwrap();
        let de: WalOperation = serde_json::from_str(&json).unwrap();
        assert!(matches!(de, WalOperation::Replicate { .. }));
    }

    #[test]
    fn test_load_from_disk_skips_lower_version() {
        // WAL 重放时跳过 version <= 当前 version 的操作
        let dir = temp_dir();
        let snapshot_path = dir.join("snapshot.json");
        let wal_path = dir.join("wal.log");

        // 手动构造快照（k1 version=5）+ WAL（k1 version=3，应被跳过）
        let snapshot_entries = vec![StorageEntry {
            key: "k1".to_string(),
            value: serde_json::json!("snapshot"),
            timestamp: 1000,
            node_id: "node-1".to_string(),
            version: 5,
        }];
        std::fs::write(
            &snapshot_path,
            serde_json::to_string(&snapshot_entries).unwrap(),
        )
        .unwrap();

        let wal_line = serde_json::json!({
            "op": "put",
            "key": "k1",
            "value": "wal",
            "timestamp": 2000,
            "version": 3,
            "node_id": "node-1"
        });
        std::fs::write(&wal_path, format!("{}\n", wal_line)).unwrap();

        let store = SharedStore::new(
            "node-1",
            NodeRole::Primary,
            ConflictResolution::default(),
            StorageQuota::default(),
        )
        .with_persistence(&snapshot_path, &wal_path);

        store.load_from_disk().unwrap();

        // k1 应保持快照中的值（version 5 > WAL version 3，跳过）
        let k1 = store.get("k1").unwrap();
        assert_eq!(k1.value, serde_json::json!("snapshot"));
        assert_eq!(k1.version, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
