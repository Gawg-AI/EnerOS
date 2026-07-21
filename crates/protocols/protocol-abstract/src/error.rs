//! 协议抽象层错误类型.
//!
//! 定义 [`ProtocolError`]，覆盖点查找/适配器查找/地址匹配/读写/初始化/
//! 启动/配置/不支持等 9 类错误场景。

/// 协议抽象层错误（9 变体）.
///
/// 派生 `Debug`/`Clone`/`PartialEq`/`Eq`，便于在测试中精确匹配错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// 点未找到（point_id 不存在于适配器点表或路由表）。
    PointNotFound,
    /// 适配器未找到（协议类型未注册或路由目标缺失）。
    AdapterNotFound,
    /// 地址类型不匹配（如对 IEC 104 地址执行 Modbus 操作）。
    AddrTypeMismatch,
    /// 读操作失败（协议层返回错误）。
    ReadFailed,
    /// 写操作失败（协议层返回错误）。
    WriteFailed,
    /// 协议初始化失败（config 无效或资源不足）。
    ProtocolInit,
    /// 协议未启动（start() 前执行了读写/轮询）。
    ProtocolNotStarted,
    /// 配置无效（字段缺失或取值非法）。
    InvalidConfig,
    /// 不支持的操作（如对只读点写值，或协议不支持的功能）。
    Unsupported,
}
