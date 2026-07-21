//! IEC 104 从站配置.

/// IEC 104 从站配置
///
/// 包含公共地址、监听端口、三个超时（t1/t2/t3）与流控参数（k/w）。
/// 超时使用 `u32` 毫秒（D5：无 `Duration` 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iec104Config {
    /// 公共地址（ASDU 地址），标识本站
    pub common_addr: u16,
    /// 监听端口（IEC 104 标准端口 2404）
    pub listen_port: u16,
    /// t1 超时：发送 U 格式后等待确认的超时（默认 15000ms）
    pub t1_timeout_ms: u32,
    /// t2 超时：确认超时，必须 < t1（默认 10000ms）
    pub t2_timeout_ms: u32,
    /// t3 超时：空闲时发送测试帧的超时（默认 20000ms）
    pub t3_timeout_ms: u32,
    /// k：未确认 I 帧最大数（默认 12）
    pub k: u16,
    /// w：最迟确认 I 帧数，收到 w 个 I 帧后必须发 S 帧（默认 8）
    pub w: u16,
}

impl Default for Iec104Config {
    fn default() -> Self {
        Self {
            common_addr: 1,
            listen_port: 2404,
            t1_timeout_ms: 15000,
            t2_timeout_ms: 10000,
            t3_timeout_ms: 20000,
            k: 12,
            w: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = Iec104Config::default();
        assert_eq!(cfg.common_addr, 1);
        assert_eq!(cfg.listen_port, 2404);
        assert_eq!(cfg.t1_timeout_ms, 15000);
        assert_eq!(cfg.t2_timeout_ms, 10000);
        assert_eq!(cfg.t3_timeout_ms, 20000);
        assert_eq!(cfg.k, 12);
        assert_eq!(cfg.w, 8);
    }

    #[test]
    fn test_t2_less_than_t1() {
        // 蓝图 §8.4 坑点：t2 必须 < t1
        let cfg = Iec104Config::default();
        assert!(cfg.t2_timeout_ms < cfg.t1_timeout_ms);
    }

    #[test]
    fn test_field_access_and_modify() {
        let cfg = Iec104Config {
            common_addr: 2,
            listen_port: 2405,
            t3_timeout_ms: 30000,
            ..Default::default()
        };
        assert_eq!(cfg.common_addr, 2);
        assert_eq!(cfg.listen_port, 2405);
        assert_eq!(cfg.t3_timeout_ms, 30000);
    }

    #[test]
    fn test_clone_eq() {
        let cfg = Iec104Config::default();
        let cloned = cfg.clone();
        assert_eq!(cfg, cloned);
    }
}
