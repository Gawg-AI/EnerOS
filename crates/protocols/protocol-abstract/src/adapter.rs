//! ProtocolAdapter trait — 协议适配器生命周期 + 点访问.
//!
//! [`ProtocolAdapter`] 继承 [`crate::access::PointAccess`]，增加初始化/启动/
//! 停止/轮询/状态查询能力，是协议抽象层对接具体协议栈（Modbus/IEC 104/CAN）
//! 的统一抽象。

use crate::access::PointAccess;
use crate::config::AdapterConfig;
use crate::error::ProtocolError;

/// 适配器状态机.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterState {
    /// 未初始化（`new` 后、`init` 前）。
    Uninitialized,
    /// 已初始化（`init` 成功后、`start` 前）。
    Initialized,
    /// 运行中（`start` 成功后）。
    Running,
    /// 已停止（`stop` 后）。
    Stopped,
    /// 错误态（生命周期方法或读写失败）。
    Error,
}

/// 协议适配器 trait（继承 [`PointAccess`]）.
///
/// 生命周期：`init` → `start` → `poll`（循环）→ `stop`。
/// `poll(now_ms)` 接受注入时间戳（D5），与 v0.50.0 D1 一致。
pub trait ProtocolAdapter: PointAccess {
    /// 初始化适配器（加载设备配置、分配资源）。
    fn init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError>;

    /// 启动适配器（建立连接、开始收发）。
    fn start(&mut self) -> Result<(), ProtocolError>;

    /// 停止适配器（断开连接、释放资源）。
    fn stop(&mut self) -> Result<(), ProtocolError>;

    /// 周期轮询（处理收发、超时、心跳等），`now_ms` 为注入时间戳（D5）。
    fn poll(&mut self, now_ms: u64) -> Result<(), ProtocolError>;

    /// 查询当前适配器状态。
    fn state(&self) -> AdapterState;
}
