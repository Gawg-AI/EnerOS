//! Integration tests for v0.36.0 AgentSpawner.
//!
//! 验证 Agent 启动器的端到端行为：
//! - 完整成功路径（Created → Ready → Running）
//! - on_init / on_start / load_code 失败时进入 Error 状态
//! - spawn_blocking 与 spawn 等价
//! - 多 Agent 独立 spawn
//! - priority_override / mem_override 应用
//! - AgentContext 在回调中传递正确的 agent_id 与 config

use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;

use eneros_agent::{
    AgentConfig, AgentContext, AgentEntry, AgentError, AgentFactory, AgentId, AgentRegistry,
    AgentSpawner, AgentState, AgentType, LifecycleManager,
};

// ---- Helper agent implementations ----

/// Success Agent: on_init/on_start return Ok.
struct SuccessAgent;
impl AgentEntry for SuccessAgent {
    fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Ok(())
    }
    fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Ok(())
    }
    fn on_stop(&mut self, _ctx: &mut AgentContext) {}
}

/// FailInit Agent: on_init returns Err.
struct FailInitAgent;
impl AgentEntry for FailInitAgent {
    fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Err(AgentError::InitFailed(String::from("test init failure")))
    }
    fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Ok(())
    }
    fn on_stop(&mut self, _ctx: &mut AgentContext) {}
}

/// FailStart Agent: on_start returns Err.
struct FailStartAgent;
impl AgentEntry for FailStartAgent {
    fn on_init(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Ok(())
    }
    fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Err(AgentError::StartFailed(String::from("test start failure")))
    }
    fn on_stop(&mut self, _ctx: &mut AgentContext) {}
}

/// Recording Agent: captures ctx.agent_id and ctx.config.name during on_init.
struct RecordingAgent {
    captured: Rc<RefCell<Option<(AgentId, String)>>>,
}
impl AgentEntry for RecordingAgent {
    fn on_init(&mut self, ctx: &mut AgentContext) -> Result<(), AgentError> {
        *self.captured.borrow_mut() = Some((ctx.agent_id, ctx.config.name.clone()));
        Ok(())
    }
    fn on_start(&mut self, _ctx: &mut AgentContext) -> Result<(), AgentError> {
        Ok(())
    }
    fn on_stop(&mut self, _ctx: &mut AgentContext) {}
}

// ---- Helper factory implementations ----

/// Success factory: creates SuccessAgent.
struct SuccessFactory;
impl AgentFactory for SuccessFactory {
    fn create(
        &self,
        _agent_type: AgentType,
        _name: &str,
    ) -> Result<Box<dyn AgentEntry>, AgentError> {
        Ok(Box::new(SuccessAgent))
    }
}

/// Fail factory: load_code fails (CodeLoadFailed).
struct FailFactory;
impl AgentFactory for FailFactory {
    fn create(
        &self,
        _agent_type: AgentType,
        _name: &str,
    ) -> Result<Box<dyn AgentEntry>, AgentError> {
        Err(AgentError::CodeLoadFailed(String::from(
            "no agent registered",
        )))
    }
}

/// FailInit factory: creates FailInitAgent.
struct FailInitFactory;
impl AgentFactory for FailInitFactory {
    fn create(
        &self,
        _agent_type: AgentType,
        _name: &str,
    ) -> Result<Box<dyn AgentEntry>, AgentError> {
        Ok(Box::new(FailInitAgent))
    }
}

/// FailStart factory: creates FailStartAgent.
struct FailStartFactory;
impl AgentFactory for FailStartFactory {
    fn create(
        &self,
        _agent_type: AgentType,
        _name: &str,
    ) -> Result<Box<dyn AgentEntry>, AgentError> {
        Ok(Box::new(FailStartAgent))
    }
}

/// Recording factory: creates RecordingAgent sharing the captured cell.
struct RecordingFactory {
    captured: Rc<RefCell<Option<(AgentId, String)>>>,
}
impl AgentFactory for RecordingFactory {
    fn create(
        &self,
        _agent_type: AgentType,
        _name: &str,
    ) -> Result<Box<dyn AgentEntry>, AgentError> {
        Ok(Box::new(RecordingAgent {
            captured: self.captured.clone(),
        }))
    }
}

// ---- Helper functions ----

/// Create an AgentSpawner with shared registry and lifecycle.
fn make_spawner<F: AgentFactory + 'static>(
    factory: F,
) -> (
    AgentSpawner,
    Rc<RefCell<AgentRegistry>>,
    Rc<RefCell<LifecycleManager>>,
) {
    let reg = Rc::new(RefCell::new(AgentRegistry::new()));
    let lifecycle = Rc::new(RefCell::new(LifecycleManager::new(reg.clone())));
    let spawner = AgentSpawner::new(reg.clone(), lifecycle.clone(), Rc::new(factory));
    (spawner, reg, lifecycle)
}

/// Create a minimal AgentConfig.
fn make_config(agent_type: AgentType, name: &str) -> AgentConfig {
    AgentConfig {
        agent_type,
        name: String::from(name),
        binary_path: None,
        config_path: None,
        priority_override: None,
        mem_override: None,
    }
}

// ---- Integration tests ----

#[test]
fn integration_spawn_full_success_path() {
    let (spawner, _reg, lifecycle) = make_spawner(SuccessFactory);
    let config = make_config(AgentType::Energy, "e1");
    let id = spawner.spawn(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
}

#[test]
fn integration_spawn_init_failure_error_state() {
    let (spawner, reg, lifecycle) = make_spawner(FailInitFactory);
    let config = make_config(AgentType::Energy, "e1");
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert_eq!(
        err,
        AgentError::InitFailed(String::from("test init failure"))
    );
    assert_eq!(reg.borrow().count(), 1);
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
}

#[test]
fn integration_spawn_start_failure_error_state() {
    let (spawner, reg, lifecycle) = make_spawner(FailStartFactory);
    let config = make_config(AgentType::Energy, "e1");
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert_eq!(
        err,
        AgentError::StartFailed(String::from("test start failure"))
    );
    assert_eq!(reg.borrow().count(), 1);
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
}

#[test]
fn integration_spawn_load_code_failure() {
    let (spawner, reg, lifecycle) = make_spawner(FailFactory);
    let config = make_config(AgentType::Energy, "e1");
    let err = spawner.spawn(config, 1000).unwrap_err();
    assert_eq!(
        err,
        AgentError::CodeLoadFailed(String::from("no agent registered"))
    );
    assert_eq!(reg.borrow().count(), 1);
    let id = reg.borrow().list_all()[0].agent_id;
    assert_eq!(lifecycle.borrow().current_state(id), Ok(AgentState::Error));
}

#[test]
fn integration_spawn_blocking_same_as_spawn() {
    let (spawner, _reg, lifecycle) = make_spawner(SuccessFactory);
    let config = make_config(AgentType::Energy, "e1");
    let id = spawner.spawn_blocking(config, 1000).unwrap();
    assert_eq!(
        lifecycle.borrow().current_state(id),
        Ok(AgentState::Running)
    );
}

#[test]
fn integration_spawn_multiple_agents() {
    let (spawner, reg, lifecycle) = make_spawner(SuccessFactory);
    let id1 = spawner
        .spawn(make_config(AgentType::Energy, "e1"), 1000)
        .unwrap();
    let id2 = spawner
        .spawn(make_config(AgentType::Market, "m1"), 1000)
        .unwrap();
    let id3 = spawner
        .spawn(make_config(AgentType::Grid, "g1"), 1000)
        .unwrap();
    assert_eq!(reg.borrow().count(), 3);
    assert_eq!(
        lifecycle.borrow().current_state(id1),
        Ok(AgentState::Running)
    );
    assert_eq!(
        lifecycle.borrow().current_state(id2),
        Ok(AgentState::Running)
    );
    assert_eq!(
        lifecycle.borrow().current_state(id3),
        Ok(AgentState::Running)
    );
}

#[test]
fn integration_spawn_with_overrides() {
    let (spawner, reg, _lifecycle) = make_spawner(SuccessFactory);
    // Device default priority is 100, default mem_quota is 32 MB;
    // override to 200 and 1024 respectively to verify both are applied.
    let mut config = make_config(AgentType::Device, "d1");
    config.priority_override = Some(200);
    config.mem_override = Some(1024);
    let id = spawner.spawn(config, 1000).unwrap();
    assert_eq!(reg.borrow().get(id).unwrap().priority, 200);
    assert_eq!(reg.borrow().get(id).unwrap().mem_quota, 1024);
}

#[test]
fn integration_spawn_agent_context_correct() {
    let captured = Rc::new(RefCell::new(None));
    let (spawner, _reg, _lifecycle) = make_spawner(RecordingFactory {
        captured: captured.clone(),
    });
    let config = make_config(AgentType::Energy, "rec-agent");
    let id = spawner.spawn(config, 1000).unwrap();

    let cap = captured.borrow();
    let (ctx_id, ctx_name) = cap.as_ref().expect("context not captured");
    assert_eq!(*ctx_id, id);
    assert_eq!(ctx_name, "rec-agent");
}
