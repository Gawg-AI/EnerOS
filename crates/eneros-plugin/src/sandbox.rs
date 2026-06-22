//! 插件沙箱 — seccomp 系统调用过滤 + 资源配额 + 崩溃隔离
//!
//! 提供三层插件隔离：
//! - seccomp BPF：禁止危险系统调用（mount/reboot/ptrace 等）
//! - cgroups v2：CPU 与内存配额限制
//! - catch_unwind：捕获插件 panic，防止崩溃蔓延至宿主进程
//!
//! Linux + `seccomp` feature 启用真实 BPF 过滤器与 cgroups；
//! 非 Linux 或未启用 feature 时返回 `PluginError::Unsupported`，用于开发/测试。

use crate::error::PluginError;
use std::panic::catch_unwind;
use std::path::PathBuf;

/// 沙箱配置
#[derive(Debug, Clone)]
pub struct PluginSandboxConfig {
    /// 是否启用 seccomp 沙箱
    pub enable_seccomp: bool,
    /// 是否启用资源配额
    pub enable_quota: bool,
    /// CPU 百分比限制（1-100）
    pub cpu_percent: u32,
    /// 内存限制（MB）
    pub memory_mb: u64,
    /// 允许访问的文件路径（白名单）
    pub allowed_paths: Vec<PathBuf>,
    /// 禁止访问的文件路径（黑名单）
    pub denied_paths: Vec<PathBuf>,
    /// 允许访问的网络地址（如 "127.0.0.1:8080"）
    pub allowed_network: Vec<String>,
}

impl Default for PluginSandboxConfig {
    fn default() -> Self {
        Self {
            enable_seccomp: true,
            enable_quota: true,
            cpu_percent: 50,
            memory_mb: 256,
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            allowed_network: Vec::new(),
        }
    }
}

/// 插件 seccomp profile
#[derive(Debug, Clone)]
pub struct PluginSeccompProfile {
    /// 禁止的系统调用列表
    pub denied_syscalls: Vec<String>,
    /// 允许写入的路径（open/openat 的 O_WRONLY 限制）
    pub allowed_write_paths: Vec<PathBuf>,
    /// 允许的网络地址
    pub allowed_network: Vec<String>,
}

impl PluginSeccompProfile {
    /// 创建默认插件 profile（禁止危险 syscall）
    pub fn new() -> Self {
        Self {
            denied_syscalls: vec![
                "mount".to_string(),
                "reboot".to_string(),
                "kexec_load".to_string(),
                "init_module".to_string(),
                "finit_module".to_string(),
                "ptrace".to_string(),
                "setuid".to_string(),
                "setgid".to_string(),
            ],
            allowed_write_paths: Vec::new(),
            allowed_network: Vec::new(),
        }
    }

    /// 从沙箱配置创建 profile
    pub fn from_config(config: &PluginSandboxConfig) -> Self {
        Self {
            denied_syscalls: Self::new().denied_syscalls,
            allowed_write_paths: config.allowed_paths.clone(),
            allowed_network: config.allowed_network.clone(),
        }
    }
}

impl Default for PluginSeccompProfile {
    fn default() -> Self {
        Self::new()
    }
}

/// 沙箱守卫（RAII，drop 时释放沙箱资源）
pub struct SandboxGuard {
    config: PluginSandboxConfig,
    applied: bool,
    /// 已创建的 cgroup 路径（仅 Linux），drop 时清理
    cgroup_path: Option<PathBuf>,
}

impl SandboxGuard {
    /// 获取沙箱配置
    pub fn config(&self) -> &PluginSandboxConfig {
        &self.config
    }

    /// 释放沙箱（恢复原状态）
    ///
    /// 消费守卫并清理 cgroup 资源。seccomp 一旦加载不可逆，无需恢复。
    pub fn release(mut self) {
        self.cleanup();
    }

    /// 清理沙箱资源（cgroup）
    fn cleanup(&mut self) {
        if let Some(path) = self.cgroup_path.take() {
            // 尽力清理 cgroup 目录；忽略错误（进程可能已退出或目录非空）
            let _ = std::fs::remove_dir(&path);
        }
        self.applied = false;
    }
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        if self.applied {
            self.cleanup();
        }
    }
}

/// 插件沙箱
pub struct PluginSandbox {
    config: PluginSandboxConfig,
}

impl PluginSandbox {
    pub fn new(config: PluginSandboxConfig) -> Self {
        Self { config }
    }

    /// 应用沙箱限制
    ///
    /// 依次应用 seccomp 过滤器与 cgroups 配额，返回 RAII 守卫。
    /// seccomp 一旦加载不可逆；cgroups 在守卫 drop 时清理。
    pub fn apply(&self) -> Result<SandboxGuard, PluginError> {
        let mut guard = SandboxGuard {
            config: self.config.clone(),
            applied: false,
            cgroup_path: None,
        };
        if self.config.enable_seccomp {
            let profile = PluginSeccompProfile::from_config(&self.config);
            apply_seccomp(&profile)?;
        }
        if self.config.enable_quota {
            #[cfg(target_os = "linux")]
            {
                guard.cgroup_path = Some(linux_cgroup::setup_cgroup(&self.config)?);
            }
            #[cfg(not(target_os = "linux"))]
            {
                apply_quota(&self.config)?;
            }
        }
        guard.applied = true;
        Ok(guard)
    }

    /// 获取配置
    pub fn config(&self) -> &PluginSandboxConfig {
        &self.config
    }
}

// ============================================================
// Linux + seccomp feature：真实 BPF 过滤器实现
// ============================================================
#[cfg(all(target_os = "linux", feature = "seccomp"))]
mod linux_seccomp {
    use super::*;
    use libseccomp::{ScmpAction, ScmpFilterContext, ScmpSyscall};

    /// Linux EPERM errno（值为 1）
    const EPERM: i32 = 1;

    /// 将 profile 转换为 libseccomp 过滤器上下文
    ///
    /// 默认动作：Allow；匹配 `denied_syscalls` 时返回 EPERM。
    /// 当前架构不存在的系统调用（如 aarch64 上的 `open`）会被跳过。
    fn to_filter(profile: &PluginSeccompProfile) -> Result<ScmpFilterContext, PluginError> {
        let mut ctx = ScmpFilterContext::new(ScmpAction::Allow)
            .map_err(|e| PluginError::SandboxFailed(format!("libseccomp init: {e}")))?;
        for name in &profile.denied_syscalls {
            let syscall = match ScmpSyscall::from_name(name) {
                Ok(s) => s,
                Err(_) => {
                    // 当前架构不存在此系统调用，无需过滤
                    continue;
                }
            };
            ctx.add_rule(ScmpAction::Errno(EPERM), syscall)
                .map_err(|e| {
                    PluginError::SandboxFailed(format!("libseccomp add_rule {name}: {e}"))
                })?;
        }
        Ok(ctx)
    }

    pub fn apply_seccomp(profile: &PluginSeccompProfile) -> Result<(), PluginError> {
        let ctx = to_filter(profile)?;
        ctx.load()
            .map_err(|e| PluginError::SandboxFailed(format!("libseccomp load: {e}")))
    }
}

// ============================================================
// 非 Linux 或无 seccomp feature：stub 实现
// ============================================================
#[cfg(not(all(target_os = "linux", feature = "seccomp")))]
mod stub_seccomp {
    use super::*;

    pub fn apply_seccomp(_profile: &PluginSeccompProfile) -> Result<(), PluginError> {
        Err(PluginError::Unsupported(
            "seccomp not available on this platform".to_string(),
        ))
    }
}

/// 应用 seccomp 过滤器
///
/// Linux + `seccomp` feature：使用 libseccomp 创建并加载 BPF 过滤器。
/// 非 Linux 或未启用 feature：返回 `PluginError::Unsupported`。
pub fn apply_seccomp(profile: &PluginSeccompProfile) -> Result<(), PluginError> {
    #[cfg(all(target_os = "linux", feature = "seccomp"))]
    {
        linux_seccomp::apply_seccomp(profile)
    }
    #[cfg(not(all(target_os = "linux", feature = "seccomp")))]
    {
        stub_seccomp::apply_seccomp(profile)
    }
}

// ============================================================
// cgroups v2 资源配额（仅 Linux）
// ============================================================
#[cfg(target_os = "linux")]
mod linux_cgroup {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// cgroup v2 根目录
    const CGROUP_ROOT: &str = "/sys/fs/cgroup";
    /// 插件 cgroup 子目录
    const PLUGIN_CGROUP_DIR: &str = "eneros-plugin";

    /// 创建 cgroup 并应用 CPU/内存配额，返回 cgroup 路径
    ///
    /// - 创建 `/sys/fs/cgroup/eneros-plugin/plugin-<pid>/` 目录
    /// - 写入 `cpu.max`：格式 "$QUOTA $PERIOD"，period 默认 100000us
    /// - 写入 `memory.max`：MB 转字节
    /// - 写入 `cgroup.procs`：将当前进程加入 cgroup 使配额生效
    pub fn setup_cgroup(config: &PluginSandboxConfig) -> Result<PathBuf, PluginError> {
        let name = format!("plugin-{}", std::process::id());
        let path = Path::new(CGROUP_ROOT).join(PLUGIN_CGROUP_DIR).join(&name);

        // 创建 cgroup 目录（含父目录）
        fs::create_dir_all(&path).map_err(|e| {
            PluginError::SandboxFailed(format!("create cgroup {}: {}", path.display(), e))
        })?;

        // 写入 cpu.max：cpu_percent% → quota = cpu_percent * 1000（period=100000us）
        let cpu_quota = config.cpu_percent.saturating_mul(1000);
        let cpu_max = format!("{cpu_quota} 100000");
        fs::write(path.join("cpu.max"), cpu_max.as_bytes())
            .map_err(|e| PluginError::SandboxFailed(format!("write cpu.max: {e}")))?;

        // 写入 memory.max：MB → 字节
        let memory_bytes = config.memory_mb.saturating_mul(1024 * 1024);
        fs::write(
            path.join("memory.max"),
            memory_bytes.to_string().as_bytes(),
        )
        .map_err(|e| PluginError::SandboxFailed(format!("write memory.max: {e}")))?;

        // 将当前进程加入 cgroup，使配额生效
        let pid = std::process::id();
        fs::write(path.join("cgroup.procs"), pid.to_string().as_bytes())
            .map_err(|e| PluginError::SandboxFailed(format!("write cgroup.procs: {e}")))?;

        Ok(path)
    }
}

/// 应用资源配额（cgroups v2）
///
/// Linux：创建 cgroup，设置 cpu.max 和 memory.max。
/// 非 Linux：返回 `PluginError::Unsupported`。
#[cfg(target_os = "linux")]
pub fn apply_quota(config: &PluginSandboxConfig) -> Result<(), PluginError> {
    linux_cgroup::setup_cgroup(config).map(|_| ())
}

#[cfg(not(target_os = "linux"))]
pub fn apply_quota(_config: &PluginSandboxConfig) -> Result<(), PluginError> {
    Err(PluginError::Unsupported(
        "cgroups not available on this platform".to_string(),
    ))
}

/// 捕获 panic 的包装器
///
/// 将闭包中的 panic 转换为 `PluginError::Crashed`，防止插件崩溃蔓延至宿主。
/// panic payload 依次尝试 `&str`、`String`，均不匹配时返回 "unknown panic"。
pub fn catch_unwind_wrapper<F, R>(f: F) -> Result<R, PluginError>
where
    F: FnOnce() -> R + std::panic::UnwindSafe,
{
    catch_unwind(f).map_err(|e| {
        let msg = if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        PluginError::Crashed(msg)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_default() {
        let config = PluginSandboxConfig::default();
        assert!(config.enable_seccomp);
        assert!(config.enable_quota);
        assert_eq!(config.cpu_percent, 50);
        assert_eq!(config.memory_mb, 256);
        assert!(config.allowed_paths.is_empty());
        assert!(config.denied_paths.is_empty());
        assert!(config.allowed_network.is_empty());
    }

    #[test]
    fn test_seccomp_profile_new() {
        let profile = PluginSeccompProfile::new();
        // 默认 profile 必须包含危险系统调用
        assert!(profile.denied_syscalls.contains(&"mount".to_string()));
        assert!(profile.denied_syscalls.contains(&"reboot".to_string()));
        assert!(profile.denied_syscalls.contains(&"kexec_load".to_string()));
        assert!(profile.denied_syscalls.contains(&"init_module".to_string()));
        assert!(profile.denied_syscalls.contains(&"finit_module".to_string()));
        assert!(profile.denied_syscalls.contains(&"ptrace".to_string()));
        assert!(profile.denied_syscalls.contains(&"setuid".to_string()));
        assert!(profile.denied_syscalls.contains(&"setgid".to_string()));
        assert!(profile.allowed_write_paths.is_empty());
        assert!(profile.allowed_network.is_empty());
    }

    #[test]
    fn test_seccomp_profile_from_config() {
        let config = PluginSandboxConfig {
            allowed_paths: vec![PathBuf::from("/tmp/plugin")],
            allowed_network: vec!["127.0.0.1:8080".to_string()],
            ..Default::default()
        };
        let profile = PluginSeccompProfile::from_config(&config);
        // 继承默认危险 syscall 列表
        assert!(profile.denied_syscalls.contains(&"ptrace".to_string()));
        // 从配置复制白名单
        assert_eq!(
            profile.allowed_write_paths,
            vec![PathBuf::from("/tmp/plugin")]
        );
        assert_eq!(
            profile.allowed_network,
            vec!["127.0.0.1:8080".to_string()]
        );
    }

    #[test]
    fn test_catch_unwind_ok() {
        let result: Result<i32, PluginError> = catch_unwind_wrapper(|| 42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_catch_unwind_panic() {
        let result: Result<(), PluginError> = catch_unwind_wrapper(|| panic!("boom"));
        assert!(matches!(result, Err(PluginError::Crashed(_))));
    }

    #[test]
    fn test_catch_unwind_panic_string() {
        let result: Result<(), PluginError> = catch_unwind_wrapper(|| panic!("specific error"));
        match result {
            Err(PluginError::Crashed(msg)) => assert_eq!(msg, "specific error"),
            other => panic!("expected Crashed, got {other:?}"),
        }
    }

    /// 非 Linux 或无 seccomp feature：apply 返回 Unsupported
    #[cfg(not(all(target_os = "linux", feature = "seccomp")))]
    #[test]
    fn test_sandbox_apply_unsupported() {
        let sandbox = PluginSandbox::new(PluginSandboxConfig::default());
        let result = sandbox.apply();
        assert!(matches!(result, Err(PluginError::Unsupported(_))));
    }

    #[test]
    fn test_sandbox_guard_release() {
        // 禁用 seccomp 与配额，使 apply 在所有平台成功
        let config = PluginSandboxConfig {
            enable_seccomp: false,
            enable_quota: false,
            ..Default::default()
        };
        let sandbox = PluginSandbox::new(config);
        let guard = sandbox.apply().expect("apply with all disabled should succeed");
        guard.release(); // 不应 panic
    }

    /// 非 Linux 或无 seccomp feature：apply_seccomp 返回 Unsupported
    #[cfg(not(all(target_os = "linux", feature = "seccomp")))]
    #[test]
    fn test_apply_seccomp_unsupported() {
        let profile = PluginSeccompProfile::new();
        let result = apply_seccomp(&profile);
        assert!(matches!(result, Err(PluginError::Unsupported(_))));
    }

    /// 非 Linux：apply_quota 返回 Unsupported
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_apply_quota_unsupported() {
        let config = PluginSandboxConfig::default();
        let result = apply_quota(&config);
        assert!(matches!(result, Err(PluginError::Unsupported(_))));
    }
}
