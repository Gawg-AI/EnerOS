//! # EnerOS Agent SDK v1.0
//!
//! 外部开发者基于此 SDK 开发 EnerOS Agent。
//!
//! ## 快速开始
//!
//! ```ignore
//! use eneros_agent_sdk::{register_agent, Agent, AgentDescriptor, AgentError, Context};
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
//!
//! register_agent!(HelloAgent, "hello-agent");
//! ```
//!
//! ## 模块结构
//!
//! - [`agent`] — Agent trait、AgentDescriptor、AgentError
//! - [`context`] — Context（运行时环境）
//! - [`lifecycle`] — 生命周期状态机
//! - [`ipc`] — IPC 通信
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

extern crate alloc;

pub mod agent;
pub mod config;
pub mod context;
pub mod ipc;
pub mod lifecycle;
pub mod logger;
pub mod storage;

// 重导出核心类型
pub use agent::{Agent, AgentDescriptor, AgentError, AgentId, SDK_VERSION};
pub use config::Config;
pub use context::Context;
pub use ipc::IpcClient;
pub use lifecycle::{LifecycleManager, LifecycleState};
pub use logger::{LogLevel, Logger};
pub use storage::Storage;

/// Agent 注册宏（SDK v1.0 稳定 API）。
///
/// 导出 C ABI 入口函数，供 Agent Runtime 动态加载。
///
/// # 导出函数
/// - `eneros_agent_create` — 创建 Agent 实例
/// - `eneros_agent_descriptor` — 获取 Agent 描述符
/// - `eneros_agent_destroy` — 销毁 Agent 实例
///
/// # 示例
/// ```ignore
/// use eneros_agent_sdk::register_agent;
///
/// struct MyAgent;
/// impl MyAgent { fn new() -> Self { Self } }
/// // ... impl Agent for MyAgent ...
///
/// register_agent!(MyAgent, "my-agent");
/// ```
#[macro_export]
macro_rules! register_agent {
    ($type:ty, $name:literal) => {
        /// 创建 Agent 实例（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_agent_create() -> *mut dyn $crate::Agent {
            let agent = <$type>::new();
            Box::into_raw(Box::new(agent))
        }

        /// 获取 Agent 描述符（C ABI）。
        #[no_mangle]
        pub extern "C" fn eneros_agent_descriptor() -> $crate::AgentDescriptor {
            let agent = <$type>::new();
            agent.descriptor()
        }

        /// 销毁 Agent 实例（C ABI）。
        ///
        /// # Safety
        /// 调用者必须确保 `ptr` 是由 `eneros_agent_create` 返回的有效指针。
        #[no_mangle]
        pub unsafe extern "C" fn eneros_agent_destroy(ptr: *mut dyn $crate::Agent) {
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }

        /// Agent 名称常量。
        pub const AGENT_NAME: &str = $name;
    };
}

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
