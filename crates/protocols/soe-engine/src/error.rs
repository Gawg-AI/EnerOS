//! SOE 引擎错误类型.

/// SOE 引擎错误.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoeError {
    /// 事件队列已满.
    QueueFull,
    /// 持久化存储错误.
    StorageError,
    /// 上传通道错误.
    UploadError,
    /// 事件未找到.
    NotFound,
    /// 参数非法.
    InvalidArgument,
}
