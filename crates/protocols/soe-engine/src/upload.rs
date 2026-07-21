//! SOE 事件上传通道抽象.

use crate::error::SoeError;
use crate::event::SoeEvent;

/// 事件上传通道 trait（D5：解耦网络栈）.
pub trait UploadChannel {
    /// 上传一批事件.
    fn upload(&mut self, events: &[SoeEvent]) -> Result<(), SoeError>;
    /// 通道是否已连接.
    fn is_connected(&self) -> bool;
}

/// 内存 mock 上传通道（用于测试与开发）.
#[derive(Debug, Clone, Default)]
pub struct MockUploadChannel {
    /// 上传调用次数.
    upload_count: u32,
    /// 已上传事件列表.
    uploaded_events: alloc::vec::Vec<SoeEvent>,
    /// 是否已连接.
    connected: bool,
}

impl MockUploadChannel {
    /// 构造（默认未连接）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置连接状态.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// 返回上传调用次数.
    pub fn upload_count(&self) -> u32 {
        self.upload_count
    }

    /// 返回已上传事件切片.
    pub fn uploaded_events(&self) -> &[SoeEvent] {
        &self.uploaded_events
    }
}

impl UploadChannel for MockUploadChannel {
    fn upload(&mut self, events: &[SoeEvent]) -> Result<(), SoeError> {
        if !self.connected {
            return Err(SoeError::UploadError);
        }
        self.uploaded_events.extend_from_slice(events);
        self.upload_count = self.upload_count.saturating_add(1);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}
