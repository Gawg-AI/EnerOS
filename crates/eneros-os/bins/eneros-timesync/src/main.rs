//! EnerOS 时间同步守护进程
//!
//! 加载 `/etc/eneros/timesync.toml` 配置，构造 [`TimeSyncManager`]，
//! 执行一次性 `apply()` 后进入后台守护循环：
//! - PTP 模式：try_wait 监控 ptp4l/phc2sys 子进程，崩溃重启（指数退避），
//!   每 10 秒通过 pmc 轮询更新 status。
//! - NTP 模式：按 poll_interval_secs 周期同步。
//!
//! 处理 SIGTERM/SIGINT 优雅退出。

use eneros_os::init::TimeSyncManager;
#[cfg(target_os = "linux")]
use eneros_os::init::request_daemon_shutdown;
use std::path::Path;
use std::process::ExitCode;

/// 默认配置文件路径
const DEFAULT_CONFIG_PATH: &str = "/etc/eneros/timesync.toml";

fn main() -> ExitCode {
    // 1. 初始化日志
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let pid = std::process::id();
    tracing::info!("EnerOS timesync daemon starting (pid={})", pid);

    // 2. 加载配置
    let config_path = std::env::var("ENEROS_TIMESYNC_CONFIG")
        .unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    tracing::info!("Loading timesync config from: {}", config_path);

    let mut manager = match TimeSyncManager::load(Path::new(&config_path)) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to load config from {}: {}", config_path, e);
            return ExitCode::FAILURE;
        }
    };

    // 3. 安装信号处理器（SIGTERM/SIGINT → 优雅退出）
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = install_signal_handlers() {
            tracing::error!("Failed to install signal handlers: {}", e);
            return ExitCode::FAILURE;
        }
        tracing::info!("Signal handlers installed (SIGTERM/SIGINT)");
    }

    // 4. 执行一次性同步
    tracing::info!("Applying initial time sync...");
    if let Err(e) = manager.apply() {
        tracing::warn!("Initial apply failed: {} (continuing to daemon mode)", e);
    }

    let source = manager.status().source;
    tracing::info!("Active clock source: {:?}", source);

    // 5. 进入守护循环
    #[cfg(target_os = "linux")]
    {
        tracing::info!("Entering daemon loop");
        if let Err(e) = manager.run_daemon() {
            tracing::error!("Daemon loop error: {}", e);
            return ExitCode::FAILURE;
        }
        tracing::info!("Daemon loop exited gracefully");
    }

    #[cfg(not(target_os = "linux"))]
    {
        tracing::warn!("timesync daemon not supported on this platform");
        let _ = manager;
    }

    ExitCode::SUCCESS
}

/// 安装 SIGTERM/SIGINT 信号处理器（Linux only）
///
/// 信号处理器仅执行原子 store（`request_daemon_shutdown`），是 async-signal-safe 的。
/// `run_daemon` 内部轮询 `DAEMON_SHUTDOWN` 标志实现优雅退出。
#[cfg(target_os = "linux")]
fn install_signal_handlers() -> Result<(), String> {
    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

    extern "C" fn handle_shutdown(_sig: libc::c_int) {
        request_daemon_shutdown();
    }

    let action = SigAction::new(
        SigHandler::Handler(handle_shutdown),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );

    unsafe {
        for sig in [Signal::SIGTERM, Signal::SIGINT] {
            sigaction(sig, &action)
                .map_err(|e| format!("sigaction {} failed: {}", sig, e))?;
        }
    }

    Ok(())
}
