//! ProtocolManager — 多协议适配器管理 + 点路由.
//!
//! [`ProtocolManager`] 持有 `BTreeMap<ProtocolType, Box<dyn ProtocolAdapter>>`
//!（D4：使用 `Box` 而非 `Arc<RwLock<...>>`，no_std 单线程），
//! 并维护 `BTreeMap<PointId, ProtocolType>` 点路由表，将点读写请求路由到
//! 对应协议适配器。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_upa_model::{DataPoint, PointId, PointValue};

use crate::adapter::{AdapterState, ProtocolAdapter};
use crate::config::ProtocolType;
use crate::error::ProtocolError;

/// 协议管理器（多协议适配器 + 点路由）.
///
/// 使用 `BTreeMap` 而非 `HashMap`（no_std 友好、有序、key 可推导，D4）。
pub struct ProtocolManager {
    /// 协议类型 → 适配器实例。
    adapters: BTreeMap<ProtocolType, Box<dyn ProtocolAdapter>>,
    /// 点 ID → 协议类型（读写路由）。
    point_routes: BTreeMap<PointId, ProtocolType>,
}

impl ProtocolManager {
    /// 创建空管理器。
    pub fn new() -> Self {
        Self {
            adapters: BTreeMap::new(),
            point_routes: BTreeMap::new(),
        }
    }

    /// 注册适配器（按 `protocol_type()` 索引，重复注册覆盖旧实例）。
    pub fn register_adapter(&mut self, adapter: Box<dyn ProtocolAdapter>) {
        let pt = adapter.protocol_type();
        self.adapters.insert(pt, adapter);
    }

    /// 注册点路由（point_id → protocol_type）。
    pub fn register_route(&mut self, point_id: PointId, protocol_type: ProtocolType) {
        self.point_routes.insert(point_id, protocol_type);
    }

    /// 按路由读取单点。
    ///
    /// 路由缺失返回 [`ProtocolError::AdapterNotFound`]，
    /// 适配器内点缺失由适配器返回 [`ProtocolError::PointNotFound`]。
    pub fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        let pt = self
            .point_routes
            .get(&point_id)
            .copied()
            .ok_or(ProtocolError::AdapterNotFound)?;
        let adapter = self
            .adapters
            .get_mut(&pt)
            .ok_or(ProtocolError::AdapterNotFound)?;
        adapter.read_point(point_id)
    }

    /// 批量读取多点（逐点路由）。
    pub fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        point_ids.iter().map(|&id| self.read_point(id)).collect()
    }

    /// 按路由写入单点。
    pub fn write_point(
        &mut self,
        point_id: PointId,
        value: PointValue,
    ) -> Result<(), ProtocolError> {
        let pt = self
            .point_routes
            .get(&point_id)
            .copied()
            .ok_or(ProtocolError::AdapterNotFound)?;
        let adapter = self
            .adapters
            .get_mut(&pt)
            .ok_or(ProtocolError::AdapterNotFound)?;
        adapter.write_point(point_id, value)
    }

    /// 轮询所有适配器（`now_ms` 注入时间戳，D5）。
    ///
    /// 单个适配器 poll 失败不影响其他适配器（错误被忽略，仅推进状态）。
    pub fn poll_all(&mut self, now_ms: u64) {
        for adapter in self.adapters.values_mut() {
            let _ = adapter.poll(now_ms);
        }
    }

    /// 返回已注册适配器数量。
    pub fn adapter_count(&self) -> usize {
        self.adapters.len()
    }

    /// 查询指定协议类型适配器的状态。
    pub fn adapter_state(&self, protocol_type: ProtocolType) -> Option<AdapterState> {
        self.adapters.get(&protocol_type).map(|a| a.state())
    }
}

impl Default for ProtocolManager {
    fn default() -> Self {
        Self::new()
    }
}
