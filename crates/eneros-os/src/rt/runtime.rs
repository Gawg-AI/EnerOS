use serde::{Deserialize, Serialize};

/// Real-time runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtConfig {
    /// CPU cores isolated for RT tasks (e.g., [2, 3])
    pub cpus: Vec<u32>,
    /// SCHED_FIFO priority (1-99)
    pub priority: u32,
    /// Lock all memory pages (mlockall)
    pub lock_memory: bool,
    /// Use huge pages
    pub use_huge_pages: bool,
}

impl Default for RtConfig {
    fn default() -> Self {
        Self {
            cpus: vec![],
            priority: 80,
            lock_memory: true,
            use_huge_pages: false,
        }
    }
}

/// Real-time runtime manager
#[derive(Debug)]
pub struct RtRuntime {
    config: RtConfig,
}

impl RtRuntime {
    pub fn new(config: RtConfig) -> Self {
        Self { config }
    }

    /// Configure the current thread for real-time scheduling
    pub fn configure_current_thread(&self) -> Result<(), RtError> {
        #[cfg(target_os = "linux")]
        {
            use std::mem;

            // Set CPU affinity
            if !self.config.cpus.is_empty() {
                let mut cpuset: libc::cpu_set_t = unsafe { mem::zeroed() };
                for &cpu in &self.config.cpus {
                    unsafe { libc::CPU_SET(cpu as usize, &mut cpuset) };
                }
                let ret = unsafe {
                    libc::sched_setaffinity(0, mem::size_of::<libc::cpu_set_t>(), &cpuset)
                };
                if ret != 0 {
                    return Err(RtError::CpuAffinityFailed(std::io::Error::last_os_error()));
                }
            }

            // Set SCHED_FIFO scheduling
            let param = libc::sched_param {
                sched_priority: self.config.priority as i32,
            };
            let ret = unsafe {
                libc::sched_setscheduler(0, libc::SCHED_FIFO, &param)
            };
            if ret != 0 {
                return Err(RtError::SchedSetFailed(std::io::Error::last_os_error()));
            }

            // Lock memory
            if self.config.lock_memory {
                let ret = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
                if ret != 0 {
                    return Err(RtError::MlockFailed(std::io::Error::last_os_error()));
                }
            }

            // Configure huge pages
            if self.config.use_huge_pages {
                // Reserve static huge pages (requires root)
                std::fs::write("/proc/sys/vm/nr_hugepages", b"20")
                    .map_err(RtError::HugePageFailed)?;

                // Enable transparent huge pages for the current thread's stack
                let mut attr: libc::pthread_attr_t = unsafe { mem::zeroed() };
                let ret = unsafe { libc::pthread_getattr_np(libc::pthread_self(), &mut attr) };
                if ret != 0 {
                    return Err(RtError::HugePageFailed(std::io::Error::last_os_error()));
                }

                let mut stack_addr: *mut libc::c_void = std::ptr::null_mut();
                let mut stack_size: libc::size_t = 0;
                let getstack_ret = unsafe {
                    libc::pthread_attr_getstack(&attr, &mut stack_addr, &mut stack_size)
                };
                unsafe { libc::pthread_attr_destroy(&mut attr) };
                if getstack_ret != 0 {
                    return Err(RtError::HugePageFailed(std::io::Error::last_os_error()));
                }

                let ret = unsafe { libc::madvise(stack_addr, stack_size, libc::MADV_HUGEPAGE) };
                if ret != 0 {
                    return Err(RtError::HugePageFailed(std::io::Error::last_os_error()));
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            // Non-Linux: no-op (development environment)
            let _ = &self.config;
        }

        Ok(())
    }

    pub fn config(&self) -> &RtConfig {
        &self.config
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RtError {
    #[error("failed to set CPU affinity: {0}")]
    CpuAffinityFailed(std::io::Error),
    #[error("failed to set SCHED_FIFO: {0}")]
    SchedSetFailed(std::io::Error),
    #[error("failed to lock memory: {0}")]
    MlockFailed(std::io::Error),
    #[error("failed to configure huge pages: {0}")]
    HugePageFailed(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rt_config_default() {
        let config = RtConfig::default();
        assert_eq!(config.priority, 80);
        assert!(config.lock_memory);
    }

    #[test]
    fn test_rt_runtime_creation() {
        let runtime = RtRuntime::new(RtConfig::default());
        assert_eq!(runtime.config().priority, 80);
    }
}
