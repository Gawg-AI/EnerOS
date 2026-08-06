//! Agent trait 与核心类型（SDK v1.0 稳定 API）。
//!
//! 外部开发者实现 [`Agent`] trait；宿主经 IPC wire codec（`crate::wire`）
//! 与 Agent 进程通信（HYG-002：`register_agent!` 宏已按 AGT-003 删除，
//! 不再导出 extern "C" -> *mut dyn 入口）。
//!
//! # API 稳定性承诺
//! v1.0 承诺本模块公共接口向后兼容。

use heapless::String as HString;

use crate::context::Context;

/// SDK 版本号。
pub const SDK_VERSION: &str = "1.0.0";

/// Agent 错误类型（SDK 公共）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// 初始化失败
    InitFailed(HString<128>),
    /// IPC 通信错误
    IpcError(HString<128>),
    /// 存储错误
    StorageError(HString<128>),
    /// 配置错误
    ConfigError(HString<128>),
    /// 功能不支持
    NotSupported,
    /// 生命周期状态非法
    InvalidState,
    /// 超时
    Timeout,
}

impl core::fmt::Display for AgentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AgentError::InitFailed(msg) => write!(f, "init failed: {}", msg),
            AgentError::IpcError(msg) => write!(f, "ipc error: {}", msg),
            AgentError::StorageError(msg) => write!(f, "storage error: {}", msg),
            AgentError::ConfigError(msg) => write!(f, "config error: {}", msg),
            AgentError::NotSupported => write!(f, "not supported"),
            AgentError::InvalidState => write!(f, "invalid lifecycle state"),
            AgentError::Timeout => write!(f, "timeout"),
        }
    }
}

/// Agent 描述符（SDK 级别，用于外部开发者声明 Agent 元信息）。
///
/// 与内部 `eneros-agent` crate 的 `AgentDescriptor`（13 字段运行时结构）不同，
/// 本结构仅包含开发者需要提供的静态元信息。
#[derive(Debug, Clone)]
pub struct AgentDescriptor {
    /// Agent 名称（≤64 字节）
    pub name: HString<64>,
    /// Agent 版本（≤32 字节，语义化版本号）
    pub version: HString<32>,
    /// 作者（≤64 字节）
    pub author: HString<64>,
    /// 描述（≤256 字节）
    pub description: HString<256>,
    /// 能力列表（≤8 项，每项 ≤32 字节）
    pub capabilities: [Option<HString<32>>; 8],
    /// 配置 schema（JSON，≤512 字节）
    pub config_schema: HString<512>,
}

impl AgentDescriptor {
    /// 创建最小描述符（仅名称和版本）。
    pub fn minimal(name: &str, version: &str) -> Self {
        Self {
            name: HString::<64>::try_from(name).unwrap_or_default(),
            version: HString::<32>::try_from(version).unwrap_or_default(),
            author: HString::<64>::new(),
            description: HString::<256>::new(),
            capabilities: Default::default(),
            config_schema: HString::<512>::new(),
        }
    }

    /// 设置作者。
    pub fn with_author(mut self, author: &str) -> Self {
        self.author = HString::<64>::try_from(author).unwrap_or_default();
        self
    }

    /// 设置描述。
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = HString::<256>::try_from(desc).unwrap_or_default();
        self
    }

    /// 添加能力。
    pub fn with_capability(mut self, index: usize, cap: &str) -> Self {
        if index < 8 {
            self.capabilities[index] = Some(HString::<32>::try_from(cap).unwrap_or_default());
        }
        self
    }

    /// 设置配置 schema。
    pub fn with_config_schema(mut self, schema: &str) -> Self {
        self.config_schema = HString::<512>::try_from(schema).unwrap_or_default();
        self
    }
}

/// Agent trait：外部开发者实现此接口（SDK v1.0 稳定 API）。
///
/// # 生命周期
/// ```text
/// new() → on_init → on_start → on_tick (循环) → on_stop → on_destroy
/// ```
///
/// # 示例
/// ```text
/// use eneros_agent_sdk::{Agent, AgentDescriptor, AgentError, Context};
///
/// struct HelloAgent;
///
/// impl HelloAgent {
///     fn new() -> Self { Self }
/// }
///
/// impl Agent for HelloAgent {
///     fn descriptor(&self) -> AgentDescriptor {
///         AgentDescriptor::minimal("hello", "1.0.0")
///     }
///     fn on_init(&mut self, _ctx: &mut Context) -> Result<(), AgentError> { Ok(()) }
///     fn on_start(&mut self, _ctx: &mut Context) -> Result<(), AgentError> { Ok(()) }
///     fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> { Ok(()) }
///     fn on_stop(&mut self, _ctx: &mut Context) -> Result<(), AgentError> { Ok(()) }
///     fn on_destroy(&mut self, _ctx: &mut Context) {}
/// }
/// ```
pub trait Agent: Send + Sync {
    /// 获取 Agent 描述符。
    fn descriptor(&self) -> AgentDescriptor;

    /// 初始化回调（加载配置、分配资源）。
    fn on_init(&mut self, ctx: &mut Context) -> Result<(), AgentError>;

    /// 启动回调（启动主循环）。
    fn on_start(&mut self, ctx: &mut Context) -> Result<(), AgentError>;

    /// 周期执行回调（由 Runtime 按配置周期调用）。
    fn on_tick(&mut self, ctx: &mut Context) -> Result<(), AgentError>;

    /// 停止回调（释放资源、保存状态）。
    fn on_stop(&mut self, ctx: &mut Context) -> Result<(), AgentError>;

    /// 销毁回调（最终清理，不可失败）。
    fn on_destroy(&mut self, ctx: &mut Context);
}

/// Agent ID（SDK 级别，由 Runtime 分配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentId(pub u64);

impl AgentId {
    /// 零值（未分配）。
    pub const ZERO: AgentId = AgentId(0);
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// AGT1：描述符构建器。
    #[test]
    fn agt1_descriptor_builder() {
        let desc = AgentDescriptor::minimal("test-agent", "1.0.0")
            .with_author("EnerOS Team")
            .with_description("A test agent")
            .with_capability(0, "data-collection")
            .with_capability(1, "alarm")
            .with_config_schema(r#"{"interval_ms": "u32"}"#);

        assert_eq!(desc.name.as_str(), "test-agent");
        assert_eq!(desc.version.as_str(), "1.0.0");
        assert_eq!(desc.author.as_str(), "EnerOS Team");
        assert_eq!(desc.description.as_str(), "A test agent");
        assert_eq!(
            desc.capabilities[0].as_ref().unwrap().as_str(),
            "data-collection"
        );
        assert_eq!(desc.capabilities[1].as_ref().unwrap().as_str(), "alarm");
        assert!(desc.capabilities[2].is_none());
    }

    /// AGT2：错误类型 Display。
    #[test]
    fn agt2_error_display() {
        let e = AgentError::InitFailed(HString::<128>::try_from("timeout").unwrap());
        assert_eq!(format!("{}", e), "init failed: timeout");

        assert_eq!(format!("{}", AgentError::NotSupported), "not supported");
        assert_eq!(
            format!("{}", AgentError::InvalidState),
            "invalid lifecycle state"
        );
        assert_eq!(format!("{}", AgentError::Timeout), "timeout");
    }

    /// AGT3：错误类型 Eq。
    #[test]
    fn agt3_error_eq() {
        let e1 = AgentError::NotSupported;
        let e2 = AgentError::NotSupported;
        assert_eq!(e1, e2);

        let e3 = AgentError::Timeout;
        assert_ne!(e1, e3);
    }

    /// AGT4：AgentId 零值。
    #[test]
    fn agt4_agent_id_zero() {
        assert_eq!(AgentId::ZERO, AgentId(0));
        assert_ne!(AgentId::ZERO, AgentId(1));
    }

    /// AGT5：SDK 版本常量。
    #[test]
    fn agt5_sdk_version() {
        assert_eq!(SDK_VERSION, "1.0.0");
    }
}
