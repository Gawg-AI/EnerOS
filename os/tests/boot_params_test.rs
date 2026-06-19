//! EnerOS boot parameter verification tests
//!
//! Tests the RT kernel boot parameter parsing and verification logic:
//! - `parse_cmdline()`: extracts parameter values from a /proc/cmdline string
//! - `check_rt_kernel()`: reads /sys/kernel/realtime to confirm PREEMPT_RT
//!
//! Linux-only tests are cfg-gated so the file compiles on all platforms.

/// Parse a kernel cmdline string and extract the value for `key`.
///
/// Returns `Some(value)` when `key=value` is present as a whole token
/// (e.g., `isolcpus=2,3` -> `Some("2,3")`), or `None` if absent.
fn parse_cmdline<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    for token in cmdline.split_whitespace() {
        if let Some(rest) = token.strip_prefix(key) {
            if let Some(value) = rest.strip_prefix('=') {
                return Some(value);
            }
        }
    }
    None
}

/// Check whether the running kernel is PREEMPT_RT.
///
/// Reads `/sys/kernel/realtime` and returns `true` if it contains `1`.
#[cfg(target_os = "linux")]
fn check_rt_kernel() -> bool {
    std::fs::read_to_string("/sys/kernel/realtime")
        .map(|content| content.trim() == "1")
        .unwrap_or(false)
}

/// Parsing `isolcpus=2,3` from a full cmdline string returns the value.
#[test]
fn test_parse_cmdline_extracts_isolcpus() {
    let cmdline = "root=/dev/sda2 ro isolcpus=2,3 nohz_full=2,3 rcu_nocbs=2,3";
    assert_eq!(parse_cmdline(cmdline, "isolcpus"), Some("2,3"));
}

/// A cmdline without the requested parameter returns `None`.
#[test]
fn test_parse_cmdline_missing_param() {
    let cmdline = "root=/dev/sda2 ro console=ttyS0,115200";
    assert_eq!(parse_cmdline(cmdline, "isolcpus"), None);
}

/// `check_rt_kernel()` returns a bool on Linux without panicking.
#[cfg(target_os = "linux")]
#[test]
fn test_check_rt_kernel_returns_bool() {
    let _: bool = check_rt_kernel();
}
