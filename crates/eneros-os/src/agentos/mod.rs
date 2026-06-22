//! AgentOS 内核 — Agent 进程管理、IPC、权限强制、资源配额、调度策略
//!
//! 本模块是 EnerOS AgentOS 架构的核心层（L3），提供 OS 级别的 Agent 管理：
//!
//! - `registry`: Agent 进程注册表（PID/状态/权限/配额元数据）
//! - `supervisor`: Agent 生命周期监督（spawn/stop/restart/崩溃重启）
//! - `ipc`: Agent 间消息传递（Unix socket + 共享内存 RT 通道）
//! - `authority`: 权限强制（Linux capabilities + seccomp）
//! - `quota`: 资源配额（cgroups v2 CPU/内存限制）
//! - `scheduler`: 调度策略（SCHED_FIFO RT Agent / SCHED_OTHER 普通 Agent）
//! - `seccomp`: seccomp BPF 系统调用沙箱（按 AuthorityLevel 限制 syscall）

pub mod registry;
pub mod supervisor;
pub mod ipc;
pub mod authority;
pub mod quota;
pub mod scheduler;
pub mod seccomp;

pub use registry::{AgentInfo, AgentRegistry, AgentStatus, AgentType};
pub use supervisor::{AgentSpawnConfig, AgentSupervisor, SupervisorError};
pub use ipc::{
    AgentIpcClient, AgentIpcServer, IpcTransport, AgentIpcConfig, NetworkNamespaceConfig,
    NetworkNamespaceManager, NamespaceError, ChannelConfig, SharedMemoryChannel,
};
pub use authority::{AuthorityEnforcer, AuthorityLevelSeccompExt, CapabilitySet};
pub use quota::{ResourceQuota, ResourceUsage, QuotaConfig};
pub use scheduler::{AgentScheduler, SchedulingPolicy};
pub use seccomp::{SeccompAction, SeccompError, SeccompProfile, SeccompRule};
