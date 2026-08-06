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

/// 真实上传通道（v0.233.0，B3-10）：注入传输闭包（104/61850 上送由上层提供）。
pub struct RealUploadChannel<F> {
    /// 上传闭包（事件序列化 → 通道）。
    upload_fn: F,
    /// 连接状态。
    connected: bool,
}

impl<F> RealUploadChannel<F>
where
    F: FnMut(&[SoeEvent]) -> Result<(), SoeError>,
{
    /// 创建真实上传通道。
    pub fn new(upload_fn: F) -> Self {
        Self {
            upload_fn,
            connected: false,
        }
    }

    /// 标记通道连接。
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }
}

impl<F> UploadChannel for RealUploadChannel<F>
where
    F: FnMut(&[SoeEvent]) -> Result<(), SoeError>,
{
    fn upload(&mut self, events: &[SoeEvent]) -> Result<(), SoeError> {
        if !self.connected {
            return Err(SoeError::UploadError);
        }
        (self.upload_fn)(events)
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
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
