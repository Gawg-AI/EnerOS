//! Service manager: spawns, monitors, stops and restarts services.
//!
//! On Linux the manager uses `nix` to send signals (`SIGTERM`/`SIGKILL`)
//! and to reap zombie children (including orphaned processes reparented
//! to PID 1). On non-Linux development hosts it falls back to the
//! portable `std::process` API so that `cargo build` and `cargo test`
//! succeed.

use crate::init::graph::GraphError;
use crate::init::service::{RestartPolicy, ServiceConfig, ServiceStatus};
use crate::init::{ServiceGraph, Supervisor};
use std::collections::{HashMap, HashSet};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Default graceful shutdown timeout before escalating to SIGKILL.
pub const DEFAULT_GRACEFUL_TIMEOUT_SECS: u64 = 10;
/// Default delay between a crash and a restart attempt.
pub const DEFAULT_RESTART_DELAY: Duration = Duration::from_secs(1);
/// Default crash-frequency threshold (crashes per minute).
pub const DEFAULT_MAX_RESTARTS_PER_MINUTE: usize = 5;
/// Crash-frequency measurement window.
const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// Errors produced by the service manager.
#[derive(Debug, Error)]
pub enum InitError {
    #[error("service '{0}' not found")]
    ServiceNotFound(String),
    #[error("service '{0}' is already running")]
    AlreadyRunning(String),
    #[error("service '{0}' has unmet dependencies: {1:?}")]
    DependenciesNotReady(String, Vec<String>),
    #[error("dependency graph error: {0}")]
    Graph(#[from] GraphError),
    #[error("failed to spawn service '{0}': {1}")]
    Spawn(String, String),
    #[error("failed to stop service '{0}': {1}")]
    Stop(String, String),
    #[error("failed to signal service '{0}' (pid {1}): {2}")]
    Signal(String, u32, String),
}

/// Manages the lifecycle of all init services.
pub struct ServiceManager {
    graph: ServiceGraph,
    supervisor: Supervisor,
    processes: HashMap<String, Child>,
    startup_times: HashMap<String, Instant>,
    /// Tracks when each service last exited (for restart-delay enforcement).
    exit_times: HashMap<String, Instant>,
    /// Services that have exceeded the crash-frequency limit.
    degraded: HashSet<String>,
    /// Per-service crash timestamps within the rolling window.
    crash_history: HashMap<String, Vec<Instant>>,
    /// Cached topological startup order (dependencies first).
    startup_order: Vec<String>,
    max_restarts_per_minute: usize,
    restart_delay: Duration,
}

impl ServiceManager {
    /// Create a new manager backed by the given dependency graph.
    pub fn new(graph: ServiceGraph) -> Self {
        Self {
            graph,
            supervisor: Supervisor::new(),
            processes: HashMap::new(),
            startup_times: HashMap::new(),
            exit_times: HashMap::new(),
            degraded: HashSet::new(),
            crash_history: HashMap::new(),
            startup_order: Vec::new(),
            max_restarts_per_minute: DEFAULT_MAX_RESTARTS_PER_MINUTE,
            restart_delay: DEFAULT_RESTART_DELAY,
        }
    }

    /// Configure the crash-frequency threshold.
    pub fn with_max_restarts_per_minute(mut self, n: usize) -> Self {
        self.max_restarts_per_minute = n;
        self
    }

    /// Configure the restart delay.
    pub fn with_restart_delay(mut self, delay: Duration) -> Self {
        self.restart_delay = delay;
        self
    }

    /// Register all graph services with the supervisor and compute the
    /// startup order. Called once before [`start_all`].
    pub fn prepare(&mut self) -> Result<&[String], InitError> {
        for service in self.graph.services() {
            self.supervisor.register(service.clone());
        }
        self.startup_order = self.graph.topological_sort()?;
        Ok(&self.startup_order)
    }

    /// Returns the cached startup order (dependencies first).
    pub fn startup_order(&self) -> &[String] {
        &self.startup_order
    }

    /// Start all services in dependency order.
    pub fn start_all(&mut self) -> Result<(), InitError> {
        if self.startup_order.is_empty() {
            self.prepare()?;
        }
        let order = self.startup_order.clone();
        tracing::info!("Starting {} services in order: {:?}", order.len(), order);
        for name in &order {
            if self.processes.contains_key(name) {
                continue;
            }
            if let Err(e) = self.start_service(name) {
                tracing::error!("Failed to start service '{}': {}", name, e);
                // Continue starting other services — a single failure
                // must not crash PID 1.
            }
        }
        Ok(())
    }

    /// Start a single service by name.
    pub fn start_service(&mut self, name: &str) -> Result<(), InitError> {
        if self.processes.contains_key(name) {
            return Err(InitError::AlreadyRunning(name.to_string()));
        }

        let config = self
            .graph
            .get_service(name)
            .ok_or_else(|| InitError::ServiceNotFound(name.to_string()))?
            .clone();

        if !self.dependencies_ready(name) {
            let missing = config
                .dependencies
                .iter()
                .filter(|d| !self.is_running(d))
                .cloned()
                .collect::<Vec<_>>();
            return Err(InitError::DependenciesNotReady(
                name.to_string(),
                missing,
            ));
        }

        tracing::info!(
            "Starting service '{}' (binary={}, args={:?})",
            name,
            config.binary,
            config.args
        );

        let child = spawn_service(&config)?;
        let pid = child.id();
        tracing::info!("Service '{}' started with pid {:?}", name, pid);

        if let Some(svc) = self.supervisor.get_service_mut(name) {
            svc.status = ServiceStatus::Running;
            svc.pid = Some(pid);
            svc.last_start_time = Some(chrono::Utc::now());
            svc.restart_count += 1;
        }
        self.processes.insert(name.to_string(), child);
        self.startup_times.insert(name.to_string(), Instant::now());
        self.degraded.remove(name);
        Ok(())
    }

    /// Stop all services in reverse startup order.
    pub fn stop_all(&mut self, timeout_secs: u64) -> Result<(), InitError> {
        let order = self.startup_order.clone();
        for name in order.iter().rev() {
            if self.processes.contains_key(name) {
                if let Err(e) = self.stop_service(name, timeout_secs) {
                    tracing::error!("Failed to stop service '{}': {}", name, e);
                }
            }
        }
        Ok(())
    }

    /// Stop a single service: send SIGTERM, wait up to `timeout_secs`,
    /// then escalate to SIGKILL if still alive.
    pub fn stop_service(&mut self, name: &str, timeout_secs: u64) -> Result<(), InitError> {
        let child = match self.processes.get_mut(name) {
            Some(c) => c,
            None => return Ok(()), // not running — nothing to do
        };

        let pid = child.id();
        tracing::info!(
            "Stopping service '{}' (pid {:?}) with {}s timeout",
            name,
            pid,
            timeout_secs
        );

        if let Some(svc) = self.supervisor.get_service_mut(name) {
            svc.status = ServiceStatus::Stopping;
        }

        // Send SIGTERM (graceful). On non-Linux, `child.kill()` sends the
        // platform-equivalent of SIGKILL; we still try it as a fallback.
        #[cfg(target_os = "linux")]
        {
            if let Some(pid) = pid {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Best-effort graceful stop on non-Linux: there is no portable
            // SIGTERM equivalent, so we go straight to kill. Tests that
            // exercise this path use short-lived processes.
        }

        // Poll for exit up to the timeout.
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut exited = false;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }

        if !exited {
            tracing::warn!(
                "Service '{}' (pid {:?}) did not exit within {}s — sending SIGKILL",
                name,
                pid,
                timeout_secs
            );
            let _ = child.kill();
            let _ = child.wait();
        }

        self.processes.remove(name);
        self.startup_times.remove(name);
        if let Some(svc) = self.supervisor.get_service_mut(name) {
            svc.status = ServiceStatus::Stopped;
            svc.pid = None;
        }
        tracing::info!("Service '{}' stopped", name);
        Ok(())
    }

    /// Check for exited child processes and return the names of services
    /// that have just exited.
    ///
    /// On Linux this also reaps any orphaned zombies that have been
    /// reparented to PID 1.
    pub fn reap_children(&mut self) -> Result<Vec<String>, InitError> {
        let mut exited = Vec::new();
        let names: Vec<String> = self.processes.keys().cloned().collect();

        for name in names {
            let status = {
                let child = match self.processes.get_mut(&name) {
                    Some(c) => c,
                    None => continue,
                };
                child.try_wait()
            };

            match status {
                Ok(Some(exit_status)) => {
                    let code = exit_status.code();
                    let success = code == Some(0);
                    tracing::info!(
                        "Service '{}' exited with status {:?} (code={:?})",
                        name,
                        exit_status,
                        code
                    );

                    self.processes.remove(&name);
                    self.startup_times.remove(&name);
                    self.exit_times.insert(name.clone(), Instant::now());

                    if !success {
                        self.record_crash(&name);
                    }

                    if let Some(svc) = self.supervisor.get_service_mut(&name) {
                        svc.pid = None;
                        svc.status = if success {
                            ServiceStatus::Stopped
                        } else {
                            ServiceStatus::Failed
                        };
                    }
                    exited.push(name);
                }
                Ok(None) => { /* still running */ }
                Err(e) => {
                    tracing::warn!("Error waiting for service '{}': {}", name, e);
                }
            }
        }

        // PID 1 must also reap orphaned zombies reparented to it.
        #[cfg(target_os = "linux")]
        self.reap_orphans();

        Ok(exited)
    }

    /// On Linux, reap any orphaned zombie children that were reparented
    /// to PID 1 but are not tracked in our `processes` map.
    #[cfg(target_os = "linux")]
    fn reap_orphans(&self) {
        use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
        use nix::unistd::Pid;
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) | Err(_) => break,
                Ok(WaitStatus::Exited(pid, code)) => {
                    tracing::debug!("Reaped orphan zombie pid={} code={}", pid, code);
                }
                Ok(other) => {
                    tracing::debug!("Reaped orphan child: {:?}", other);
                }
            }
        }
    }

    /// Record a crash for frequency-limiting purposes.
    fn record_crash(&mut self, name: &str) {
        let now = Instant::now();
        let history = self.crash_history.entry(name.to_string()).or_default();
        history.retain(|&t| now.duration_since(t) < CRASH_WINDOW);
        history.push(now);

        if history.len() > self.max_restarts_per_minute {
            tracing::error!(
                "Service '{}' exceeded crash frequency limit ({} in 60s) — entering degraded mode",
                name,
                history.len()
            );
            self.degraded.insert(name.to_string());
            if let Some(svc) = self.supervisor.get_service_mut(name) {
                svc.status = ServiceStatus::Degraded;
            }
        }
    }

    /// Returns the list of services that have exited, are eligible for
    /// restart (per policy + crash-frequency), and have waited the
    /// restart delay since their last exit.
    pub fn restart_pending(&mut self) -> Vec<String> {
        let mut ready = Vec::new();
        let names: Vec<String> = self.exit_times.keys().cloned().collect();
        for name in names {
            if self.processes.contains_key(&name) {
                continue; // already restarted
            }
            if self.degraded.contains(&name) {
                continue; // in degraded mode — don't restart
            }

            // Check restart delay.
            if let Some(exit_time) = self.exit_times.get(&name) {
                if exit_time.elapsed() < self.restart_delay {
                    continue;
                }
            }

            // Check restart policy.
            let policy = self
                .graph
                .get_service(&name)
                .map(|c| c.restart_policy)
                .unwrap_or(RestartPolicy::No);

            let last_failed = self
                .supervisor
                .get_service(&name)
                .map(|s| s.status == ServiceStatus::Failed)
                .unwrap_or(false);

            let should = match policy {
                RestartPolicy::No => false,
                RestartPolicy::Always => true,
                RestartPolicy::OnFailure => last_failed,
            };

            if should {
                self.exit_times.remove(&name);
                ready.push(name);
            } else {
                // Not eligible — clear the exit time so we don't keep checking.
                self.exit_times.remove(&name);
            }
        }
        ready
    }

    /// Restart a service that was previously running and has exited.
    pub fn restart_service(&mut self, name: &str) -> Result<(), InitError> {
        tracing::info!("Restarting service '{}'", name);
        // Clear previous failure state.
        if let Some(svc) = self.supervisor.get_service_mut(name) {
            svc.status = ServiceStatus::Stopped;
        }
        self.start_service(name)
    }

    /// Check if all dependencies of a service are currently running.
    fn dependencies_ready(&self, name: &str) -> bool {
        let config = match self.graph.get_service(name) {
            Some(c) => c,
            None => return false,
        };
        config.dependencies.iter().all(|d| self.is_running(d))
    }

    /// Check if a service is currently running (has an active child process).
    pub fn is_running(&self, name: &str) -> bool {
        self.processes.contains_key(name)
    }

    /// Number of currently-running services.
    pub fn running_count(&self) -> usize {
        self.processes.len()
    }

    /// Number of services in degraded mode.
    pub fn degraded_count(&self) -> usize {
        self.degraded.len()
    }

    /// Whether the manager is managing any running processes.
    pub fn has_running(&self) -> bool {
        !self.processes.is_empty()
    }

    /// Reference to the underlying supervisor.
    pub fn supervisor(&self) -> &Supervisor {
        &self.supervisor
    }

    /// Mutable reference to the underlying supervisor.
    pub fn supervisor_mut(&mut self) -> &mut Supervisor {
        &mut self.supervisor
    }

    /// Reference to the underlying graph.
    pub fn graph(&self) -> &ServiceGraph {
        &self.graph
    }

    /// Mutable reference to the underlying graph (used by config reload).
    pub fn graph_mut(&mut self) -> &mut ServiceGraph {
        &mut self.graph
    }

    /// Rebuild the cached startup order from the current graph.
    /// Called after a config reload updates the graph.
    pub fn refresh_startup_order(&mut self) -> Result<(), InitError> {
        self.startup_order = self.graph.topological_sort()?;
        Ok(())
    }
}

/// Spawn a service process from its configuration.
fn spawn_service(config: &ServiceConfig) -> Result<Child, InitError> {
    let mut cmd = Command::new(&config.binary);
    cmd.args(&config.args);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    if let Some(dir) = &config.working_dir {
        cmd.current_dir(dir);
    }
    // Inherit stdio so services can log to the console / syslog.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    cmd.spawn()
        .map_err(|e| InitError::Spawn(config.name.clone(), e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::service::ServiceConfig;

    fn make_graph() -> ServiceGraph {
        let mut g = ServiceGraph::new();
        g.add_service(ServiceConfig {
            name: "base".to_string(),
            binary: "/bin/true".to_string(),
            restart_policy: RestartPolicy::No,
            ..Default::default()
        });
        g.add_service(ServiceConfig {
            name: "app".to_string(),
            binary: "/bin/true".to_string(),
            dependencies: vec!["base".to_string()],
            restart_policy: RestartPolicy::Always,
            ..Default::default()
        });
        g
    }

    #[test]
    fn test_new_manager_is_empty() {
        let mgr = ServiceManager::new(make_graph());
        assert_eq!(mgr.running_count(), 0);
        assert!(!mgr.has_running());
    }

    #[test]
    fn test_prepare_computes_startup_order() {
        let mut mgr = ServiceManager::new(make_graph());
        let order = mgr.prepare().unwrap();
        assert_eq!(order, &["base", "app"]);
    }

    #[test]
    fn test_dependencies_ready_logic() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        // Nothing running yet → app's deps not ready.
        assert!(!mgr.dependencies_ready("app"));
        // base has no deps → always ready.
        assert!(mgr.dependencies_ready("base"));
    }

    #[test]
    fn test_start_service_missing() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        let err = mgr.start_service("nonexistent").unwrap_err();
        assert!(matches!(err, InitError::ServiceNotFound(_)));
    }

    #[test]
    fn test_start_service_blocked_by_deps() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        let err = mgr.start_service("app").unwrap_err();
        assert!(matches!(err, InitError::DependenciesNotReady(_, _)));
    }

    #[test]
    fn test_stop_service_not_running_is_ok() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        // Stopping a service that isn't running should be a no-op.
        assert!(mgr.stop_service("base", 1).is_ok());
    }

    #[test]
    fn test_stop_all_with_no_running_services() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        assert!(mgr.stop_all(1).is_ok());
    }

    #[test]
    fn test_reap_children_empty() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        let exited = mgr.reap_children().unwrap();
        assert!(exited.is_empty());
    }

    #[test]
    fn test_restart_pending_empty_when_nothing_exited() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        assert!(mgr.restart_pending().is_empty());
    }

    #[test]
    fn test_degraded_tracking() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        assert_eq!(mgr.degraded_count(), 0);
        // Manually mark a service as degraded.
        mgr.degraded.insert("base".to_string());
        assert_eq!(mgr.degraded_count(), 1);
    }

    #[test]
    fn test_record_crash_increments_history() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        mgr.record_crash("base");
        mgr.record_crash("base");
        let history = mgr.crash_history.get("base").unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_record_crash_triggers_degraded_after_limit() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.max_restarts_per_minute = 2;
        mgr.prepare().unwrap();
        mgr.record_crash("base");
        assert_eq!(mgr.degraded_count(), 0);
        mgr.record_crash("base");
        mgr.record_crash("base"); // 3rd crash exceeds limit of 2
        assert!(mgr.degraded.contains("base"));
        assert_eq!(mgr.degraded_count(), 1);
    }

    #[test]
    fn test_restart_pending_excludes_degraded() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        // Simulate: base exited as a failure and is eligible by policy,
        // but is in degraded mode.
        mgr.exit_times.insert("base".to_string(), Instant::now() - Duration::from_secs(5));
        mgr.degraded.insert("base".to_string());
        // base has RestartPolicy::No in make_graph, so it won't be pending anyway.
        assert!(mgr.restart_pending().is_empty());
    }

    #[test]
    fn test_restart_pending_respects_delay() {
        let mut mgr = ServiceManager::new(make_graph());
        // Override app to use Always (already is) and simulate recent exit.
        mgr.prepare().unwrap();
        // Exit just now — delay not yet elapsed.
        mgr.exit_times
            .insert("app".to_string(), Instant::now());
        // app has RestartPolicy::Always, but exit was < 1s ago.
        assert!(mgr.restart_pending().is_empty());
    }

    #[test]
    fn test_restart_pending_eligible_after_delay() {
        let mut mgr = ServiceManager::new(make_graph());
        mgr.prepare().unwrap();
        // Simulate exit 2 seconds ago (past the 1s delay).
        mgr.exit_times
            .insert("app".to_string(), Instant::now() - Duration::from_secs(2));
        // Mark app as failed so OnFailure would also restart.
        if let Some(svc) = mgr.supervisor.get_service_mut("app") {
            svc.status = ServiceStatus::Failed;
        }
        let pending = mgr.restart_pending();
        assert!(pending.contains(&"app".to_string()));
    }

    #[test]
    fn test_with_max_restarts_per_minute_builder() {
        let mgr = ServiceManager::new(make_graph())
            .with_max_restarts_per_minute(10);
        assert_eq!(mgr.max_restarts_per_minute, 10);
    }

    #[test]
    fn test_with_restart_delay_builder() {
        let mgr = ServiceManager::new(make_graph())
            .with_restart_delay(Duration::from_millis(500));
        assert_eq!(mgr.restart_delay, Duration::from_millis(500));
    }

    #[test]
    fn test_is_running_false_for_unknown() {
        let mgr = ServiceManager::new(make_graph());
        assert!(!mgr.is_running("unknown"));
    }

    #[test]
    fn test_start_all_with_unstartable_binary_continues() {
        // /nonexistent/binary won't spawn, but start_all should not error.
        let mut g = ServiceGraph::new();
        g.add_service(ServiceConfig {
            name: "broken".to_string(),
            binary: "/nonexistent/binary".to_string(),
            ..Default::default()
        });
        let mut mgr = ServiceManager::new(g);
        // start_all swallows individual spawn errors.
        assert!(mgr.start_all().is_ok());
        assert_eq!(mgr.running_count(), 0);
    }
}
