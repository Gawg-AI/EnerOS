//! seccomp BPF 过滤器 — 按 AuthorityLevel 限制 Agent 可用系统调用
//!
//! 将应用层 AuthorityLevel 映射为 Linux seccomp BPF 规则，在 Agent 进程
//! exec 前通过 `pre_exec` 钩子加载，实现 OS 级 syscall 沙箱。
//!
//! 权限层级与限制策略：
//! - Observer：最严格（禁止写文件、AF_PACKET 原始套接字、挂载、重启、加载模块、ptrace）
//! - Operator：禁止 AF_PACKET、挂载、重启、加载模块、ptrace
//! - Supervisor：禁止 kexec、加载模块、ptrace
//! - Emergency：仅禁止 ptrace
//!
//! 非 Linux 平台或未启用 `seccomp` feature 时提供 stub 实现，用于开发/测试。

use eneros_core::AuthorityLevel;

// ---- Linux ABI 常量（稳定值，跨架构一致）----
/// Linux O_WRONLY 标志位（值为 1）
const O_WRONLY: u64 = 1;
/// Linux AF_PACKET 地址族（值为 17）
const AF_PACKET: u64 = 17;
/// Linux EPERM errno（值为 1）
const EPERM: i32 = 1;

/// seccomp 错误类型
#[derive(Debug, thiserror::Error)]
pub enum SeccompError {
    /// libseccomp 库调用失败
    #[error("libseccomp error: {0}")]
    Libseccomp(String),
    /// 当前平台或 feature 不支持 seccomp
    #[error("seccomp unsupported on this platform or feature not enabled")]
    Unsupported,
    /// profile 配置无效
    #[error("invalid seccomp profile: {0}")]
    InvalidProfile(String),
}

/// seccomp 动作
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeccompAction {
    /// 允许系统调用
    Allow,
    /// 返回指定 errno（默认 EPERM=1）
    Errno(i32),
}

impl Default for SeccompAction {
    fn default() -> Self {
        SeccompAction::Errno(EPERM)
    }
}

/// 系统调用参数比较器（平台无关表示）
///
/// 用于条件规则：当 `(arg & mask) == datum` 时匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgComparator {
    /// 参数索引（从 0 开始）
    pub arg_index: u32,
    /// 位掩码
    pub mask: u64,
    /// 比较值
    pub datum: u64,
}

impl ArgComparator {
    pub fn new(arg_index: u32, mask: u64, datum: u64) -> Self {
        Self {
            arg_index,
            mask,
            datum,
        }
    }
}

/// seccomp 规则
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeccompRule {
    /// 系统调用名称（如 "ptrace"）
    pub syscall_name: String,
    /// 匹配时执行的动作
    pub action: SeccompAction,
    /// 参数比较器（空 = 无条件规则）
    pub comparators: Vec<ArgComparator>,
}

impl SeccompRule {
    /// 创建无条件规则
    pub fn new(syscall_name: &str, action: SeccompAction) -> Self {
        Self {
            syscall_name: syscall_name.to_string(),
            action,
            comparators: Vec::new(),
        }
    }

    /// 添加参数比较器，将规则转为条件规则
    pub fn with_comparator(mut self, cmp: ArgComparator) -> Self {
        self.comparators.push(cmp);
        self
    }
}

/// seccomp profile — 一组规则 + 所属权限层级
#[derive(Debug, Clone)]
pub struct SeccompProfile {
    /// 该 profile 对应的权限层级
    pub authority_level: AuthorityLevel,
    /// 规则列表
    pub rules: Vec<SeccompRule>,
}

impl SeccompProfile {
    /// 按权限层级生成 seccomp profile
    pub fn new(authority_level: AuthorityLevel) -> Self {
        let rules = match authority_level {
            AuthorityLevel::Observer => observer_profile(),
            AuthorityLevel::Operator => operator_profile(),
            AuthorityLevel::Supervisor => supervisor_profile(),
            AuthorityLevel::Emergency => emergency_profile(),
        };
        Self {
            authority_level,
            rules,
        }
    }
}

// ============================================================
// 4 个 profile 生成函数（平台无关，使用 Linux ABI 常量）
// ============================================================

/// Observer profile：最严格
///
/// - 禁止 `open`/`openat` 的 O_WRONLY 标志位（写文件）
/// - 禁止 `socket(AF_PACKET)`、`mount`、`reboot`、`kexec_load`、`init_module`、`finit_module`、`ptrace`
fn observer_profile() -> Vec<SeccompRule> {
    let eperm = SeccompAction::Errno(EPERM);
    vec![
        // open: arg1(flags) 的 O_WRONLY 位被设置时拒绝
        SeccompRule::new("open", eperm.clone())
            .with_comparator(ArgComparator::new(1, O_WRONLY, O_WRONLY)),
        // openat: arg2(flags) 的 O_WRONLY 位被设置时拒绝
        SeccompRule::new("openat", eperm.clone())
            .with_comparator(ArgComparator::new(2, O_WRONLY, O_WRONLY)),
        // socket(AF_PACKET=17)：arg0(domain) 等于 AF_PACKET 时拒绝
        SeccompRule::new("socket", eperm.clone())
            .with_comparator(ArgComparator::new(0, AF_PACKET, AF_PACKET)),
        SeccompRule::new("mount", eperm.clone()),
        SeccompRule::new("reboot", eperm.clone()),
        SeccompRule::new("kexec_load", eperm.clone()),
        SeccompRule::new("init_module", eperm.clone()),
        SeccompRule::new("finit_module", eperm.clone()),
        SeccompRule::new("ptrace", eperm),
    ]
}

/// Operator profile
///
/// - 禁止 `socket(AF_PACKET)`、`mount`、`reboot`、`kexec_load`、`init_module`、`finit_module`、`ptrace`
fn operator_profile() -> Vec<SeccompRule> {
    let eperm = SeccompAction::Errno(EPERM);
    vec![
        SeccompRule::new("socket", eperm.clone())
            .with_comparator(ArgComparator::new(0, AF_PACKET, AF_PACKET)),
        SeccompRule::new("mount", eperm.clone()),
        SeccompRule::new("reboot", eperm.clone()),
        SeccompRule::new("kexec_load", eperm.clone()),
        SeccompRule::new("init_module", eperm.clone()),
        SeccompRule::new("finit_module", eperm.clone()),
        SeccompRule::new("ptrace", eperm),
    ]
}

/// Supervisor profile
///
/// - 禁止 `kexec_load`、`init_module`、`finit_module`、`ptrace`
fn supervisor_profile() -> Vec<SeccompRule> {
    let eperm = SeccompAction::Errno(EPERM);
    vec![
        SeccompRule::new("kexec_load", eperm.clone()),
        SeccompRule::new("init_module", eperm.clone()),
        SeccompRule::new("finit_module", eperm.clone()),
        SeccompRule::new("ptrace", eperm),
    ]
}

/// Emergency profile：仅禁止 `ptrace`
fn emergency_profile() -> Vec<SeccompRule> {
    vec![SeccompRule::new("ptrace", SeccompAction::Errno(EPERM))]
}

// ============================================================
// Linux + seccomp feature：真实 BPF 过滤器实现
// ============================================================
#[cfg(all(target_os = "linux", feature = "seccomp"))]
mod linux_impl {
    use super::*;
    use libseccomp::{ScmpAction, ScmpArgCompare, ScmpCompareOp, ScmpFilterContext, ScmpSyscall};

    impl SeccompProfile {
        /// 转换为 libseccomp 过滤器上下文
        ///
        /// 默认动作：Allow；匹配规则的动作为 `Errno(EPERM)`。
        /// 对于当前架构不存在的系统调用（如 aarch64 上的 `open`），跳过该规则。
        pub fn to_filter(&self) -> Result<ScmpFilterContext, SeccompError> {
            let mut ctx = ScmpFilterContext::new(ScmpAction::Allow)
                .map_err(|e| SeccompError::Libseccomp(e.to_string()))?;

            for rule in &self.rules {
                let syscall = match ScmpSyscall::from_name(&rule.syscall_name) {
                    Ok(s) => s,
                    Err(_) => {
                        // 该架构上不存在此系统调用，无需过滤
                        continue;
                    }
                };
                let action = match rule.action {
                    SeccompAction::Allow => ScmpAction::Allow,
                    SeccompAction::Errno(errno) => ScmpAction::Errno(errno),
                };
                if rule.comparators.is_empty() {
                    ctx.add_rule(action, syscall)
                        .map_err(|e| SeccompError::Libseccomp(e.to_string()))?;
                } else {
                    let cmps: Vec<ScmpArgCompare> = rule
                        .comparators
                        .iter()
                        .map(|c| {
                            ScmpArgCompare::new(
                                c.arg_index,
                                ScmpCompareOp::MaskedEqual(c.mask),
                                c.datum,
                            )
                        })
                        .collect();
                    ctx.add_rule_conditional(action, syscall, &cmps)
                        .map_err(|e| SeccompError::Libseccomp(e.to_string()))?;
                }
            }
            Ok(ctx)
        }
    }

    /// 加载 BPF 过滤器到当前进程
    ///
    /// 在 Agent spawn 的 `pre_exec` 钩子中调用。libseccomp 的 `seccomp_load()`
    /// 是 async-signal-safe 的，可在 fork 后多线程环境使用。
    pub fn apply_seccomp(profile: &SeccompProfile) -> Result<(), SeccompError> {
        let ctx = profile.to_filter()?;
        ctx.load().map_err(|e| SeccompError::Libseccomp(e.to_string()))
    }
}

// ============================================================
// 非 Linux 或无 seccomp feature：stub 实现
// ============================================================
#[cfg(not(all(target_os = "linux", feature = "seccomp")))]
mod stub_impl {
    use super::*;

    impl SeccompProfile {
        /// stub：返回 Unsupported（无 libseccomp 可用）
        pub fn to_filter(&self) -> Result<(), SeccompError> {
            Err(SeccompError::Unsupported)
        }
    }

    /// stub：返回 Unsupported
    #[allow(dead_code)]
    pub fn apply_seccomp(_profile: &SeccompProfile) -> Result<(), SeccompError> {
        Err(SeccompError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：检查 profile 是否包含指定名称的无条件规则
    fn has_rule(profile: &SeccompProfile, name: &str) -> bool {
        profile.rules.iter().any(|r| r.syscall_name == name)
    }

    #[test]
    fn test_seccomp_profile_new() {
        let obs = SeccompProfile::new(AuthorityLevel::Observer);
        assert_eq!(obs.authority_level, AuthorityLevel::Observer);
        assert!(!obs.rules.is_empty());

        let sup = SeccompProfile::new(AuthorityLevel::Supervisor);
        assert_eq!(sup.authority_level, AuthorityLevel::Supervisor);
        assert!(!sup.rules.is_empty());

        // 权限越高，规则越少（限制越宽松）
        assert!(obs.rules.len() > sup.rules.len());
    }

    #[test]
    fn test_observer_profile_rules() {
        let profile = SeccompProfile::new(AuthorityLevel::Observer);
        // 必须包含这些高风险系统调用的禁止规则
        assert!(has_rule(&profile, "ptrace"));
        assert!(has_rule(&profile, "mount"));
        assert!(has_rule(&profile, "reboot"));
        assert!(has_rule(&profile, "kexec_load"));
        assert!(has_rule(&profile, "init_module"));
        assert!(has_rule(&profile, "finit_module"));
        // Observer 特有：open/openat 写限制 + socket(AF_PACKET)
        assert!(has_rule(&profile, "open"));
        assert!(has_rule(&profile, "openat"));
        assert!(has_rule(&profile, "socket"));

        // 验证 open 规则带 O_WRONLY 比较器
        let open_rule = profile
            .rules
            .iter()
            .find(|r| r.syscall_name == "open")
            .unwrap();
        assert!(!open_rule.comparators.is_empty());
        let cmp = &open_rule.comparators[0];
        assert_eq!(cmp.arg_index, 1);
        assert_eq!(cmp.mask, O_WRONLY);
        assert_eq!(cmp.datum, O_WRONLY);
    }

    #[test]
    fn test_operator_profile_rules() {
        let profile = SeccompProfile::new(AuthorityLevel::Operator);
        // 包含 ptrace
        assert!(has_rule(&profile, "ptrace"));
        assert!(has_rule(&profile, "mount"));
        assert!(has_rule(&profile, "reboot"));
        // 不包含 open 限制（Operator 可写文件）
        assert!(!has_rule(&profile, "open"));
        assert!(!has_rule(&profile, "openat"));
    }

    #[test]
    fn test_supervisor_profile_rules() {
        let profile = SeccompProfile::new(AuthorityLevel::Supervisor);
        // 仅禁止 kexec/init_module/finit_module/ptrace
        assert!(has_rule(&profile, "kexec_load"));
        assert!(has_rule(&profile, "init_module"));
        assert!(has_rule(&profile, "finit_module"));
        assert!(has_rule(&profile, "ptrace"));
        // 不禁止 mount/reboot/open
        assert!(!has_rule(&profile, "mount"));
        assert!(!has_rule(&profile, "reboot"));
        assert!(!has_rule(&profile, "open"));
        assert_eq!(profile.rules.len(), 4);
    }

    #[test]
    fn test_emergency_profile_rules() {
        let profile = SeccompProfile::new(AuthorityLevel::Emergency);
        // 仅禁止 ptrace
        assert_eq!(profile.rules.len(), 1);
        assert!(has_rule(&profile, "ptrace"));
        assert!(!has_rule(&profile, "mount"));
        assert!(!has_rule(&profile, "kexec_load"));
    }

    /// 非 Linux 或无 seccomp feature：apply_seccomp 返回 Unsupported
    #[cfg(not(all(target_os = "linux", feature = "seccomp")))]
    #[test]
    fn test_apply_seccomp_unsupported() {
        let profile = SeccompProfile::new(AuthorityLevel::Observer);
        let result = stub_impl::apply_seccomp(&profile);
        assert!(matches!(result, Err(SeccompError::Unsupported)));

        // to_filter 同样返回 Unsupported
        let filter_result = profile.to_filter();
        assert!(matches!(filter_result, Err(SeccompError::Unsupported)));
    }

    #[test]
    fn test_seccomp_action_default() {
        let action = SeccompAction::default();
        assert!(matches!(action, SeccompAction::Errno(EPERM)));
    }

    #[test]
    fn test_seccomp_rule_with_comparator() {
        let rule = SeccompRule::new("open", SeccompAction::Errno(EPERM))
            .with_comparator(ArgComparator::new(1, O_WRONLY, O_WRONLY));
        assert_eq!(rule.syscall_name, "open");
        assert_eq!(rule.comparators.len(), 1);
        assert_eq!(rule.comparators[0].arg_index, 1);
    }
}
