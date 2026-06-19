//! EnerOS PID 1 init system
//!
//! This is the first process (PID 1) in the EnerOS Power-Native OS.
//! It is responsible for:
//! - Starting system services in dependency order
//! - Starting Agent processes after system services are up
//! - Monitoring service/agent health and restarting failed processes
//! - Handling signals (SIGTERM, SIGINT, SIGHUP)
//! - Graceful shutdown
//! - Reaping zombie/orphan processes (PID 1 duty)

use eneros_os::agentos::{
    AgentRegistry, AgentScheduler, AgentSupervisor, AgentSpawnConfig, AuthorityEnforcer,
    ResourceQuota,
};
use eneros_os::init::{
    AgentServiceConfig, InitConfig, ServiceGraph, ServiceManager, SignalHandler,
};
use eneros_os::rt::{HardwareWatchdog, WatchdogError};
#[cfg(target_os = "linux")]
use eneros_os::update::{AbPartition, Slot};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

/// Default config file path.
const DEFAULT_CONFIG_PATH: &str = "/etc/eneros/init.toml";
/// Main-loop polling interval.
const LOOP_INTERVAL: Duration = Duration::from_millis(100);
/// Graceful shutdown timeout before escalating to SIGKILL.
const SHUTDOWN_TIMEOUT_SECS: u64 = 10;
/// 槽位状态文件路径（boot success detection）
#[cfg(target_os = "linux")]
const SLOT_STATE_PATH: &str = "/etc/eneros/slot-state.json";
/// 启动成功检测等待时间（秒）—— 服务启动后等待此时间确认稳定
#[cfg(target_os = "linux")]
const BOOT_SUCCESS_TIMEOUT_SECS: u64 = 60;
/// 最大启动尝试次数（超过则标记失败并触发回滚）
#[cfg(target_os = "linux")]
const MAX_BOOT_COUNT: u32 = 3;

fn main() -> ExitCode {
    // 1. Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let pid = std::process::id();
    let is_pid1 = is_pid1();
    tracing::info!("EnerOS init starting (pid={}, is_pid1={})", pid, is_pid1);

    // 1b. Boot success detection: mark current slot as Trying (early, before services)
    #[cfg(target_os = "linux")]
    let mut boot_slot_info = mark_boot_trying();

    // 2. Load configuration (config file or built-in default)
    let config_path = std::env::var("ENEROS_INIT_CONFIG")
        .unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    tracing::info!("Loading init config from: {}", config_path);
    let init_config = InitConfig::load_from_file_or_default(&config_path);

    // 3. Build service dependency graph
    let mut graph = ServiceGraph::new();
    for svc in &init_config.services {
        tracing::debug!(
            "Registered service '{}' (binary={}, deps={:?})",
            svc.name,
            svc.binary,
            svc.dependencies
        );
        graph.add_service(svc.clone());
    }

    // Validate the dependency graph early.
    if let Err(e) = graph.topological_sort() {
        tracing::error!("Invalid service dependency graph: {}", e);
        return ExitCode::FAILURE;
    }

    // 4. Install signal handlers
    let signals = SignalHandler::new();
    if let Err(e) = signals.install() {
        tracing::error!("Failed to install signal handlers: {}", e);
        return ExitCode::FAILURE;
    }
    tracing::info!("Signal handlers installed (SIGTERM/SIGINT/SIGHUP)");

    // 4b. Open the hardware watchdog (500ms timeout → hardware reset on stall).
    // Non-fatal: development environments may lack /dev/watchdog.
    let mut watchdog = HardwareWatchdog::open(std::path::Path::new("/dev/watchdog"), 500);
    match &watchdog {
        Ok(wd) => tracing::info!("Hardware watchdog opened (timeout={}ms)", wd.timeout_ms()),
        Err(e) => tracing::warn!(
            "Failed to open hardware watchdog: {} (non-fatal, continuing without watchdog)",
            e
        ),
    }

    // 5. Create service manager and prepare startup order
    let mut manager = ServiceManager::new(graph);
    match manager.prepare() {
        Ok(order) => {
            tracing::info!("Service startup order: {:?}", order);
        }
        Err(e) => {
            tracing::error!("Failed to compute startup order: {}", e);
            return ExitCode::FAILURE;
        }
    }

    // 6. Start all system services
    tracing::info!("Starting all system services...");
    if let Err(e) = manager.start_all() {
        tracing::error!("Failed during start_all: {}", e);
        // Don't exit — PID 1 must keep running. Individual service
        // failures are already logged inside start_all.
    }
    tracing::info!(
        "Initial startup complete: {} services running, {} degraded",
        manager.running_count(),
        manager.degraded_count()
    );

    // 7. Build AgentOS kernel components (shared registry + supervisor + scheduler + enforcer + quota)
    let registry = Arc::new(AgentRegistry::new());
    let supervisor = Arc::new(AgentSupervisor::new(Arc::clone(&registry)));
    let scheduler = Arc::new(AgentScheduler::new(Arc::clone(&registry)));
    let enforcer = Arc::new(AuthorityEnforcer::new(Arc::clone(&registry)));
    let quota = Arc::new(ResourceQuota::new(Arc::clone(&registry)));

    // 8. Spawn all configured Agent processes
    let agent_configs = init_config.agents.clone();
    let started_agents = spawn_all_agents(&agent_configs, &supervisor, &scheduler, &enforcer, &quota);
    tracing::info!(
        "Agent startup complete: {}/{} agents running",
        started_agents,
        agent_configs.len()
    );

    // 8b. Boot success detection: wait for services to stabilize, then mark Good/Failed
    #[cfg(target_os = "linux")]
    if let Some((ref mut ab, slot)) = boot_slot_info {
        check_boot_success(ab, &mut manager, slot, &mut watchdog);
    }

    // 9. Main loop
    let result = run_main_loop(
        &signals,
        &mut manager,
        &supervisor,
        &agent_configs,
        is_pid1,
        &mut watchdog,
    );

    // 10. Shutdown: stop all agents first, then stop services
    tracing::info!("Stopping all Agent processes...");
    stop_all_agents(&agent_configs, &supervisor);

    // Disable the hardware watchdog before service shutdown, which may take up
    // to SHUTDOWN_TIMEOUT_SECS — longer than the 500ms watchdog window.
    if let Ok(wd) = watchdog {
        if let Err(e) = wd.disable() {
            tracing::warn!("Failed to disable hardware watchdog: {}", e);
        }
    }

    tracing::info!("Initiating graceful shutdown (timeout={}s)...", SHUTDOWN_TIMEOUT_SECS);
    if let Err(e) = manager.stop_all(SHUTDOWN_TIMEOUT_SECS) {
        tracing::error!("Error during shutdown: {}", e);
    }
    tracing::info!(
        "Shutdown complete: {} services still running",
        manager.running_count()
    );

    match result {
        LoopResult::Shutdown => {
            tracing::info!("EnerOS init exiting (shutdown)");
            ExitCode::SUCCESS
        }
        LoopResult::NoServicesAndNotPid1 => {
            tracing::info!("EnerOS init exiting (no services, not PID 1)");
            ExitCode::SUCCESS
        }
    }
}

/// Result of the main loop.
enum LoopResult {
    /// A shutdown signal was received.
    Shutdown,
    /// All services have exited and we are not PID 1 (test/dev mode).
    NoServicesAndNotPid1,
}

/// Spawn all configured Agent processes via the AgentSupervisor, then apply
/// scheduling policy, capabilities, and resource quotas.
///
/// Returns the number of successfully started agents.
fn spawn_all_agents(
    agent_configs: &[AgentServiceConfig],
    supervisor: &Arc<AgentSupervisor>,
    scheduler: &Arc<AgentScheduler>,
    enforcer: &Arc<AuthorityEnforcer>,
    quota: &Arc<ResourceQuota>,
) -> usize {
    let mut started = 0usize;
    for cfg in agent_configs {
        let spawn_cfg = AgentSpawnConfig {
            agent_id: cfg.agent_id.clone(),
            agent_type: cfg.agent_type.clone(),
            authority: cfg.authority,
            binary: cfg.binary.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
        };

        match supervisor.spawn(spawn_cfg) {
            Ok(pid) => {
                tracing::info!(
                    "Agent '{}' spawned (pid={}, type={:?}, authority={:?})",
                    cfg.agent_id,
                    pid,
                    cfg.agent_type,
                    cfg.authority
                );

                // Apply scheduling policy (RT for self-healing, Normal for others)
                if let Err(e) = scheduler.schedule(&cfg.agent_id, cfg.scheduling_policy.clone()) {
                    tracing::warn!(
                        "Failed to schedule agent '{}': {} (non-fatal, continues with default policy)",
                        cfg.agent_id,
                        e
                    );
                }

                // Grant capabilities based on AuthorityLevel
                if let Err(e) = enforcer.auto_grant(&cfg.agent_id) {
                    tracing::warn!(
                        "Failed to grant capabilities to agent '{}': {} (non-fatal)",
                        cfg.agent_id,
                        e
                    );
                }

                // Set resource quota (cgroups v2)
                if cfg.resource_quota.has_limits() {
                    if let Err(e) = quota.set_quota(&cfg.agent_id, cfg.resource_quota.clone()) {
                        tracing::warn!(
                            "Failed to set resource quota for agent '{}': {} (non-fatal)",
                            cfg.agent_id,
                            e
                        );
                    }
                }

                started += 1;
            }
            Err(e) => {
                tracing::error!(
                    "Failed to spawn agent '{}' (binary={}): {}",
                    cfg.agent_id,
                    cfg.binary,
                    e
                );
            }
        }
    }
    started
}

/// Stop all running Agent processes gracefully.
fn stop_all_agents(agent_configs: &[AgentServiceConfig], supervisor: &Arc<AgentSupervisor>) {
    for cfg in agent_configs {
        // Only stop agents that are registered (Running or Degraded)
        if let Ok(info) = supervisor.health_check(&cfg.agent_id) {
            if info == eneros_os::agentos::AgentStatus::Running
                || info == eneros_os::agentos::AgentStatus::Degraded
            {
                tracing::info!("Stopping agent '{}'", cfg.agent_id);
                if let Err(e) = supervisor.stop(&cfg.agent_id) {
                    tracing::error!("Failed to stop agent '{}': {}", cfg.agent_id, e);
                }
            }
        }
    }
}

/// Check agent health and restart crashed agents.
///
/// Called each main-loop iteration. Respects the supervisor's crash-rate
/// limiter (5 restarts/minute → Degraded).
fn restart_crashed_agents(
    agent_configs: &[AgentServiceConfig],
    supervisor: &Arc<AgentSupervisor>,
) {
    for cfg in agent_configs {
        match supervisor.health_check(&cfg.agent_id) {
            Ok(eneros_os::agentos::AgentStatus::Crashed) => {
                tracing::warn!("Agent '{}' has crashed, attempting restart", cfg.agent_id);
                match supervisor.should_restart(&cfg.agent_id) {
                    Ok(true) => {
                        let spawn_cfg = AgentSpawnConfig {
                            agent_id: cfg.agent_id.clone(),
                            agent_type: cfg.agent_type.clone(),
                            authority: cfg.authority,
                            binary: cfg.binary.clone(),
                            args: cfg.args.clone(),
                            env: cfg.env.clone(),
                        };
                        match supervisor.restart(&spawn_cfg) {
                            Ok(new_pid) => {
                                tracing::info!(
                                    "Agent '{}' restarted (new pid={})",
                                    cfg.agent_id,
                                    new_pid
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to restart agent '{}': {}",
                                    cfg.agent_id,
                                    e
                                );
                            }
                        }
                    }
                    Ok(false) => {
                        tracing::error!(
                            "Agent '{}' in degraded mode (too many crashes), not restarting",
                            cfg.agent_id
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to check restart eligibility for agent '{}': {}",
                            cfg.agent_id,
                            e
                        );
                    }
                }
            }
            Ok(_) => { /* Running/Stopped/Starting — no action */ }
            Err(e) => {
                tracing::debug!(
                    "Health check failed for agent '{}': {} (may not be registered yet)",
                    cfg.agent_id,
                    e
                );
            }
        }
    }
}

/// The init main loop: monitors signals, reaps children, restarts services and agents.
fn run_main_loop(
    signals: &SignalHandler,
    manager: &mut ServiceManager,
    supervisor: &Arc<AgentSupervisor>,
    agent_configs: &[AgentServiceConfig],
    is_pid1: bool,
    watchdog: &mut Result<HardwareWatchdog, WatchdogError>,
) -> LoopResult {
    loop {
        // Check for shutdown signal (SIGTERM / SIGINT)
        if signals.should_shutdown() {
            tracing::info!("Shutdown signal received");
            return LoopResult::Shutdown;
        }

        // Check for reload signal (SIGHUP)
        if signals.should_reload() {
            tracing::info!("Reload signal (SIGHUP) received — reloading configuration");
            handle_reload(manager);
            signals.clear_reload();
        }

        // Reap exited children and detect failures
        match manager.reap_children() {
            Ok(exited) => {
                for name in &exited {
                    tracing::info!("Service '{}' has exited", name);
                }
            }
            Err(e) => {
                tracing::error!("Error reaping children: {}", e);
            }
        }

        // Restart eligible services (respects policy, crash-frequency, delay)
        let pending = manager.restart_pending();
        for name in pending {
            tracing::info!("Attempting restart of service '{}'", name);
            if let Err(e) = manager.restart_service(&name) {
                tracing::error!("Failed to restart service '{}': {}", name, e);
            }
        }

        // Check agent health and restart crashed agents
        restart_crashed_agents(agent_configs, supervisor);

        // PID 1 must never exit unless explicitly shutting down.
        // Non-PID-1 runs (tests/dev) can exit once all services are gone
        // and nothing is pending restart.
        if !is_pid1 && !manager.has_running() {
            // Double-check: nothing pending after a final reap.
            match manager.reap_children() {
                Ok(_) => {}
                Err(e) => tracing::error!("Error during final reap: {}", e),
            }
            if !manager.has_running() && manager.restart_pending().is_empty() {
                return LoopResult::NoServicesAndNotPid1;
            }
        }

        // Feed the hardware watchdog. A failure is non-fatal (warn + continue):
        // development environments may not have a real /dev/watchdog.
        if let Ok(ref mut wd) = watchdog {
            if let Err(e) = wd.keepalive() {
                tracing::warn!("Watchdog keepalive failed: {}", e);
            }
        }

        // Brief sleep to avoid busy-looping
        std::thread::sleep(LOOP_INTERVAL);
    }
}

/// Handle SIGHUP: reload configuration and update the manager's graph.
///
/// Running services are not restarted; the new configuration takes effect
/// for any service that exits and is restarted by the supervisor.
fn handle_reload(manager: &mut ServiceManager) {
    let config_path = std::env::var("ENEROS_INIT_CONFIG")
        .unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let new_config = InitConfig::load_from_file_or_default(&config_path);

    let mut new_graph = ServiceGraph::new();
    for svc in &new_config.services {
        new_graph.add_service(svc.clone());
    }

    match new_graph.topological_sort() {
        Ok(order) => {
            tracing::info!(
                "Configuration reloaded: {} services, order={:?}",
                new_config.services.len(),
                order
            );
            // Replace the graph. The manager keeps running processes;
            // new config applies on next restart of each service.
            let old_graph = std::mem::replace(manager.graph_mut(), new_graph);
            // Re-register configs with the supervisor for services that
            // are not currently running.
            for svc in &new_config.services {
                if !manager.is_running(&svc.name) {
                    manager.supervisor_mut().register(svc.clone());
                }
            }
            // Refresh the cached startup order from the new graph.
            if let Err(e) = manager.refresh_startup_order() {
                tracing::error!("Failed to refresh startup order after reload: {}", e);
            }
            // Preserve old graph data for running services by not touching
            // the processes map.
            let _ = old_graph;
        }
        Err(e) => {
            tracing::error!("Reload failed — invalid dependency graph: {}", e);
        }
    }
}

/// Returns `true` if this process is PID 1.
fn is_pid1() -> bool {
    std::process::id() == 1
}

// ---------------------------------------------------------------------------
// Boot success detection (Linux only)
// ---------------------------------------------------------------------------

/// 启动成功检测：标记当前槽位为 Trying，返回 (AbPartition, Slot) 用于后续检测。
///
/// 流程：
/// 1. 读取 `ENEROS_BOOT_SLOT` 环境变量确定当前槽位
/// 2. 加载 AbPartition（从 /etc/eneros/slot-state.json）
/// 3. 调用 mark_trying(current_slot) — 标记为 Trying + boot_count +1
/// 4. 若 boot_count > MAX_BOOT_COUNT，标记失败并触发回滚，返回 None
///
/// 返回 None 的情况：
/// - ENEROS_BOOT_SLOT 未设置或无效
/// - 槽位状态文件加载失败
/// - boot_count 超过上限（已标记失败并回滚）
#[cfg(target_os = "linux")]
fn mark_boot_trying() -> Option<(AbPartition, Slot)> {
    let slot_name = std::env::var("ENEROS_BOOT_SLOT").ok()?;
    let current_slot = match slot_name.as_str() {
        "A" | "a" => Slot::A,
        "B" | "b" => Slot::B,
        _ => {
            tracing::warn!(
                "无效的 ENEROS_BOOT_SLOT='{}'，跳过启动成功检测",
                slot_name
            );
            return None;
        }
    };

    let path = std::path::Path::new(SLOT_STATE_PATH);
    let mut ab = match AbPartition::load_from_file(path) {
        Ok(ab) => ab,
        Err(e) => {
            tracing::warn!("加载槽位状态失败: {}，跳过启动成功检测", e);
            return None;
        }
    };

    ab.mark_trying(current_slot);

    let boot_count = match current_slot {
        Slot::A => ab.boot_count_a,
        Slot::B => ab.boot_count_b,
    };

    if boot_count > MAX_BOOT_COUNT {
        tracing::error!(
            "槽位 {:?} 启动次数 {} 超过上限 {}，标记失败并回滚",
            current_slot,
            boot_count,
            MAX_BOOT_COUNT
        );
        ab.mark_failed(current_slot);
        trigger_rollback(&mut ab, current_slot);
        return None;
    }

    tracing::info!(
        "槽位 {:?} 标记为 Trying（启动次数 {}）",
        current_slot,
        boot_count
    );
    Some((ab, current_slot))
}

/// 检测启动是否成功：等待服务稳定后标记槽位为 Good 或 Failed。
///
/// 流程：
/// 1. 等待 BOOT_SUCCESS_TIMEOUT_SECS 秒（期间持续喂硬件看门狗）
/// 2. 调用 reap_children 处理已退出的服务
/// 3. 检查是否有降级或失败的服务
/// 4. 全部正常 → mark_good；存在异常 → mark_failed + trigger_rollback
#[cfg(target_os = "linux")]
fn check_boot_success(
    ab: &mut AbPartition,
    manager: &mut ServiceManager,
    current_slot: Slot,
    watchdog: &mut Result<HardwareWatchdog, WatchdogError>,
) {
    tracing::info!(
        "等待 {} 秒以确认服务稳定...",
        BOOT_SUCCESS_TIMEOUT_SECS
    );

    // 等待期间持续喂硬件看门狗，避免超时复位
    let deadline =
        std::time::Instant::now() + Duration::from_secs(BOOT_SUCCESS_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        if let Ok(ref mut wd) = watchdog {
            if let Err(e) = wd.keepalive() {
                tracing::warn!("启动检测期间看门狗喂狗失败: {}", e);
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }

    // 处理已退出的服务，更新状态
    if let Err(e) = manager.reap_children() {
        tracing::warn!("reap_children 失败: {}", e);
    }

    let degraded = manager.degraded_count();
    let any_failed = manager
        .supervisor()
        .services()
        .any(|s| s.status == eneros_os::init::ServiceStatus::Failed);

    if degraded == 0 && !any_failed {
        ab.mark_good(current_slot);
        tracing::info!(
            "启动成功: 槽位 {:?} 标记为 Good（{} 个服务运行中）",
            current_slot,
            manager.running_count()
        );
    } else {
        ab.mark_failed(current_slot);
        tracing::error!(
            "启动失败: 槽位 {:?} 标记为 Failed（{} 个降级，存在失败服务: {}）",
            current_slot,
            degraded,
            any_failed
        );
        trigger_rollback(ab, current_slot);
    }
}

/// 触发回滚：切换到最近的 Good 槽位。
///
/// 若当前已在 Good 槽位则无需操作；否则切换槽位（持久化由 AbPartition 自动完成）。
#[cfg(target_os = "linux")]
fn trigger_rollback(ab: &mut AbPartition, failed_slot: Slot) {
    match ab.last_good_slot() {
        Some(good) if good != failed_slot => {
            tracing::info!("回滚到槽位 {:?}", good);
            ab.switch_slot();
        }
        Some(good) => {
            tracing::info!("已在 Good 槽位 {:?}，无需回滚", good);
        }
        None => {
            tracing::error!("没有可用的 Good 槽位用于回滚");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_os::init::{RestartPolicy, ServiceConfig};

    #[test]
    fn test_is_pid1_returns_bool() {
        // In test environment we are almost certainly not PID 1.
        let _ = is_pid1();
    }

    #[test]
    fn test_loop_result_enum() {
        // Just ensure the enum variants exist and can be matched.
        let r = LoopResult::Shutdown;
        match r {
            LoopResult::Shutdown => {}
            LoopResult::NoServicesAndNotPid1 => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_main_loop_exits_on_shutdown_signal() {
        let signals = SignalHandler::new();
        signals.request_shutdown();

        let mut graph = ServiceGraph::new();
        graph.add_service(ServiceConfig {
            name: "test".to_string(),
            binary: "/bin/true".to_string(),
            restart_policy: RestartPolicy::No,
            ..Default::default()
        });

        let mut manager = ServiceManager::new(graph);
        manager.prepare().unwrap();

        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(registry));

        let result = run_main_loop(
            &signals,
            &mut manager,
            &supervisor,
            &[],
            false,
            &mut Err(WatchdogError::OpenFailed(std::io::Error::from_raw_os_error(2))),
        );
        assert!(matches!(result, LoopResult::Shutdown));
    }

    #[test]
    fn test_main_loop_exits_when_no_services_and_not_pid1() {
        let signals = SignalHandler::new();
        let mut graph = ServiceGraph::new();
        // Use a binary that exits immediately with success.
        graph.add_service(ServiceConfig {
            name: "quick".to_string(),
            binary: "/bin/true".to_string(),
            restart_policy: RestartPolicy::No,
            ..Default::default()
        });

        let mut manager = ServiceManager::new(graph);
        manager.prepare().unwrap();
        // Start the quick service — it will exit almost immediately.
        let _ = manager.start_all();

        // Give it a moment to exit.
        std::thread::sleep(Duration::from_millis(200));

        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(registry));

        let result = run_main_loop(
            &signals,
            &mut manager,
            &supervisor,
            &[],
            false,
            &mut Err(WatchdogError::OpenFailed(std::io::Error::from_raw_os_error(2))),
        );
        // Should exit because no services running and not PID 1.
        assert!(matches!(result, LoopResult::NoServicesAndNotPid1));
    }

    #[test]
    fn test_handle_reload_does_not_crash() {
        let mut graph = ServiceGraph::new();
        graph.add_service(ServiceConfig {
            name: "test".to_string(),
            binary: "/bin/true".to_string(),
            ..Default::default()
        });
        let mut manager = ServiceManager::new(graph);
        manager.prepare().unwrap();
        // handle_reload reads from a nonexistent config path (env not set
        // in tests → DEFAULT_CONFIG_PATH which doesn't exist on test host)
        // and falls back to defaults. It must not panic.
        handle_reload(&mut manager);
    }

    #[test]
    fn test_spawn_all_agents_with_empty_config() {
        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(Arc::clone(&registry)));
        let scheduler = Arc::new(AgentScheduler::new(Arc::clone(&registry)));
        let enforcer = Arc::new(AuthorityEnforcer::new(Arc::clone(&registry)));
        let quota = Arc::new(ResourceQuota::new(registry));

        let started = spawn_all_agents(&[], &supervisor, &scheduler, &enforcer, &quota);
        assert_eq!(started, 0);
    }

    #[test]
    fn test_spawn_all_agents_registers_in_registry() {
        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(Arc::clone(&registry)));
        let scheduler = Arc::new(AgentScheduler::new(Arc::clone(&registry)));
        let enforcer = Arc::new(AuthorityEnforcer::new(Arc::clone(&registry)));
        let quota = Arc::new(ResourceQuota::new(registry));

        let agent_cfg = vec![AgentServiceConfig {
            agent_id: "test-agent-1".to_string(),
            agent_type: eneros_os::agentos::AgentType::Dispatch,
            authority: eneros_core::AuthorityLevel::Supervisor,
            binary: "/bin/eneros-dispatch-agent".to_string(),
            ..Default::default()
        }];

        let started = spawn_all_agents(&agent_cfg, &supervisor, &scheduler, &enforcer, &quota);
        assert_eq!(started, 1);

        // Verify the agent was registered. On non-Linux, health_check may
        // return Crashed (because is_alive() stub returns false), but the
        // key assertion is that the agent is found (not NotFound).
        let status = supervisor.health_check("test-agent-1").unwrap();
        assert!(
            status == eneros_os::agentos::AgentStatus::Running
                || status == eneros_os::agentos::AgentStatus::Crashed,
            "agent should be registered (Running or Crashed on non-Linux), got {:?}",
            status
        );
    }

    #[test]
    fn test_stop_all_agents_idempotent() {
        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(registry));

        // Stopping with empty config should not crash
        stop_all_agents(&[], &supervisor);

        // Stopping agents that were never spawned should not crash
        let cfg = vec![AgentServiceConfig {
            agent_id: "never-spawned".to_string(),
            binary: "/bin/nope".to_string(),
            ..Default::default()
        }];
        stop_all_agents(&cfg, &supervisor);
    }

    #[test]
    fn test_restart_crashed_agents_no_crash_with_empty() {
        let registry = Arc::new(AgentRegistry::new());
        let supervisor = Arc::new(AgentSupervisor::new(registry));

        // Should not crash with empty config
        restart_crashed_agents(&[], &supervisor);
    }
}
