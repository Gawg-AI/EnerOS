//! 蓝图 40 WP-40.12：durable intent（投递意图持久化——重启对账基础）。
//!
//! `ControlDispatch` 在投递前把意图写入 [`IntentStore`]；device-agent
//! 重启/投递超时后经 `pending()`/`find()` 对账（先读回/查询状态，不盲重试
//! 非幂等命令）。本卷提供 trait + 内存实现（服务装配可绑定持久介质——
//! 依赖 EL0 存储句柄移交，见 evidence/40 RECON §0）；`update_state` 记录
//! 终态供恢复审计。

use alloc::string::String;
use alloc::vec::Vec;

use crate::dispatch::DispatchState;

/// 意图存储错误（写失败必须显式——不吞）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentStoreError {
    /// 存储不可用/IO 失败。
    Unavailable(String),
    /// 容量上限。
    Full,
    /// 未知命令 ID。
    Unknown,
}

/// 一条投递意图（WP-40.12 对账记录）。
#[derive(Debug, Clone)]
pub struct IntentRecord {
    /// 命令 ID（信封 command_id）。
    pub command_id: [u8; 16],
    /// 关联 ID。
    pub correlation_id: [u8; 16],
    /// 签发时刻（单调毫秒）。
    pub issued_at_ms: u64,
    /// 过期时刻（单调毫秒）。
    pub expires_at_ms: u64,
    /// 目标设备。
    pub target_device: String,
    /// 功率载荷（kW）。
    pub power_kw: f64,
    /// 输入遥测摘要。
    pub input_digest: Option<[u8; 32]>,
    /// 当前终态（投递前 Persisted；对账/回执后更新）。
    pub state: DispatchState,
}

/// durable intent 存储 trait（唯一持久化 seam——调用方提供实现）。
pub trait IntentStore {
    /// 追加意图（投递前调用）。
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError>;
    /// 更新终态（回执/对账后）。
    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError>;
    /// 按命令 ID 查询。
    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord>;
    /// 未决（非终态）意图——重启对账输入。
    fn pending(&self) -> Vec<IntentRecord>;
}

/// 内存实现（有界；测试/装配期——持久介质绑定见 WP-40.12 对账）。
#[derive(Debug, Clone)]
pub struct InMemoryIntentStore {
    /// 记录（FIFO；容量上限）。
    records: Vec<IntentRecord>,
    /// 容量上限。
    cap: usize,
}

impl InMemoryIntentStore {
    /// 创建（容量上限）。
    pub fn new(cap: usize) -> Self {
        Self {
            records: Vec::new(),
            cap,
        }
    }
}

impl Default for InMemoryIntentStore {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl IntentStore for InMemoryIntentStore {
    fn append(&mut self, rec: &IntentRecord) -> Result<(), IntentStoreError> {
        if self.records.len() >= self.cap {
            return Err(IntentStoreError::Full);
        }
        self.records.push(rec.clone());
        Ok(())
    }

    fn update_state(
        &mut self,
        command_id: &[u8; 16],
        state: DispatchState,
    ) -> Result<(), IntentStoreError> {
        let Some(rec) = self
            .records
            .iter_mut()
            .find(|r| &r.command_id == command_id)
        else {
            return Err(IntentStoreError::Unknown);
        };
        rec.state = state;
        Ok(())
    }

    fn find(&self, command_id: &[u8; 16]) -> Option<IntentRecord> {
        self.records
            .iter()
            .find(|r| &r.command_id == command_id)
            .cloned()
    }

    fn pending(&self) -> Vec<IntentRecord> {
        self.records
            .iter()
            .filter(|r| {
                !matches!(
                    r.state,
                    DispatchState::Committed | DispatchState::Rejected | DispatchState::Failed
                )
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u8) -> IntentRecord {
        IntentRecord {
            command_id: [id; 16],
            correlation_id: [0xAA; 16],
            issued_at_ms: 1_000,
            expires_at_ms: 1_300,
            target_device: String::from("pcs"),
            power_kw: 5.0,
            input_digest: None,
            state: DispatchState::Persisted,
        }
    }

    #[test]
    fn append_find_update() {
        let mut store = InMemoryIntentStore::new(16);
        store.append(&rec(1)).expect("append");
        let r = store.find(&[1; 16]).expect("find");
        assert_eq!(r.state, DispatchState::Persisted);
        store
            .update_state(&[1; 16], DispatchState::Committed)
            .expect("update");
        assert_eq!(
            store.find(&[1; 16]).unwrap().state,
            DispatchState::Committed
        );
        assert!(store.pending().is_empty(), "Committed 不再是未决");
    }

    #[test]
    fn pending_lists_inflight() {
        let mut store = InMemoryIntentStore::new(16);
        store.append(&rec(1)).expect("a");
        store.append(&rec(2)).expect("b");
        store
            .update_state(&[2; 16], DispatchState::Indeterminate)
            .expect("u");
        let pend = store.pending();
        assert_eq!(pend.len(), 2, "Persisted + Indeterminate 都未决（需对账）");
    }

    #[test]
    fn full_rejects() {
        let mut store = InMemoryIntentStore::new(1);
        store.append(&rec(1)).expect("a");
        assert_eq!(store.append(&rec(2)), Err(IntentStoreError::Full));
    }

    #[test]
    fn unknown_update_rejected() {
        let mut store = InMemoryIntentStore::new(4);
        assert_eq!(
            store.update_state(&[9; 16], DispatchState::Committed),
            Err(IntentStoreError::Unknown)
        );
    }
}
