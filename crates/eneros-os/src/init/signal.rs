//! Signal handling for the init process (PID 1)
//!
//! On Linux, real signal handlers are installed via `nix` for SIGTERM,
//! SIGINT and SIGHUP. SIGCHLD is intentionally not blocked here so that
//! `waitpid` in the service manager can reap zombie children directly.
//!
//! On non-Linux targets (e.g. Windows development hosts), the handler is
//! a no-op and the flag setters are exposed for testing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Errors that can occur while installing signal handlers.
#[derive(Debug, Error)]
pub enum SignalError {
    #[error("failed to install signal handler for {0}: {1}")]
    Install(String, String),
}

/// Shared, atomic signal flags queried by the init main loop.
#[derive(Debug, Clone)]
pub struct SignalHandler {
    shutdown_requested: Arc<AtomicBool>,
    reload_requested: Arc<AtomicBool>,
}

impl SignalHandler {
    /// Create a new signal handler with all flags cleared.
    pub fn new() -> Self {
        Self {
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            reload_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Install OS signal handlers.
    ///
    /// On Linux this registers handlers for SIGTERM, SIGINT and SIGHUP.
    /// On other platforms it is a no-op (the flags remain usable from
    /// tests via [`request_shutdown`] / [`request_reload`]).
    #[cfg(target_os = "linux")]
    pub fn install(&self) -> Result<(), SignalError> {
        use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

        // Signal handlers only perform atomic stores, which are
        // async-signal-safe. We never allocate or call non-reentrant
        // functions from inside the handler.
        extern "C" fn handle_shutdown(_sig: libc::c_int) {
            SHUTDOWN_FLAG.store(true, Ordering::SeqCst);
        }
        extern "C" fn handle_reload(_sig: libc::c_int) {
            RELOAD_FLAG.store(true, Ordering::SeqCst);
        }

        // Reset static flags before installing.
        SHUTDOWN_FLAG.store(false, Ordering::SeqCst);
        RELOAD_FLAG.store(false, Ordering::SeqCst);

        let action_shutdown = SigAction::new(
            SigHandler::Handler(handle_shutdown),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        let action_reload = SigAction::new(
            SigHandler::Handler(handle_reload),
            SaFlags::SA_RESTART,
            SigSet::empty(),
        );

        unsafe {
            for sig in [Signal::SIGTERM, Signal::SIGINT] {
                sigaction(sig, &action_shutdown).map_err(|e| {
                    SignalError::Install(sig.to_string(), e.to_string())
                })?;
            }
            sigaction(Signal::SIGHUP, &action_reload).map_err(|e| {
                SignalError::Install(Signal::SIGHUP.to_string(), e.to_string())
            })?;
        }

        Ok(())
    }

    /// No-op install on non-Linux platforms.
    #[cfg(not(target_os = "linux"))]
    pub fn install(&self) -> Result<(), SignalError> {
        Ok(())
    }

    /// Returns `true` if a shutdown signal (SIGTERM/SIGINT) was received.
    pub fn should_shutdown(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            if SHUTDOWN_FLAG.load(Ordering::SeqCst) {
                return true;
            }
        }
        self.shutdown_requested.load(Ordering::SeqCst)
    }

    /// Returns `true` if a reload signal (SIGHUP) was received.
    pub fn should_reload(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            if RELOAD_FLAG.load(Ordering::SeqCst) {
                return true;
            }
        }
        self.reload_requested.load(Ordering::SeqCst)
    }

    /// Clear the reload flag after the main loop has processed a reload.
    pub fn clear_reload(&self) {
        #[cfg(target_os = "linux")]
        {
            RELOAD_FLAG.store(false, Ordering::SeqCst);
        }
        self.reload_requested.store(false, Ordering::SeqCst);
    }

    /// Clear the shutdown flag (used by tests).
    pub fn clear_shutdown(&self) {
        #[cfg(target_os = "linux")]
        {
            SHUTDOWN_FLAG.store(false, Ordering::SeqCst);
        }
        self.shutdown_requested.store(false, Ordering::SeqCst);
    }

    /// Test helper: simulate a shutdown signal being received.
    pub fn request_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
    }

    /// Test helper: simulate a reload signal being received.
    pub fn request_reload(&self) {
        self.reload_requested.store(true, Ordering::SeqCst);
    }
}

impl Default for SignalHandler {
    fn default() -> Self {
        Self::new()
    }
}

// Static flags used by the Linux signal handlers. They mirror the
// `Arc<AtomicBool>` fields so that the C-language handler (which cannot
// capture Rust state) can still communicate with the main loop.
#[cfg(target_os = "linux")]
static SHUTDOWN_FLAG: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "linux")]
static RELOAD_FLAG: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_handler_has_cleared_flags() {
        let h = SignalHandler::new();
        assert!(!h.should_shutdown());
        assert!(!h.should_reload());
    }

    #[test]
    fn test_request_shutdown_sets_flag() {
        let h = SignalHandler::new();
        h.request_shutdown();
        assert!(h.should_shutdown());
        assert!(!h.should_reload());
    }

    #[test]
    fn test_request_reload_sets_flag() {
        let h = SignalHandler::new();
        h.request_reload();
        assert!(h.should_reload());
        assert!(!h.should_shutdown());
    }

    #[test]
    fn test_clear_reload_resets_flag() {
        let h = SignalHandler::new();
        h.request_reload();
        assert!(h.should_reload());
        h.clear_reload();
        assert!(!h.should_reload());
    }

    #[test]
    fn test_clear_shutdown_resets_flag() {
        let h = SignalHandler::new();
        h.request_shutdown();
        assert!(h.should_shutdown());
        h.clear_shutdown();
        assert!(!h.should_shutdown());
    }

    #[test]
    fn test_install_does_not_error() {
        // On non-Linux this is a no-op; on Linux it installs real handlers.
        // Either way it should not return an error in a test environment.
        let h = SignalHandler::new();
        assert!(h.install().is_ok());
    }

    #[test]
    fn test_default_impl() {
        let h = SignalHandler::default();
        assert!(!h.should_shutdown());
        assert!(!h.should_reload());
    }

    #[test]
    fn test_clone_shares_state() {
        let h = SignalHandler::new();
        let h2 = h.clone();
        h.request_shutdown();
        assert!(h2.should_shutdown());
        h2.clear_shutdown();
        assert!(!h.should_shutdown());
    }
}
