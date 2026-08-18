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
use alloc::string::ToString;
use alloc::vec::Vec;

/// 服务运行状态（**故障恢复**状态机：6 级重启语义，见 [`ServiceSupervisor`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// 正常运行（或等待重启执行）。
    Running,
    /// 故障后等待退避重启。
    Suspended,
    /// 连续故障超阈值，不再自动重启（人工介入）。
    Faulted,
}

/// WP-37.1（复盘）：蓝图 11 态**生命周期**状态机——从声明到受监督的
/// 完整运行时状态，与 [`ServiceState`]（故障恢复）正交：同一服务可同时
/// 处于生命周期 `Ready` 与恢复态 `Suspended`（故障等待退避重启时其
/// 能力仍处于生命周期 Ready/降级语义由调用方按 readiness 契约解释）。
///
/// 转换矩阵（非法转换拒绝，防跳步/回退）：
/// ```text
/// Declared → Verified → Created → Mapped → Configured → Started
/// Started → Ready | Degraded | Failed | Stopped
/// Ready   → Degraded | Failed | Stopped
/// Degraded→ Ready | Failed | Stopped
/// Failed  → Restarting | Stopped
/// Restarting → Started | Failed
/// Stopped → （终态）
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// 已声明（manifest/bundle 条目）。
    Declared,
    /// 已校验（digest/ELF 校验通过）。
    Verified,
    /// 已创建（TCB/VSpace/CNode 对象创建）。
    Created,
    /// 已映射（地址空间/页表建立）。
    Mapped,
    /// 已配置（能力/寄存器/优先级）。
    Configured,
    /// 已启动（tcb_resume 执行）。
    Started,
    /// 就绪（readiness IPC handshake 通过）。
    Ready,
    /// 降级（optional 能力缺失/部分功能不可用）。
    Degraded,
    /// 失败（required 阻断或多次重启仍失败）。
    Failed,
    /// 重启中（重启预算内退避后重新 Started）。
    Restarting,
    /// 停止（终态）。
    Stopped,
}

/// 非法生命周期转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleError {
    /// 当前状态。
    pub from: LifecycleState,
    /// 被拒绝的目标状态。
    pub to: LifecycleState,
}

/// 单服务生命周期状态机（蓝图 WP-37.1 数据模型）。
#[derive(Debug, Clone)]
pub struct ServiceLifecycle {
    state: LifecycleState,
}

impl ServiceLifecycle {
    /// 新建：`Declared`。
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Declared,
        }
    }

    /// 当前生命周期状态。
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// 尝试转换；非法转换拒绝（不改变状态）。
    pub fn transition(&mut self, to: LifecycleState) -> Result<(), LifecycleError> {
        if Self::allowed(self.state, to) {
            self.state = to;
            Ok(())
        } else {
            Err(LifecycleError {
                from: self.state,
                to,
            })
        }
    }

    /// 合法转换表（蓝图 11 态单向推进；重启回路仅 Failed→Restarting→
    /// Started，且由调用方在重启预算内驱动）。
    fn allowed(from: LifecycleState, to: LifecycleState) -> bool {
        use LifecycleState::*;
        matches!(
            (from, to),
            (Declared, Verified)
                | (Verified, Created)
                | (Created, Mapped)
                | (Mapped, Configured)
                | (Configured, Started)
                | (Started, Ready)
                | (Started, Degraded)
                | (Started, Failed)
                | (Started, Stopped)
                | (Ready, Degraded)
                | (Ready, Failed)
                | (Ready, Stopped)
                | (Degraded, Ready)
                | (Degraded, Failed)
                | (Degraded, Stopped)
                | (Failed, Restarting)
                | (Failed, Stopped)
                | (Restarting, Started)
                | (Restarting, Failed)
        )
    }
}

impl Default for ServiceLifecycle {
    fn default() -> Self {
        Self::new()
    }
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

/// WP-37.2（复盘落地）：capability 授权集——**声明即授权**；未声明
/// capability 访问一律拒绝（蓝图：capability 未声明、超预算、跨 tenant
/// 或错误 badge 直接失败）。no_std：BTreeSet（alloc）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityGrantSet {
    grants: alloc::collections::BTreeSet<String>,
}

impl CapabilityGrantSet {
    /// 空授权集（默认拒绝一切）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 声明一个授权。
    pub fn grant(&mut self, capability: &str) {
        self.grants.insert(capability.to_string());
    }

    /// 授权校验：未声明 → false（fail-closed）。
    pub fn is_granted(&self, capability: &str) -> bool {
        self.grants.contains(capability)
    }

    /// 授权条目数（预算审计用）。
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// 是否空集。
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }
}

/// 单服务 supervisor 状态。
#[derive(Debug, Clone)]
pub struct ServiceSupervisor {
    /// 服务名（日志/告警标识）。
    pub name: String,
    /// 当前状态（故障恢复）。
    pub state: ServiceState,
    /// WP-37.1（复盘）：生命周期状态（蓝图 11 态，与恢复态正交）。
    pub lifecycle: ServiceLifecycle,
    /// WP-37.2（复盘）：重启纪元——每次重启递增；旧 fencing token/
    /// endpoint 的 epoch < 当前纪元 → 失效（吊销语义）。
    pub epoch: u64,
    /// WP-37.2（复盘）：能力授权集（唯一授权来源；未声明 → 拒绝）。
    pub grants: CapabilityGrantSet,
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
    /// 新建（Running，空窗口；生命周期 Declared；epoch=1；空授权集）。
    pub fn new(name: &str, now_ms: u64) -> Self {
        Self {
            name: String::from(name),
            state: ServiceState::Running,
            lifecycle: ServiceLifecycle::new(),
            epoch: 1,
            grants: CapabilityGrantSet::new(),
            fault_count: 0,
            window_start_ms: now_ms,
            last_fault_ms: now_ms,
            backoff_idx: 0,
            restart_count: 0,
        }
    }

    /// 生命周期转换（非法跳步/回退拒绝，状态不变）。
    pub fn lifecycle_transition(&mut self, to: LifecycleState) -> Result<(), LifecycleError> {
        self.lifecycle.transition(to)
    }

    /// WP-37.1 #8：推进生命周期到 `Started`（spawn 完成后调用；readiness
    /// 探测前的基线）。逐级走合法转换（Declared→…→Started）；任一级失败
    /// 即停（状态保持当前级，不猜测）。
    pub fn advance_lifecycle_to_started(&mut self) {
        use LifecycleState::*;
        for target in [Verified, Created, Mapped, Configured, Started] {
            if self.lifecycle_transition(target).is_err() {
                break;
            }
        }
    }

    /// 当前生命周期状态。
    pub fn lifecycle_state(&self) -> LifecycleState {
        self.lifecycle.state()
    }

    /// WP-37.2（复盘）：fencing token 校验——token 签发纪元 < 当前纪元
    /// → 已吊销（restart 后旧 endpoint/fencing token 失效）。
    pub fn fencing_token_valid(&self, token_epoch: u64) -> bool {
        token_epoch >= self.epoch
    }

    /// 当前纪元（签发新 token 用）。
    pub fn current_epoch(&self) -> u64 {
        self.epoch
    }

    /// 能力授权校验（唯一授权来源；未声明 → 拒绝）。
    pub fn capability_granted(&self, capability: &str) -> bool {
        self.grants.is_granted(capability)
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
            // WP-37.2（复盘）：重启完成 → 纪元递增——吊销旧 endpoint/
            // fencing token（旧 token epoch < 新纪元 → 拒绝）。
            self.epoch = self.epoch.saturating_add(1);
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

    /// 按服务数新建（占位名 `svc-<i>`——30 RT-029：生产路径已改用
    /// [`Supervisor::new`] 真实服务名；本构造仅测试批量便捷，禁止生产消费）。
    #[cfg(test)]
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

    /// WP-37.2（复盘）：重启纪元递增吊销旧 token；能力授权默认拒绝。
    #[test]
    fn test_epoch_revokes_old_tokens_and_grants_are_fail_closed() {
        let mut svc = ServiceSupervisor::new("svc-ctrl", 0);
        assert_eq!(svc.current_epoch(), 1);
        // 纪元 1 签发的 token 有效。
        assert!(svc.fencing_token_valid(1));
        // 故障 → 重启 → 纪元 2：旧 token（epoch 1）失效。
        svc.on_fault(10);
        svc.restart_done();
        assert_eq!(svc.current_epoch(), 2);
        assert!(!svc.fencing_token_valid(1), "重启后旧 token 必须吊销");
        assert!(svc.fencing_token_valid(2), "新纪元 token 有效");
        // 再次重启 → 纪元 3。
        svc.on_fault(20);
        svc.restart_done();
        assert!(!svc.fencing_token_valid(2), "再重启后 epoch2 token 也失效");
        // 能力授权：默认拒绝一切（fail-closed）。
        assert!(!svc.capability_granted("any-cap"), "未声明能力必须拒绝");
        svc.grants.grant("ep:svc-ctrl");
        assert!(svc.capability_granted("ep:svc-ctrl"));
        assert!(!svc.capability_granted("ep:other"), "未声明仍拒绝");
        assert!(!svc.grants.is_empty());
    }

    /// WP-37.1（复盘）：生命周期 11 态——正向全链合法，非法跳步/回退拒绝。
    #[test]
    fn test_lifecycle_full_forward_chain_and_illegal_rejected() {
        use LifecycleState::*;
        let mut lc = ServiceLifecycle::new();
        assert_eq!(lc.state(), Declared);
        // 正向全链：Declared → … → Ready。
        for (from, to) in [
            (Declared, Verified),
            (Verified, Created),
            (Created, Mapped),
            (Mapped, Configured),
            (Configured, Started),
            (Started, Ready),
        ] {
            lc.transition(to).expect("forward chain must hold");
            assert_eq!(lc.state(), to, "从 {from:?} → {to:?}");
        }
        // 跳步拒绝：Ready → Mapped 非法。
        assert_eq!(
            lc.transition(Mapped),
            Err(LifecycleError {
                from: Ready,
                to: Mapped
            })
        );
        assert_eq!(lc.state(), Ready, "非法转换不得改变状态");
        // 回退拒绝：Ready → Verified 非法。
        assert!(lc.transition(Verified).is_err());
        // 故障回路：Ready → Failed → Restarting → Started 合法。
        lc.transition(Failed).expect("Ready→Failed");
        lc.transition(Restarting).expect("Failed→Restarting");
        lc.transition(Started).expect("Restarting→Started");
        // 终态：Stopped 后无合法转换。
        lc.transition(Stopped).expect("Started→Stopped");
        assert!(lc.transition(Started).is_err(), "Stopped 为终态");
    }

    /// WP-37.1 #8：advance_lifecycle_to_started 逐级推进（readiness 探测
    /// 前基线）；到达 Started 后 Ready/Degraded 分支合法。
    #[test]
    fn test_advance_lifecycle_to_started_then_readiness_branches() {
        use LifecycleState::*;
        let mut svc = ServiceSupervisor::new("svc-x", 0);
        assert_eq!(svc.lifecycle_state(), Declared);
        svc.advance_lifecycle_to_started();
        assert_eq!(svc.lifecycle_state(), Started);
        // Ready 分支（readiness handshake 通过）。
        svc.lifecycle_transition(Ready).expect("Started→Ready");
        assert_eq!(svc.lifecycle_state(), Ready);
        // Degraded 分支（另一服务）：Started → Degraded。
        let mut svc2 = ServiceSupervisor::new("svc-y", 0);
        svc2.advance_lifecycle_to_started();
        svc2.lifecycle_transition(Degraded)
            .expect("Started→Degraded");
        assert_eq!(svc2.lifecycle_state(), Degraded);
    }

    /// WP-37.1（复盘，review nit）：Degraded 分支全矩阵 + 直接 Started→Failed。
    #[test]
    fn test_lifecycle_degraded_branches_and_direct_fail() {
        use LifecycleState::*;
        // Started → Degraded → Ready（恢复）→ Stopped。
        let mut lc = ServiceLifecycle::new();
        for s in [Verified, Created, Mapped, Configured, Started] {
            lc.transition(s).expect("forward");
        }
        lc.transition(Degraded).expect("Started→Degraded");
        assert_eq!(lc.state(), Degraded);
        lc.transition(Ready).expect("Degraded→Ready（恢复）");
        lc.transition(Degraded).expect("Ready→Degraded");
        lc.transition(Failed).expect("Degraded→Failed");
        lc.transition(Stopped).expect("Failed→Stopped");
        // 直接 Started → Failed（required 阻断场景）。
        let mut lc2 = ServiceLifecycle::new();
        for s in [Verified, Created, Mapped, Configured, Started] {
            lc2.transition(s).expect("forward2");
        }
        lc2.transition(Failed).expect("Started→Failed（直接）");
        assert_eq!(lc2.state(), Failed);
        // Degraded 非法跳步：Degraded → Mapped 拒绝。
        let mut lc3 = ServiceLifecycle::new();
        for s in [Verified, Created, Mapped, Configured, Started, Degraded] {
            lc3.transition(s).expect("forward3");
        }
        assert!(lc3.transition(Mapped).is_err(), "Degraded 回退拒绝");
    }

    /// WP-37.1（复盘）：生命周期与故障恢复状态机正交——生命周期 Ready 的
    /// 服务可同时处于恢复态 Suspended（故障等待退避），互不干扰。
    #[test]
    fn test_lifecycle_orthogonal_to_recovery_state() {
        let mut svc = ServiceSupervisor::new("svc-hello", 0);
        for s in [
            LifecycleState::Verified,
            LifecycleState::Created,
            LifecycleState::Mapped,
            LifecycleState::Configured,
            LifecycleState::Started,
            LifecycleState::Ready,
        ] {
            svc.lifecycle_transition(s).expect("ready path");
        }
        assert_eq!(svc.lifecycle.state(), LifecycleState::Ready);
        // 生命周期 Ready 期间发生故障：恢复态 Suspended，生命周期保持。
        let action = svc.on_fault(10);
        assert!(matches!(action, Action::Restart { .. }));
        assert_eq!(svc.state, ServiceState::Suspended);
        assert_eq!(
            svc.lifecycle.state(),
            LifecycleState::Ready,
            "生命周期与恢复态正交"
        );
        // 重启完成：恢复态回 Running，生命周期不受影响。
        svc.restart_done();
        assert_eq!(svc.state, ServiceState::Running);
    }
}
