//! IEC 104 主站配置.
//!
//! D3：超时/间隔使用 `u32` 毫秒（无 `Duration` 类型，与 v0.48.0 D5 一致）。

/// IEC 104 主站配置
///
/// 包含时钟同步周期、t3 保活超时、默认轮询周期与端口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterConfig {
    /// 时钟同步周期（毫秒，默认 600000 = 10 分钟）
    pub clock_sync_interval_ms: u32,
    /// t3 保活超时（毫秒，默认 20000）
    pub t3_timeout_ms: u32,
    /// 默认轮询周期（毫秒，默认 30000）
    pub poll_interval_ms: u32,
    /// 默认端口（IEC 104 标准端口 2404）
    pub default_port: u16,
    /// 31 PR-14：对端地址（装配层从 configs/*.toml 注入——生产路径禁止
    /// 字面量 IP）。缺省 `[0,0,0,0]` = 未注入哨兵，装配未注入即
    /// 连接失败（fail-closed，31 DoD grep 判据）。
    pub peer_ip: [u8; 4],
    /// 31 PR-14：对端端口。
    pub peer_port: u16,
}

impl Default for MasterConfig {
    fn default() -> Self {
        Self {
            clock_sync_interval_ms: 600_000,
            t3_timeout_ms: 20_000,
            poll_interval_ms: 30_000,
            default_port: 2404,
            peer_ip: [0, 0, 0, 0], // 未注入哨兵——生产必须经装配注入（PR-14）
            peer_port: 2404,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = MasterConfig::default();
        assert_eq!(cfg.clock_sync_interval_ms, 600_000);
        assert_eq!(cfg.t3_timeout_ms, 20_000);
        assert_eq!(cfg.poll_interval_ms, 30_000);
        assert_eq!(cfg.default_port, 2404);
        // PR-14：缺省对端地址为未注入哨兵（禁止字面量兜底 IP）。
        assert_eq!(cfg.peer_ip, [0, 0, 0, 0]);
        assert_eq!(cfg.peer_port, 2404);
    }

    #[test]
    fn test_copy_eq() {
        let cfg = MasterConfig::default();
        let copied = cfg;
        assert_eq!(cfg, copied);
    }

    #[test]
    fn test_modify() {
        let cfg = MasterConfig {
            clock_sync_interval_ms: 300_000,
            t3_timeout_ms: 15_000,
            ..Default::default()
        };
        assert_eq!(cfg.clock_sync_interval_ms, 300_000);
        assert_eq!(cfg.t3_timeout_ms, 15_000);
        assert_eq!(cfg.poll_interval_ms, 30_000);
    }
}
