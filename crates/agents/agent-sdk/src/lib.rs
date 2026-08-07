//! # EnerOS Agent SDK v1.0
//!
//! 外部开发者基于此 SDK 开发 EnerOS Agent。
//!
//! ## 快速开始
//!
//! Agent 是编译为签名的 ELF64 PIE 二进制的独立进程（MASTER §G6 AGT-003）。
//! 开发者实现 [`Agent`] trait，编译为 AArch64 ELF，由 supervisor 通过
//! `LoadedElf::parse` 加载、`ProcessHandle` 隔离运行。无动态插件、无跨二进制
//! trait-object ABI。
//!
//! ```text
//! use eneros_agent_sdk::{Agent, AgentDescriptor, AgentError, Context};
//!
//! struct HelloAgent;
//!
//! impl HelloAgent {
//!     fn new() -> Self { Self }
//! }
//!
//! impl Agent for HelloAgent {
//!     fn descriptor(&self) -> AgentDescriptor {
//!         AgentDescriptor::minimal("hello", "1.0.0")
//!             .with_author("EnerOS Team")
//!             .with_description("Hello World Agent")
//!     }
//!     fn on_init(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
//!         ctx.logger.info("HelloAgent initialized");
//!         Ok(())
//!     }
//!     fn on_start(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
//!         ctx.logger.info("HelloAgent started");
//!         Ok(())
//!     }
//!     fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
//!         Ok(())
//!     }
//!     fn on_stop(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
//!         ctx.logger.info("HelloAgent stopped");
//!         Ok(())
//!     }
//!     fn on_destroy(&mut self, _ctx: &mut Context) {}
//! }
//! ```
//!
//! ## 模块结构
//!
//! - [`agent`] — Agent trait、AgentDescriptor、AgentError
//! - [`context`] — Context（运行时环境）
//! - [`lifecycle`] — 生命周期状态机
//! - [`ipc`] — IPC 通信（wire codec AGT-006/007）
//! - [`wire`] — IPC 二进制 wire codec
//! - [`storage`] — 键值存储
//! - [`logger`] — 日志
//! - [`config`] — 配置
//!
//! ## no_std 合规
//!
//! 本 crate 完全 no_std，仅依赖 `alloc` 和 `heapless`。
//!
//! ## API 稳定性
//!
//! v1.0 承诺公共 API 向后兼容。

#![cfg_attr(not(test), no_std)]
#![allow(unexpected_cfgs)] // sel4_target 由 sel4-sys build.rs 注入
// AgentError uses HString<128> (136 bytes) for no_std error messages without
// allocation. The large Err variant is an intentional trade-off.
#![allow(clippy::result_large_err)]

extern crate alloc;

pub mod agent;
pub mod config;
pub mod context;
pub mod ipc;
#[cfg(any(test, feature = "sel4"))]
pub mod ipc_sel4;

// sel4 feature 仅限目标构建（seL4 userland）：host 上显式拒绝。
#[cfg(all(feature = "sel4", not(sel4_target)))]
compile_error!(
    "feature `sel4` requires the seL4 target (aarch64-unknown-none);      host builds must not enable it (use the default SPSC queue)"
);
pub mod lifecycle;
pub mod logger;
pub mod storage;
pub mod wire;

// 重导出核心类型
pub use agent::{Agent, AgentDescriptor, AgentError, AgentId, SDK_VERSION};
pub use config::Config;
pub use context::Context;
#[cfg(any(test, feature = "mock"))]
pub use ipc::IpcClient;
/// seL4 IPC buffer 消息寄存器区上限（120 字节，蓝图/20 R2-2）。
pub use ipc::MAX_MSG_SIZE;
pub use lifecycle::{LifecycleManager, LifecycleState};
pub use logger::{LogLevel, Logger};
pub use storage::Storage;

// NOTE: The `register_agent!` macro (extern "C" -> *mut dyn Agent) was removed
// because MASTER §G6 AGT-003 prohibits cross-binary Rust ABI / dynamic plugin
// loading. Agents are signed ELF64 PIE binaries loaded via `LoadedElf::parse`
// and run as isolated EL0 processes (`ProcessHandle`). Communication is via
// the IPC wire codec (AGT-006/007), not trait-object method calls.

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// SDK1：版本常量。
    #[test]
    fn sdk1_version() {
        assert_eq!(SDK_VERSION, "1.0.0");
    }

    /// SDK2：完整 Agent 实现示例。
    #[test]
    fn sdk2_full_agent_example() {
        struct TestAgent {
            tick_count: u32,
        }

        impl TestAgent {
            fn new() -> Self {
                Self { tick_count: 0 }
            }
        }

        impl Agent for TestAgent {
            fn descriptor(&self) -> AgentDescriptor {
                AgentDescriptor::minimal("test", "1.0.0")
            }

            fn on_init(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
                ctx.logger.info("init");
                Ok(())
            }

            fn on_start(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
                ctx.logger.info("start");
                Ok(())
            }

            fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
                self.tick_count += 1;
                Ok(())
            }

            fn on_stop(&mut self, ctx: &mut Context) -> Result<(), AgentError> {
                ctx.logger.info("stop");
                Ok(())
            }

            fn on_destroy(&mut self, _ctx: &mut Context) {}
        }

        let mut agent = TestAgent::new();
        let mut ctx = Context::new(AgentId(1));

        // 生命周期
        assert!(agent.on_init(&mut ctx).is_ok());
        assert!(agent.on_start(&mut ctx).is_ok());
        assert!(agent.on_tick(&mut ctx).is_ok());
        assert!(agent.on_tick(&mut ctx).is_ok());
        assert_eq!(agent.tick_count, 2);
        assert!(agent.on_stop(&mut ctx).is_ok());
        agent.on_destroy(&mut ctx);

        // 日志计数
        assert_eq!(ctx.logger.log_count(), 3);
    }

    /// SDK3：Context 组件交互。
    #[test]
    fn sdk3_context_components() {
        let mut ctx = Context::new(AgentId(42));

        // 配置
        ctx.config.set("interval", "1000").expect("设置应成功");
        assert_eq!(ctx.config.get_u32("interval").expect("获取应成功"), 1000);

        // 存储
        ctx.storage.write("state", b"running").expect("写入应成功");
        let state = ctx.storage.read("state").expect("读取应成功");
        assert_eq!(state, b"running");

        // IPC
        ctx.ipc.send("other-agent", b"hello").expect("发送应成功");
        assert_eq!(ctx.ipc.send_count(), 1);

        // 日志
        ctx.logger.info("test message");
        assert_eq!(ctx.logger.last_message(), "test message");
    }

    /// SDK4：生命周期管理器集成。
    #[test]
    fn sdk4_lifecycle_integration() {
        let mut lcm = LifecycleManager::new();
        assert_eq!(lcm.state(), LifecycleState::Created);

        assert!(lcm.transition_to(LifecycleState::Initialized));
        assert!(lcm.transition_to(LifecycleState::Running));
        assert!(lcm.is_running());

        assert!(lcm.transition_to(LifecycleState::Stopped));
        assert!(lcm.transition_to(LifecycleState::Destroyed));
        assert!(lcm.is_destroyed());
    }

    /// SDK5：Agent trait object safety。
    #[test]
    fn sdk5_agent_object_safe() {
        struct DummyAgent;
        impl DummyAgent {
            fn new() -> Self {
                Self
            }
        }
        impl Agent for DummyAgent {
            fn descriptor(&self) -> AgentDescriptor {
                AgentDescriptor::minimal("dummy", "0.1.0")
            }
            fn on_init(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_start(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_tick(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_stop(&mut self, _ctx: &mut Context) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_destroy(&mut self, _ctx: &mut Context) {}
        }

        // 验证 trait object 可用
        let agent: Box<dyn Agent> = Box::new(DummyAgent::new());
        let desc = agent.descriptor();
        assert_eq!(desc.name.as_str(), "dummy");
    }
}
