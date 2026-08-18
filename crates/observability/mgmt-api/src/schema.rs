//! 版本化 API schema — 请求/响应/动作/状态码 (OBS-010).
//!
//! 所有管理 API 请求携带 `api_version`，服务端按版本路由。
//! 当前版本 v1，支持 v1 兼容（major 匹配）。

use alloc::format;
use alloc::string::String;

use crate::rbac::{Permission, Role};

/// 当前 API 大版本号。
pub const CURRENT_VERSION: u32 = 1;

/// API 版本号（仅 major，无 minor/patch）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion(pub u32);

impl ApiVersion {
    /// 检查请求版本是否与当前版本兼容（major 匹配）。
    pub fn is_compatible(self) -> bool {
        self.0 == CURRENT_VERSION
    }
}

/// 管理 API 动作枚举。
///
/// 每个动作映射到一个 [`Permission`]，由 RBAC 策略检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiAction {
    // === 系统状态 ===
    /// 查询系统状态（健康、版本、uptime）。
    StatusQuery,
    /// 查询 SLO 指标。
    SloQuery,

    // === Agent 管理 ===
    /// 列出所有 Agent。
    AgentList,
    /// 启动 Agent。
    AgentStart,
    /// 停止 Agent。
    AgentStop,
    /// 重启 Agent。
    AgentRestart,

    // === 配置管理 ===
    /// 读取配置。
    ConfigGet,
    /// 写入配置。
    ConfigSet,

    // === 日志/诊断 ===
    /// 查询日志。
    LogQuery,
    /// 导出支持包。
    SupportBundleExport,
    /// 符号化崩溃转储。
    CrashDumpSymbolize,

    // === 更新/维护 ===
    /// 检查更新。
    UpdateCheck,
    /// 应用更新。
    UpdateApply,
    /// 回滚更新。
    UpdateRollback,
    /// 进入维护窗口。
    MaintenanceEnter,
    /// 退出维护窗口。
    MaintenanceExit,

    // === 备份/恢复 ===
    /// 创建备份。
    BackupCreate,
    /// 恢复备份。
    BackupRestore,

    // === 安全 ===
    /// 吊销证书/能力。
    Revoke,
    /// 工厂重置。
    FactoryReset,
    /// 安全擦除。
    SecureErase,
}

impl ApiAction {
    /// 返回此动作所需的权限。
    pub fn permission(self) -> Permission {
        match self {
            Self::StatusQuery
            | Self::SloQuery
            | Self::AgentList
            | Self::ConfigGet
            | Self::LogQuery
            | Self::UpdateCheck => Permission::SystemRead,

            Self::AgentStart
            | Self::AgentStop
            | Self::AgentRestart
            | Self::ConfigSet
            | Self::SupportBundleExport
            | Self::CrashDumpSymbolize
            | Self::UpdateApply
            | Self::UpdateRollback
            | Self::MaintenanceEnter
            | Self::MaintenanceExit
            | Self::BackupCreate
            | Self::BackupRestore => Permission::SystemWrite,

            Self::Revoke | Self::FactoryReset | Self::SecureErase => Permission::SecurityAdmin,
        }
    }

    /// 动作名称（用于序列化）。
    pub fn name(self) -> &'static str {
        match self {
            Self::StatusQuery => "status.query",
            Self::SloQuery => "slo.query",
            Self::AgentList => "agent.list",
            Self::AgentStart => "agent.start",
            Self::AgentStop => "agent.stop",
            Self::AgentRestart => "agent.restart",
            Self::ConfigGet => "config.get",
            Self::ConfigSet => "config.set",
            Self::LogQuery => "log.query",
            Self::SupportBundleExport => "support-bundle.export",
            Self::CrashDumpSymbolize => "crash-dump.symbolize",
            Self::UpdateCheck => "update.check",
            Self::UpdateApply => "update.apply",
            Self::UpdateRollback => "update.rollback",
            Self::MaintenanceEnter => "maintenance.enter",
            Self::MaintenanceExit => "maintenance.exit",
            Self::BackupCreate => "backup.create",
            Self::BackupRestore => "backup.restore",
            Self::Revoke => "security.revoke",
            Self::FactoryReset => "security.factory-reset",
            Self::SecureErase => "security.secure-erase",
        }
    }
}

/// HTTP 风格状态码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusCode {
    /// 200 — 成功。
    Ok,
    /// 400 — 请求格式错误。
    BadRequest,
    /// 401 — 未认证。
    Unauthorized,
    /// 403 — 权限不足。
    Forbidden,
    /// 404 — 资源不存在。
    NotFound,
    /// 409 — 冲突（状态不允许）。
    Conflict,
    /// 429 — 限流。
    TooManyRequests,
    /// 500 — 内部错误。
    InternalError,
    /// 501 — 不支持的 API 版本。
    UnsupportedVersion,
}

impl StatusCode {
    pub fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::TooManyRequests => 429,
            Self::InternalError => 500,
            Self::UnsupportedVersion => 501,
        }
    }
}

/// 管理 API 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    /// 请求方声明的 API 版本。
    pub api_version: ApiVersion,
    /// 请求方角色。
    pub role: Role,
    /// 请求的动作。
    pub action: ApiAction,
    /// 请求正文（JSON 或其他文本格式）。
    pub payload: String,
    /// 幂等键（可选）。相同键在 24h 窗口内返回缓存响应。
    pub idempotency_key: Option<String>,
}

/// 管理 API 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    /// 请求 ID（单调递增）。
    pub request_id: u64,
    /// 响应使用的 API 版本。
    pub api_version: ApiVersion,
    /// 状态码。
    pub status: StatusCode,
    /// 响应正文。
    pub body: String,
    /// 是否为幂等重放（true = 返回缓存响应）。
    pub replayed: bool,
}

impl ApiResponse {
    /// 序列化为 JSON（传输层/证据用；无 serde 依赖的极简序列化）。
    pub fn to_json(&self) -> String {
        alloc::format!(
            "{{\"request_id\":{},\"version\":{},\"status\":{},\"body\":\"{}\",\"replayed\":{}}}",
            self.request_id,
            self.api_version.0,
            self.status.code(),
            self.body,
            self.replayed
        )
    }

    /// 构造错误响应。
    pub fn error(request_id: u64, status: StatusCode, message: &str) -> Self {
        Self {
            request_id,
            api_version: ApiVersion(CURRENT_VERSION),
            status,
            body: format!(r#"{{"error":"{}"}}"#, message.replace('"', "\\\"")),
            replayed: false,
        }
    }

    /// 构造成功响应。
    pub fn ok(request_id: u64, body: &str) -> Self {
        Self {
            request_id,
            api_version: ApiVersion(CURRENT_VERSION),
            status: StatusCode::Ok,
            body: String::from(body),
            replayed: false,
        }
    }
}

/// API 层错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 请求解析失败。
    ParseError(String),
    /// 版本不兼容。
    UnsupportedVersion(u32),
    /// 未认证。
    Unauthorized,
    /// 权限不足。
    Forbidden(Role, Permission),
    /// 限流。
    RateLimited { retry_after_secs: u64 },
}
