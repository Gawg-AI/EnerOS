//! Agent 启动配置与入口 trait — AgentConfig / AgentContext / AgentEntry
//!
//! # 设计
//! - `AgentConfig` 描述 Agent 启动时的静态配置（类型、名称、二进制路径、配额覆盖）
//! - `AgentContext` 是传递给 Agent 入口回调的运行时上下文（含共享注册表引用）
//! - `AgentEntry` 是 Agent 实现 object-safe 入口 trait（on_init / on_start / on_stop）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;

use crate::{AgentError, AgentId, AgentRegistry, AgentType};

/// Agent 启动配置（6 字段）.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub agent_type: AgentType,
    pub name: String,
    pub binary_path: Option<String>,
    pub config_path: Option<String>,
    pub priority_override: Option<u8>,
    pub mem_override: Option<usize>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            agent_type: AgentType::System,
            name: String::from("default"),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        }
    }
}

/// Agent 运行时上下文.
///
/// 传递给 `AgentEntry` 的回调方法，提供 Agent ID、配置副本与共享注册表引用。
/// 不实现 Clone（按 `&mut` 传递，避免注册表引用被多次克隆）。
#[derive(Debug)]
pub struct AgentContext {
    pub agent_id: AgentId,
    pub config: AgentConfig,
    pub registry: Rc<RefCell<AgentRegistry>>,
}

/// Agent 入口 trait（object-safe）.
///
/// 由具体 Agent 实现，`AgentSpawner` 在 spawn 流程中调用：
/// - `on_init` 在 `Created→Ready` 之后、`Ready→Running` 之前调用（初始化资源）
/// - `on_start` 在 `Ready→Running` 之后调用（启动主循环）
/// - `on_stop` 预留给 v0.38.0 崩溃恢复使用（v0.36.0 不调用）
pub trait AgentEntry {
    /// 初始化回调（加载配置、分配资源等）.
    fn on_init(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>;
    /// 启动回调（启动主循环）.
    fn on_start(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError>;
    /// 停止回调（释放资源，v0.38.0 使用）.
    fn on_stop(&mut self, ctx: &mut AgentContext);
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use alloc::string::String;
    use core::cell::RefCell;

    use super::*;
    use crate::{AgentId, AgentRegistry};

    #[test]
    fn test_agent_config_construction() {
        let config = AgentConfig {
            agent_type: AgentType::Energy,
            name: String::from("e1"),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        };
        assert_eq!(config.agent_type, AgentType::Energy);
        assert_eq!(config.name, "e1");
        assert_eq!(config.binary_path, None);
        assert_eq!(config.config_path, None);
        assert_eq!(config.priority_override, None);
        assert_eq!(config.mem_override, None);
    }

    #[test]
    fn test_agent_config_clone_eq() {
        let config = AgentConfig {
            agent_type: AgentType::Energy,
            name: String::from("e1"),
            binary_path: None,
            config_path: None,
            priority_override: None,
            mem_override: None,
        };
        let mut clone = config.clone();
        assert_eq!(config, clone);
        clone.priority_override = Some(100);
        assert_ne!(config, clone);
    }

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.agent_type, AgentType::System);
        assert_eq!(config.name, "default");
        assert_eq!(config.binary_path, None);
        assert_eq!(config.config_path, None);
        assert_eq!(config.priority_override, None);
        assert_eq!(config.mem_override, None);
    }

    #[test]
    fn test_agent_config_with_overrides() {
        let config = AgentConfig {
            agent_type: AgentType::Energy,
            name: String::from("e1"),
            binary_path: None,
            config_path: None,
            priority_override: Some(200),
            mem_override: Some(1024),
        };
        assert_eq!(config.priority_override, Some(200));
        assert_eq!(config.mem_override, Some(1024));
    }

    #[test]
    fn test_agent_context_construction() {
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let ctx = AgentContext {
            agent_id: AgentId::generate(),
            config: AgentConfig::default(),
            registry: reg.clone(),
        };
        assert_ne!(ctx.agent_id, AgentId::ZERO);
        assert_eq!(ctx.config.name, "default");
        assert_eq!(reg.borrow().count(), 0);
    }

    #[test]
    fn test_agent_entry_object_safe() {
        struct TestAgent;
        impl AgentEntry for TestAgent {
            fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
                Ok(())
            }
            fn on_stop(&mut self, _ctx: &mut AgentContext) {}
        }

        let mut a: Box<dyn AgentEntry> = Box::new(TestAgent);
        let reg = Rc::new(RefCell::new(AgentRegistry::new()));
        let mut ctx = AgentContext {
            agent_id: AgentId::generate(),
            config: AgentConfig::default(),
            registry: reg,
        };
        assert!(a.on_init(&mut ctx).is_ok());
        assert!(a.on_start(&mut ctx).is_ok());
        a.on_stop(&mut ctx);
    }
}
