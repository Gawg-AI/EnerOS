//! Agent 检查点 — CheckpointStore / InMemoryCheckpointStore / Checkpointable
//!
//! # 设计
//! - `CheckpointStore` 是检查点存储抽象 trait（D1 偏差：非 struct，因 agent crate 零依赖无 FileSystem）
//! - `InMemoryCheckpointStore` 为默认内存实现（基于 `RefCell<BTreeMap>`，单线程内部可变性）
//! - `Checkpointable` trait 供 Agent 实现者提供自定义状态保存/恢复（D8 偏差：CrashRecovery 不直接调用）
//!
//! # 偏差声明
//! - D1: `CheckpointStore` 为 trait（蓝图为 struct with `Box<dyn FileSystem>`），因 agent crate 保持零外部依赖
//! - D8: `Checkpointable` trait 定义但 CrashRecovery 不直接调用，调用方负责通过此 trait 序列化/反序列化
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::AgentError;
use crate::id::AgentId;

/// 检查点存储 trait（object-safe，D1 偏差）.
///
/// 作为检查点持久化的抽象层，支持 save / load / delete 操作。
/// 默认实现 [`InMemoryCheckpointStore`] 使用内存 BTreeMap；
/// 生产环境可注入文件系统后端。
pub trait CheckpointStore {
    /// 保存检查点数据.
    ///
    /// 若该 `id` 已有检查点，覆盖旧数据。
    fn save(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError>;

    /// 加载检查点数据.
    ///
    /// 返回 `Ok(None)` 表示该 `id` 无检查点。
    fn load(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError>;

    /// 删除检查点数据.
    ///
    /// 若该 `id` 无检查点，静默返回 `Ok(())`。
    fn delete(&self, id: AgentId) -> Result<(), AgentError>;
}

/// 内存检查点存储（D1 默认实现）.
///
/// 基于 `RefCell<BTreeMap<AgentId, Vec<u8>>>` 的内存实现，用于测试与非持久化场景。
/// 使用 `RefCell` 提供 `&self` 签名下的内部可变性（单线程 no_std 标准模式）。
/// 生产环境应替换为文件系统后端。
#[derive(Debug, Default)]
pub struct InMemoryCheckpointStore {
    store: RefCell<BTreeMap<AgentId, Vec<u8>>>,
}

impl InMemoryCheckpointStore {
    /// 创建空的内存检查点存储.
    pub fn new() -> Self {
        InMemoryCheckpointStore {
            store: RefCell::new(BTreeMap::new()),
        }
    }
}

impl CheckpointStore for InMemoryCheckpointStore {
    fn save(&self, id: AgentId, data: &[u8]) -> Result<(), AgentError> {
        self.store.borrow_mut().insert(id, Vec::from(data));
        Ok(())
    }

    fn load(&self, id: AgentId) -> Result<Option<Vec<u8>>, AgentError> {
        Ok(self.store.borrow().get(&id).cloned())
    }

    fn delete(&self, id: AgentId) -> Result<(), AgentError> {
        self.store.borrow_mut().remove(&id);
        Ok(())
    }
}

/// 可检查点 trait（object-safe，D8 偏差）.
///
/// 供 Agent 实现者提供自定义的状态保存/恢复逻辑。
/// CrashRecovery 不直接调用此 trait；调用方（如编排器）负责：
/// 1. 通过 `Checkpointable::save_state` 序列化 Agent 状态
/// 2. 通过 `CrashRecovery::save_checkpoint` 持久化到 CheckpointStore
/// 3. 恢复时通过 `CrashRecovery::restore_checkpoint` 加载字节
/// 4. 通过 `Checkpointable::restore_state` 应用到 Agent 实例
pub trait Checkpointable {
    /// 保存当前状态为字节序列.
    fn save_state(&self) -> Vec<u8>;

    /// 从字节序列恢复状态.
    fn restore_state(&mut self, data: &[u8]) -> Result<(), AgentError>;
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::rc::Rc;
    use alloc::vec::Vec;

    use super::*;
    use crate::AgentId;

    /// 测试辅助：简单的可检查点 Agent（u32 state，4 字节小端序列化）.
    struct TestAgent {
        state: u32,
    }

    impl TestAgent {
        fn new(state: u32) -> Self {
            TestAgent { state }
        }
    }

    impl Checkpointable for TestAgent {
        fn save_state(&self) -> Vec<u8> {
            self.state.to_le_bytes().to_vec()
        }

        fn restore_state(&mut self, data: &[u8]) -> Result<(), AgentError> {
            if data.len() < 4 {
                return Err(AgentError::CheckpointCorrupted {
                    agent_id: AgentId::ZERO,
                });
            }
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&data[..4]);
            self.state = u32::from_le_bytes(bytes);
            Ok(())
        }
    }

    // 1. save 后 load 返回相同字节.
    #[test]
    fn test_inmemory_save_load() {
        let store = InMemoryCheckpointStore::new();
        let id = AgentId::generate();
        let data: Vec<u8> = vec![1, 2, 3, 4, 5];
        store.save(id, &data).unwrap();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, Some(data));
    }

    // 2. 未保存的 id，load 返回 Ok(None).
    #[test]
    fn test_inmemory_load_nonexistent() {
        let store = InMemoryCheckpointStore::new();
        let id = AgentId::generate();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, None);
    }

    // 3. save 后 delete，再 load 返回 Ok(None).
    #[test]
    fn test_inmemory_delete() {
        let store = InMemoryCheckpointStore::new();
        let id = AgentId::generate();
        let data: Vec<u8> = vec![10, 20, 30];
        store.save(id, &data).unwrap();
        store.delete(id).unwrap();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, None);
    }

    // 4. 对未保存的 id delete 返回 Ok(())，不报错.
    #[test]
    fn test_inmemory_delete_nonexistent() {
        let store = InMemoryCheckpointStore::new();
        let id = AgentId::generate();
        let result = store.delete(id);
        assert!(result.is_ok());
    }

    // 5. 同一 id 二次 save 覆盖旧数据.
    #[test]
    fn test_inmemory_overwrite() {
        let store = InMemoryCheckpointStore::new();
        let id = AgentId::generate();
        let data1: Vec<u8> = vec![1, 1, 1];
        let data2: Vec<u8> = vec![2, 2, 2, 2];
        store.save(id, &data1).unwrap();
        store.save(id, &data2).unwrap();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, Some(data2));
    }

    // 6. Default 构造空存储，load 返回 None.
    #[test]
    fn test_inmemory_default() {
        let store = InMemoryCheckpointStore::default();
        let id = AgentId::generate();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, None);
    }

    // 7. 两个不同 id 的数据独立存取.
    #[test]
    fn test_inmemory_multiple_agents() {
        let store = InMemoryCheckpointStore::new();
        let id1 = AgentId::generate();
        let id2 = AgentId::generate();
        let data1: Vec<u8> = vec![0xAA];
        let data2: Vec<u8> = vec![0xBB, 0xCC];
        store.save(id1, &data1).unwrap();
        store.save(id2, &data2).unwrap();
        assert_eq!(store.load(id1).unwrap(), Some(data1));
        assert_eq!(store.load(id2).unwrap(), Some(data2));
    }

    // 8. 通过 Rc<dyn CheckpointStore> trait object 调用 save/load/delete.
    #[test]
    fn test_checkpoint_store_trait_object() {
        let store: Rc<dyn CheckpointStore> = Rc::new(InMemoryCheckpointStore::new());
        let id = AgentId::generate();
        let data: Vec<u8> = vec![7, 8, 9];
        store.save(id, &data).unwrap();
        let loaded = store.load(id).unwrap();
        assert_eq!(loaded, Some(data.clone()));
        store.delete(id).unwrap();
        let loaded_after_delete = store.load(id).unwrap();
        assert_eq!(loaded_after_delete, None);
    }

    // 9. Checkpointable trait object-safe：Box<dyn Checkpointable> 调用 save/restore.
    #[test]
    fn test_checkpointable_object_safe() {
        let agent: Box<dyn Checkpointable> = Box::new(TestAgent::new(99));
        let saved = agent.save_state();
        assert_eq!(saved, 99u32.to_le_bytes().to_vec());
        // 通过新实例 restore 后再 save_state 验证往返一致.
        let mut restored: Box<dyn Checkpointable> = Box::new(TestAgent::new(0));
        restored.restore_state(&saved).unwrap();
        assert_eq!(restored.save_state(), saved);
    }

    // 10. save_state 后 restore_state 到新实例，状态恢复正确.
    #[test]
    fn test_checkpointable_restore() {
        let agent = TestAgent::new(42);
        let saved = agent.save_state();
        let mut fresh = TestAgent::new(0);
        fresh.restore_state(&saved).unwrap();
        assert_eq!(fresh.state, 42);
    }
}
