//! 自定义负荷均衡策略 Agent 示例插件
//!
//! 本 crate 编译为 cdylib 动态库，由 EnerOS 插件加载器通过 C ABI 加载。
//! 元数据通过同目录 `manifest.toml` 提供，故不导出 `eneros_plugin_metadata`。
//!
//! 策略说明（基于规则的负荷均衡，stub 实现）：
//! - 每个 Agent 实例对应一个负荷区域，跟踪区域总负荷与阈值
//! - tick 时检查当前负荷，超过阈值则发布告警事件并记录日志
//! - handle_event 接收遥测更新，刷新内部负荷估计
//!
//! 注意：本示例仅演示插件接口契约，不执行真实控制动作。

use async_trait::async_trait;
use eneros_core::AuthorityLevel;
use eneros_plugin::agent::{
    AgentPlugin, AgentPluginAction, AgentPluginConfig, AgentPluginEvent, AgentStrategyInstance,
    StrategyPriority,
};
use std::ffi::c_void;

/// 自定义负荷均衡策略插件
pub struct CustomStrategyPlugin;

#[async_trait]
impl AgentPlugin for CustomStrategyPlugin {
    fn strategy_name(&self) -> &str {
        "custom-load-balance"
    }

    fn description(&self) -> &str {
        "Custom load balancing strategy Agent (stub)"
    }

    fn authority_level(&self) -> AuthorityLevel {
        // 插件权限上限为 Operator，这里声明 Operator 即可
        AuthorityLevel::Operator
    }

    fn priority(&self) -> StrategyPriority {
        StrategyPriority::Normal
    }

    async fn create_agent(
        &self,
        config: &AgentPluginConfig,
    ) -> Result<Box<dyn AgentStrategyInstance>, eneros_plugin::PluginError> {
        // 从 custom_config 解析负荷阈值（默认 0.85）
        let threshold = config
            .custom_config
            .get("threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.85);

        Ok(Box::new(CustomStrategyAgent {
            agent_id: config.agent_id.clone(),
            agent_type: config.agent_type.clone(),
            tick_interval_ms: config.tick_interval_ms,
            current_load: 0.0,
            threshold,
            tick_count: 0u64,
        }))
    }
}

/// 自定义负荷均衡 Agent 实例（stub）
pub struct CustomStrategyAgent {
    agent_id: String,
    agent_type: String,
    tick_interval_ms: u64,
    /// 当前归一化负荷（0.0 ~ 1.0）
    current_load: f64,
    /// 负荷告警阈值
    threshold: f64,
    /// tick 计数（用于日志）
    tick_count: u64,
}

#[async_trait]
impl AgentStrategyInstance for CustomStrategyAgent {
    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn agent_type(&self) -> &str {
        &self.agent_type
    }

    async fn handle_event(
        &mut self,
        event: &AgentPluginEvent,
    ) -> Result<Vec<AgentPluginAction>, eneros_plugin::PluginError> {
        let mut actions = Vec::new();

        // 处理遥测更新事件：刷新当前负荷估计
        if event.event_type == "telemetry_update" {
            if let Some(load) = event.payload.get("load").and_then(|v| v.as_f64()) {
                self.current_load = load;
                actions.push(AgentPluginAction::LogMessage {
                    level: "info".to_string(),
                    message: format!(
                        "agent {} load updated to {:.3}",
                        self.agent_id, self.current_load
                    ),
                });

                // 超阈值时发布告警事件
                if self.current_load > self.threshold {
                    actions.push(AgentPluginAction::PublishEvent {
                        event_type: "load_alarm".to_string(),
                        payload: serde_json::json!({
                            "agent_id": self.agent_id,
                            "load": self.current_load,
                            "threshold": self.threshold,
                        }),
                    });
                }
            }
        }

        if actions.is_empty() {
            actions.push(AgentPluginAction::NoOp);
        }
        Ok(actions)
    }

    async fn tick(&mut self) -> Result<Vec<AgentPluginAction>, eneros_plugin::PluginError> {
        self.tick_count += 1;
        // stub：tick 时仅记录日志，真实策略会在此执行负荷均衡决策
        let level = if self.current_load > self.threshold {
            "warn"
        } else {
            "info"
        };
        Ok(vec![AgentPluginAction::LogMessage {
            level: level.to_string(),
            message: format!(
                "agent {} tick #{} load={:.3} threshold={:.3}",
                self.agent_id, self.tick_count, self.current_load, self.threshold
            ),
        }])
    }

    fn tick_interval_ms(&self) -> u64 {
        self.tick_interval_ms
    }
}

impl CustomStrategyAgent {
    /// 获取当前负荷（用于调试/测试）
    pub fn current_load(&self) -> f64 {
        self.current_load
    }

    /// 获取阈值（用于调试/测试）
    pub fn threshold(&self) -> f64 {
        self.threshold
    }
}

/// C ABI 入口：创建插件实例
///
/// 返回堆分配的 `Box<CustomStrategyPlugin>` 裸指针，调用方负责通过
/// `eneros_plugin_destroy` 释放。
///
/// 注意：通过 C ABI 传递的是瘦指针（具体类型），加载器在需要时
/// 可将其包装为 `dyn AgentPlugin` trait object。
///
/// # Safety
///
/// 调用方必须保证返回的指针仅通过 `eneros_plugin_destroy` 释放一次，
/// 且在销毁前不得解引用为其他类型。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_create() -> *mut c_void {
    let plugin: Box<CustomStrategyPlugin> = Box::new(CustomStrategyPlugin);
    Box::into_raw(plugin) as *mut c_void
}

/// C ABI 入口：销毁插件实例
///
/// 接收 `eneros_plugin_create` 返回的指针并释放其内存。
/// 传入空指针时为空操作。
///
/// # Safety
///
/// `ptr` 必须为 `eneros_plugin_create` 的返回值或空指针，
/// 且同一指针不得销毁超过一次。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_destroy(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: ptr 由 eneros_plugin_create 通过 Box::into_raw 产生，
        // 调用方保证仅释放一次。
        let _ = Box::from_raw(ptr as *mut CustomStrategyPlugin);
    }
}
