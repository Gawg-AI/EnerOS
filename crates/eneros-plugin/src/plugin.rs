//! Plugin trait 定义 — 所有 EnerOS 插件必须实现的标准接口
//!
//! 定义插件的标准生命周期接口：init -> start -> stop。
//! 插件通过动态库加载后，由 loader 调用此 trait 的方法管理生命周期。

use async_trait::async_trait;

use crate::error::PluginResult;
use crate::manifest::{PluginMetadata, PluginType};

/// 插件 trait — 所有 EnerOS 插件必须实现
///
/// 生命周期顺序：`init` -> `start` -> `stop`。
/// 任何阶段失败将返回对应的 `PluginError`，由 loader 据此更新状态机。
#[async_trait]
pub trait Plugin: Send + Sync {
    /// 返回插件元数据
    fn metadata(&self) -> &PluginMetadata;

    /// 返回插件类型
    fn plugin_type(&self) -> PluginType;

    /// 初始化插件（分配资源、建立连接等）
    async fn init(&mut self) -> PluginResult<()>;

    /// 启动插件（开始处理请求）
    async fn start(&mut self) -> PluginResult<()>;

    /// 停止插件（释放资源、关闭连接）
    async fn stop(&mut self) -> PluginResult<()>;
}
