//! 22 §2.W1.3：EL0 服务 supervisor——故障状态机 + 指数退避重启 + 防抖。
//!
//! 语义（锁定 6 分级重启）：服务异常（seL4 fault）→ root-task 在 fault ep
//! 轮询到通知 → 按服务识别 → supervisor 决策：
//! - 首次故障：重启该服务（指数退避 1s/2s/4s/8s/16s/32s/60s 封顶）；
//! - 连续故障超阈值（5 次/小时）：标记 Faulted + 告警 + 不再自动重启
//!   （防抖，防"坏服务无限重启拖垮 CPU"）；
//! - 其他服务不受影响（隔离由 seL4 地址空间保证，supervisor 只决策）。
//!
//! 重启动作（[`supervisor::Action::Restart`] 由调用方执行）：fault 后 TCB
//! 被内核挂起且对象保留，`tcb_write_registers` 重置 PC/SP/参数 +
//! `tcb_resume` 即从 `_start` 重新执行（无需 revoke/retype 对象）。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// 服务运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// 正常运行（或等待重启执行）。
    Running,
    /// 故障后等待退避重启。
    Suspended,
    /// 连续故障超阈值，不再自动重启（人工介入）。
    Faulted,
}

/// supervisor 对一次故障的决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 重启该服务（`delay_ms` 退避后执行；调用方负责忙等/计时）。
    Restart { delay_ms: u64 },
    /// 标记 Faulted + 告警（不再自动重启）。
    MarkFaulted,
}

/// 指数退避序列（W1.3：1s/2s/4s/8s/16s/32s/60s 封顶）。
pub const BACKOFF_MS: [u64; 7] = [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000];

/// 连续故障阈值（次/小时）：超过即 Faulted。
pub const FAULT_THRESHOLD_PER_HOUR: u64 = 5;

/// 滑动窗口（小时，毫秒）。
pub const WINDOW_MS: u64 = 3_600_000;

/// 单服务 supervisor 状态。
#[derive(Debug, Clone)]
pub struct ServiceSupervisor {
    /// 服务名（日志/告警标识）。
    pub name: String,
    /// 当前状态。
    pub state: ServiceState,
    /// 窗口内故障次数。
    pub fault_count: u64,
    /// 窗口起点（毫秒）。
    pub window_start_ms: u64,
    /// 上次故障时刻（毫秒；Suspended 期间推进退避）。
    pub last_fault_ms: u64,
    /// 下次退避档位索引（BACKOFF_MS 下标）。
    pub backoff_idx: usize,
    /// 重启总次数（指标）。
    pub restart_count: u64,
}

impl ServiceSupervisor {
    /// 新建（Running，空窗口）。
    pub fn new(name: &str, now_ms: u64) -> Self {
        Self {
            name: String::from(name),
            state: ServiceState::Running,
            fault_count: 0,
            window_start_ms: now_ms,
            last_fault_ms: now_ms,
            backoff_idx: 0,
            restart_count: 0,
        }
    }

    /// 记录一次故障并返回决策（[`Action`]）。
    ///
    /// - Faulted 状态：返回 `MarkFaulted`（已熔断，不再变化）；
    /// - 窗口滑动：`now - window_start > WINDOW_MS` 时重置计数与退避；
    /// - 计数超阈值：`MarkFaulted`；
    /// - 否则：`Restart { delay_ms: BACKOFF_MS[backoff_idx] }`，
    ///   状态置 Suspended，退避档位推进。
    pub fn on_fault(&mut self, now_ms: u64) -> Action {
        // 已熔断：不再自动重启。
        if self.state == ServiceState::Faulted {
            return Action::MarkFaulted;
        }
        // 滑动窗口：超时重置（计数与退避归零，窗口重开）。
        if now_ms.saturating_sub(self.window_start_ms) > WINDOW_MS {
            self.fault_count = 0;
            self.backoff_idx = 0;
            self.window_start_ms = now_ms;
        }
        self.fault_count += 1;
        self.last_fault_ms = now_ms;
        if self.fault_count > FAULT_THRESHOLD_PER_HOUR {
            self.state = ServiceState::Faulted;
            return Action::MarkFaulted;
        }
        // 退避档位（封顶 60s）。
        let delay_ms = BACKOFF_MS[self.backoff_idx.min(BACKOFF_MS.len() - 1)];
        if self.backoff_idx < BACKOFF_MS.len() - 1 {
            self.backoff_idx += 1;
        }
        self.state = ServiceState::Suspended;
        Action::Restart { delay_ms }
    }

    /// 重启执行完成（退避等待后调用）：状态回 Running，重启计数 +1。
    ///
    /// 仅 `Suspended` 状态生效（Faulted 熔断后不可被 restart_done 唤醒）。
    pub fn restart_done(&mut self) {
        if self.state == ServiceState::Suspended {
            self.state = ServiceState::Running;
            self.restart_count += 1;
        }
    }

    /// 人工恢复（Faulted → Running，清窗口；运维通道后续接入）。
    pub fn recover(&mut self, now_ms: u64) {
        self.state = ServiceState::Running;
        self.fault_count = 0;
        self.backoff_idx = 0;
        self.window_start_ms = now_ms;
    }
}

/// 多服务 supervisor 表（按 spawn 顺序索引）。
#[derive(Debug, Clone)]
pub struct Supervisor {
    /// 每服务一个状态机。
    pub services: Vec<ServiceSupervisor>,
    /// 当前时间源（毫秒；由调用方注入——root-task 无墙钟，用 tick 折算）。
    now_ms: u64,
}

impl Supervisor {
    /// 新建（`names` 与 spawn 计划顺序一致）。
    pub fn new(names: &[&str], now_ms: u64) -> Self {
        Self {
            services: names
                .iter()
                .map(|n| ServiceSupervisor::new(n, now_ms))
                .collect(),
            now_ms,
        }
    }

    /// 推进时间（root-task 主循环每轮调用；退避计时依赖）。
    pub fn advance(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// 按服务数新建（root-task 侧：spawn 计划动态，名字占位 `svc-<i>`）。
    pub fn new_for_count(count: usize, now_ms: u64) -> Self {
        Self {
            services: (0..count)
                .map(|i| ServiceSupervisor::new(&alloc::format!("svc-{i}"), now_ms))
                .collect(),
            now_ms,
        }
    }

    /// 服务故障入口：识别服务（`index` = spawn 顺序）→ 决策。
    pub fn on_service_fault(&mut self, index: usize) -> Option<Action> {
        let svc = self.services.get_mut(index)?;
        Some(svc.on_fault(self.now_ms))
    }

    /// 重启完成登记。
    pub fn on_restart_done(&mut self, index: usize) {
        if let Some(svc) = self.services.get_mut(index) {
            svc.restart_done();
        }
    }

    /// 查询服务状态。
    pub fn state_of(&self, index: usize) -> Option<ServiceState> {
        self.services.get(index).map(|s| s.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首次故障 → Restart{1s}，状态 Suspended。
    #[test]
    fn test_first_fault_restarts_with_1s() {
        let mut svc = ServiceSupervisor::new("svc-hello", 0);
        let a = svc.on_fault(10);
        assert_eq!(a, Action::Restart { delay_ms: 1_000 });
        assert_eq!(svc.state, ServiceState::Suspended);
        assert_eq!(svc.fault_count, 1);
    }

    /// 指数退避：1s→2s→4s→8s→16s，随后第 6 次故障超阈值熔断（阈值先于
    /// 60s 封顶档生效——5 次/小时上限约束重启总次数）。
    #[test]
    fn test_backoff_escalates_then_caps() {
        let mut svc = ServiceSupervisor::new("svc-x", 0);
        let mut delays = Vec::new();
        for t in 1..=6u64 {
            svc.restart_done(); // 模拟每次重启成功后再故障
            if let Action::Restart { delay_ms } = svc.on_fault(t * 1000) {
                delays.push(delay_ms);
            }
        }
        assert_eq!(
            delays,
            vec![1_000, 2_000, 4_000, 8_000, 16_000],
            "退避必须指数递增（阈值 5 次先于 60s 封顶档生效）"
        );
        assert_eq!(svc.state, ServiceState::Faulted);
    }

    /// 阈值：第 6 次故障（>5/小时）→ MarkFaulted，且后续不再变化。
    #[test]
    fn test_threshold_marks_faulted_and_sticky() {
        let mut svc = ServiceSupervisor::new("svc-fault", 0);
        let mut final_action = Action::Restart { delay_ms: 0 };
        for t in 1..=7u64 {
            svc.restart_done();
            final_action = svc.on_fault(t * 100);
        }
        assert_eq!(final_action, Action::MarkFaulted);
        assert_eq!(svc.state, ServiceState::Faulted);
        // 熔断后无论再故障多少次都不再重启。
        let again = svc.on_fault(99_999);
        assert_eq!(again, Action::MarkFaulted);
        assert_eq!(svc.state, ServiceState::Faulted);
        // restart_done 在 Faulted 下无效（熔断不可被唤醒）。
        svc.restart_done();
        assert_eq!(svc.state, ServiceState::Faulted);
        assert!(svc.restart_count <= 5, "重启次数必须被阈值约束");
    }

    /// 滑动窗口：超过 1 小时后计数重置（连续故障不跨窗口累计）。
    #[test]
    fn test_window_slides_after_hour() {
        let mut svc = ServiceSupervisor::new("svc-x", 0);
        // 1 小时内 5 次 → 第 6 次 Faulted。
        for t in 1..=5u64 {
            svc.restart_done();
            svc.on_fault(t * 100);
        }
        assert_eq!(svc.state, ServiceState::Suspended);
        // 超过窗口（1 小时）后故障：计数重置，退避回到 1s，仍可重启。
        let a = svc.on_fault(WINDOW_MS + 1_000);
        assert_eq!(a, Action::Restart { delay_ms: 1_000 });
        assert_eq!(svc.fault_count, 1);
        assert_eq!(svc.backoff_idx, 1);
    }

    /// recover 复位（Faulted → Running，清窗口）。
    #[test]
    fn test_recover_clears_faulted() {
        let mut svc = ServiceSupervisor::new("svc-x", 0);
        for t in 1..=6u64 {
            svc.restart_done();
            svc.on_fault(t * 100);
        }
        assert_eq!(svc.state, ServiceState::Faulted);
        svc.recover(1_000_000);
        assert_eq!(svc.state, ServiceState::Running);
        assert_eq!(svc.fault_count, 0);
    }

    /// 多服务表：单服务故障不影响其他服务状态（隔离的 supervisor 侧）。
    #[test]
    fn test_supervisor_isolation() {
        let mut sup = Supervisor::new(&["svc-hello", "svc-fault", "svc-hog"], 0);
        sup.on_service_fault(1); // 仅 svc-fault 故障
        assert_eq!(sup.state_of(0), Some(ServiceState::Running));
        assert_eq!(sup.state_of(1), Some(ServiceState::Suspended));
        assert_eq!(sup.state_of(2), Some(ServiceState::Running));
        sup.on_restart_done(1);
        assert_eq!(sup.state_of(1), Some(ServiceState::Running));
    }

    /// 越界索引安全（fail-closed：返回 None 不 panic）。
    #[test]
    fn test_supervisor_out_of_range_safe() {
        let mut sup = Supervisor::new(&["svc-hello"], 0);
        assert_eq!(sup.on_service_fault(5), None);
    }
}
