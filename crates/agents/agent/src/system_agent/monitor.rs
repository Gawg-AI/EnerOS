//! 资源监控层 — ResourceMonitor / ResourceSource / SystemConfig / SystemStats / SystemEvent / AgentResourceUsage
//!
//! # 设计
//! - ResourceMonitor 作为 SystemAgent 的资源监控层
//! - D2: ResourceSource trait 抽象数据源（agent crate 不依赖 HAL crate）
//! - D3: ResourceMonitor 不维护 agent_stats（由 registry 提供，避免冗余）
//! - D7: SystemConfig 配置阈值（蓝图未定义但必需）
//! - D8: is_oom + find_oom_victim 职责分离（监控器只判阈值，victim 选择需访问 registry）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` 与 `core::*`，子模块不重复 `#![cfg_attr(not(test), no_std)]`。

use alloc::boxed::Box;

use crate::id::AgentId;
use crate::types::AgentState;

/// 资源数据源 trait（D2 偏差：抽象 HAL 接口）.
///
/// agent crate 不直接依赖 HAL crate；调用方提供实现。
pub trait ResourceSource {
    /// CPU 使用率（0.0~1.0）
    fn cpu_usage(&self) -> f32;
    /// 已用内存（字节）
    fn mem_used(&self) -> usize;
    /// 总内存（字节）
    fn mem_total(&self) -> usize;
    /// 温度（摄氏度）
    fn temperature(&self) -> f32;
}

/// 资源监控器.
///
/// 持有最新资源快照，可选通过 `ResourceSource` 自动 poll。
pub struct ResourceMonitor {
    /// CPU 使用率（0.0~1.0）
    pub cpu_usage: f32,
    /// 总内存（字节）
    pub mem_total: usize,
    /// 已用内存（字节）
    pub mem_used: usize,
    /// 温度（摄氏度）
    pub temperature: f32,
    source: Option<Box<dyn ResourceSource>>,
}

impl ResourceMonitor {
    /// 创建空监控器（source=None，所有值为 0）.
    pub fn new() -> Self {
        ResourceMonitor {
            cpu_usage: 0.0,
            mem_total: 0,
            mem_used: 0,
            temperature: 0.0,
            source: None,
        }
    }

    /// 创建带数据源的监控器.
    pub fn with_source(source: Box<dyn ResourceSource>) -> Self {
        ResourceMonitor {
            cpu_usage: 0.0,
            mem_total: 0,
            mem_used: 0,
            temperature: 0.0,
            source: Some(source),
        }
    }

    /// 从 source 读取最新值（若无 source 则 no-op）.
    pub fn poll(&mut self) {
        if let Some(src) = &self.source {
            self.cpu_usage = src.cpu_usage();
            self.mem_used = src.mem_used();
            self.mem_total = src.mem_total();
            self.temperature = src.temperature();
        }
    }

    /// 手动设置值（测试用）.
    pub fn set_values(&mut self, cpu: f32, mem_used: usize, mem_total: usize, temp: f32) {
        self.cpu_usage = cpu;
        self.mem_used = mem_used;
        self.mem_total = mem_total;
        self.temperature = temp;
    }

    /// 判断 OOM（mem_total==0 时返回 false）.
    pub fn is_oom(&self, threshold: f32) -> bool {
        if self.mem_total == 0 {
            return false;
        }
        let ratio = self.mem_used as f32 / self.mem_total as f32;
        ratio > threshold
    }

    /// 判断过热.
    pub fn is_overheat(&self, threshold: f32) -> bool {
        self.temperature > threshold
    }

    /// 内存使用率（mem_total==0 时返回 0.0）.
    pub fn mem_usage_percent(&self) -> f32 {
        if self.mem_total == 0 {
            return 0.0;
        }
        self.mem_used as f32 / self.mem_total as f32
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// 系统配置（D7 偏差）.
pub struct SystemConfig {
    /// OOM 阈值（内存使用率，默认 0.9）
    pub oom_threshold_percent: f32,
    /// 过热阈值（摄氏度，默认 80.0）
    pub overheat_threshold: f32,
    /// 监控周期（毫秒，默认 1000）
    pub monitor_interval_ms: u64,
}

impl Default for SystemConfig {
    fn default() -> Self {
        SystemConfig {
            oom_threshold_percent: 0.9,
            overheat_threshold: 80.0,
            monitor_interval_ms: 1000,
        }
    }
}

/// 系统级统计.
pub struct SystemStats {
    /// CPU 使用率
    pub cpu_usage: f32,
    /// 内存使用率
    pub mem_usage: f32,
    /// 温度
    pub temperature: f32,
    /// 总 Agent 数
    pub agent_count: usize,
    /// 存活 Agent 数
    pub alive_agents: usize,
    /// 错误状态 Agent 数
    pub error_agents: usize,
}

/// 系统事件（D6 偏差：返回事件列表替代 log）.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemEvent {
    /// 过热
    Overheat { temp: f32 },
    /// OOM victim 被挂起
    OomVictimSuspended { agent: AgentId },
    /// Agent 崩溃
    AgentCrashed { agent: AgentId },
    /// Agent 恢复成功
    AgentRecovered { agent: AgentId },
    /// Agent 恢复失败
    AgentRecoveryFailed { agent: AgentId },
}

/// Agent 资源使用情况（蓝图 §4.1）.
pub struct AgentResourceUsage {
    /// CPU 使用率（0~100）
    pub cpu_percent: f32,
    /// 内存占用（字节）
    pub mem_bytes: usize,
    /// Agent 状态
    pub state: AgentState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_new_empty() {
        let m = ResourceMonitor::new();
        assert_eq!(m.cpu_usage, 0.0);
        assert_eq!(m.mem_total, 0);
        assert_eq!(m.mem_used, 0);
        assert_eq!(m.temperature, 0.0);
    }

    #[test]
    fn test_monitor_set_values() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 100, 200, 65.0);
        assert_eq!(m.cpu_usage, 0.5);
        assert_eq!(m.mem_used, 100);
        assert_eq!(m.mem_total, 200);
        assert_eq!(m.temperature, 65.0);
    }

    #[test]
    fn test_monitor_is_oom_true() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 95, 100, 50.0); // 95% > 0.9
        assert!(m.is_oom(0.9));
    }

    #[test]
    fn test_monitor_is_oom_false() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 80, 100, 50.0); // 80% < 0.9
        assert!(!m.is_oom(0.9));
    }

    #[test]
    fn test_monitor_is_oom_zero_total() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 0, 0, 50.0); // mem_total=0
        assert!(!m.is_oom(0.9));
    }

    #[test]
    fn test_monitor_is_overheat_true() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 50, 100, 85.0); // 85 > 80
        assert!(m.is_overheat(80.0));
    }

    #[test]
    fn test_monitor_is_overheat_false() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 50, 100, 75.0); // 75 < 80
        assert!(!m.is_overheat(80.0));
    }

    #[test]
    fn test_monitor_mem_usage_percent() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 50, 200, 50.0);
        assert_eq!(m.mem_usage_percent(), 0.25);

        let mut m2 = ResourceMonitor::new();
        m2.set_values(0.5, 0, 0, 50.0); // mem_total=0
        assert_eq!(m2.mem_usage_percent(), 0.0);
    }

    /// Mock ResourceSource for testing.
    struct MockSource {
        cpu: f32,
        mem_used: usize,
        mem_total: usize,
        temp: f32,
    }

    impl ResourceSource for MockSource {
        fn cpu_usage(&self) -> f32 {
            self.cpu
        }
        fn mem_used(&self) -> usize {
            self.mem_used
        }
        fn mem_total(&self) -> usize {
            self.mem_total
        }
        fn temperature(&self) -> f32 {
            self.temp
        }
    }

    #[test]
    fn test_monitor_with_source_poll() {
        let source = Box::new(MockSource {
            cpu: 0.42,
            mem_used: 1024,
            mem_total: 4096,
            temp: 72.5,
        });
        let mut m = ResourceMonitor::with_source(source);
        assert_eq!(m.cpu_usage, 0.0); // before poll
        m.poll();
        assert_eq!(m.cpu_usage, 0.42);
        assert_eq!(m.mem_used, 1024);
        assert_eq!(m.mem_total, 4096);
        assert_eq!(m.temperature, 72.5);
    }

    #[test]
    fn test_monitor_poll_no_source_is_noop() {
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 100, 200, 65.0);
        m.poll(); // no source — should not change values
        assert_eq!(m.cpu_usage, 0.5);
        assert_eq!(m.mem_used, 100);
        assert_eq!(m.mem_total, 200);
        assert_eq!(m.temperature, 65.0);
    }

    #[test]
    fn test_system_config_default() {
        let cfg = SystemConfig::default();
        assert_eq!(cfg.oom_threshold_percent, 0.9);
        assert_eq!(cfg.overheat_threshold, 80.0);
        assert_eq!(cfg.monitor_interval_ms, 1000);
    }

    #[test]
    fn test_monitor_default_impl() {
        let m = ResourceMonitor::default();
        assert_eq!(m.cpu_usage, 0.0);
        assert_eq!(m.mem_total, 0);
    }

    #[test]
    fn test_system_event_variants() {
        let e1 = SystemEvent::Overheat { temp: 85.0 };
        let e2 = SystemEvent::Overheat { temp: 85.0 };
        assert_eq!(e1, e2);

        let e3 = SystemEvent::Overheat { temp: 90.0 };
        assert_ne!(e1, e3);

        let _e4 = SystemEvent::OomVictimSuspended {
            agent: AgentId::ZERO,
        };
        let _e5 = SystemEvent::AgentCrashed {
            agent: AgentId::ZERO,
        };
        let _e6 = SystemEvent::AgentRecovered {
            agent: AgentId::ZERO,
        };
        let _e7 = SystemEvent::AgentRecoveryFailed {
            agent: AgentId::ZERO,
        };
    }

    #[test]
    fn test_is_oom_boundary_at_threshold() {
        // threshold is strict > (not >=)
        let mut m = ResourceMonitor::new();
        m.set_values(0.5, 90, 100, 50.0); // exactly 0.9
        assert!(!m.is_oom(0.9)); // 0.9 > 0.9 is false
    }
}
