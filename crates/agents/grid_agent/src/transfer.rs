//! EnerOS v0.84.0 Grid Agent — 并离网切换状态机模块.
//!
//! [`GridTransfer`] 切换状态机 + RTOS 快平面紧急命令通道抽象.
//!
//! 在 v0.82.0 [`crate::GridState`] + v0.83.0 [`crate::PccState`] + v0.84.0
//! [`crate::island_detect::IslandDetector`] 基础上实现并离网切换管理：
//! - 手动切换（[`transfer_to`](GridTransfer::transfer_to)）：外部命令驱动
//!   GridConnected ↔ Islanded 切换，经 RTOS 通道下发紧急命令.
//! - 自动切换（[`check_and_transfer`](GridTransfer::check_and_transfer)）：
//!   周期性调用孤岛检测器，达到 [`IslandResult::Islanded`] 自动离网，
//!   检测到电网恢复（[`IslandResult::GridOk`]）自动并网.
//! - 中间态 [`TransferState::Transferring`]：切换进行中，避免重入.
//!
//! # 核心类型
//!
//! - [`GridTransfer`] — 切换管理器（持有 [`IslandDetector`] + [`RtosChannel`]）
//! - [`TransferState`] — 切换状态（GridConnected / Islanded / Transferring，默认 GridConnected）
//! - [`TransferReason`] — 切换原因（Manual / IslandDetected / GridRecovered / Fault）
//! - [`TransferCommand`] — 切换命令（OpenPccAndIsland / ClosePccAndSync）
//! - [`TransferRecord`] — 切换记录（时戳 / from / to / duration_ms / reason）
//! - [`TransferError`] — 切换错误（InvalidTarget / AlreadyInTarget / ChannelTimeout / ChannelError）
//! - [`RtosChannel`] / [`MockRtosChannel`] — RTOS 紧急命令通道 trait + Mock
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::boxed::Box`（trait 对象）+ `core::*`，无 `std` / `async` /
//! `panic!` / `unsafe` / `todo!` / `unreachable!` / `Instant::now()` /
//! `Duration::from_millis()`。`no_std` 属性继承自 `lib.rs`。
//!
//! # v0.84.0 偏差声明 (D1~D14)
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | D1 | async fn transfer_to() | sync transfer_to() | no_std 无 async runtime；沿用 v0.82/v0.83 sync 模式 |
//! | D2 | 三态切换（含 Fault） | 三态（GridConnected/Islanded/Transferring） | Fault 归一为 TransferReason::Fault，避免状态机膨胀 |
//! | D3 | 独立错误类型 | TransferError 4 变体 | surgical — 切换流程独立错误，不复用 GridError |
//! | D4 | 嵌入 GridAgent | 独立 GridTransfer 组件 | surgical — 不破坏 v0.82.0 GridAgent；与 PccManager/IslandDetector 模式一致 |
//! | D5 | Instant::now() 时戳 | 外部驱动（调用方提供 now_ms） | no_std 无 Instant；时戳由调度器外部提供 |
//! | D6 | unreachable!() 处理 Transferring 分支 | if/else 提前返回 InvalidTarget | no_std panic-free：禁用 panic!/todo!/unreachable! |
//! | D7 | 切换超时由 Instant 测量 | 由 RTOS 通道返回 elapsed_ms | no_std 无 Instant；通道实现负责计时 |
//! | D8 | docs/phase2/ + config/ | docs/agents/ + 内嵌单元测试 | 工作区规则 §2.3.3；沿用 v0.82/v0.83 测试模式 |
//! | D9 | 集成测试 tests/ | transfer.rs 内嵌单元测试 | 沿用 v0.82/v0.83 模式 |
//! | D10 | check_and_transfer 内部防抖 | 复用 IslandDetector 连续确认 | 不重复防抖逻辑；IslandDetector 已有 confirmation_threshold |
//! | D11 | Transferring 中间态超时回退 | 通道失败时立即回滚到原状态 | RTOS 通道同步返回，无超时分支；超时归一为 ChannelTimeout |
//! | D12 | TransferState 4 变体（含 Fault） | 3 变体 | 同 D2：Fault 归一为 Reason |
//! | D13 | TransferRecord 含 String 描述 | 全 Copy（u64/u32 + 枚举） | no_std Copy 语义；与 PccState 模式一致 |
//! | D14 | GridTransfer Copy | 非 Copy（持有 Box<dyn RtosChannel>） | trait 对象不可 Copy；避免意外分裂 |

use alloc::boxed::Box;

use crate::island_detect::{IslandDetector, IslandResult};
use crate::{GridState, PccState};

// ===== Enums =====

/// 切换状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferState {
    /// 并网（PCC 合闸，与主网同步）.
    #[default]
    GridConnected,
    /// 离网（PCC 分闸，独立运行）.
    Islanded,
    /// 切换中（PCC 操作进行中，避免重入）.
    Transferring,
}

/// 切换原因.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferReason {
    /// 手动操作（运维指令）.
    Manual,
    /// 孤岛检测触发（自动离网）.
    IslandDetected,
    /// 电网恢复（自动并网）.
    GridRecovered,
    /// 故障（保护动作）.
    Fault,
}

/// 切换命令（下发至 RTOS 快平面）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferCommand {
    /// 分闸 PCC + 进入孤岛模式.
    OpenPccAndIsland,
    /// 合闸 PCC + 同步并网.
    ClosePccAndSync,
}

/// 切换错误.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    /// 目标状态非法（如显式切换到 [`TransferState::Transferring`]）.
    InvalidTarget,
    /// 已处于目标状态（无需切换）.
    AlreadyInTarget,
    /// RTOS 通道下发超时.
    ChannelTimeout,
    /// RTOS 通道下发失败.
    ChannelError,
}

// ===== TransferRecord =====

/// 切换记录（D13：全 Copy）.
///
/// 由 [`GridTransfer::transfer_to`] 成功返回，记录本次切换的时戳、原/目标状态、
/// 切换耗时与原因。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransferRecord {
    /// 切换完成时戳（ms，由调用方提供）.
    pub timestamp: u64,
    /// 原状态.
    pub from: TransferState,
    /// 目标状态.
    pub to: TransferState,
    /// 切换耗时（ms，由 RTOS 通道返回）.
    pub duration_ms: u32,
    /// 切换原因.
    pub reason: TransferReason,
}

// ===== RtosChannel trait + MockRtosChannel =====

/// RTOS 快平面紧急命令通道抽象（D7：通道实现负责计时）.
///
/// 由具体 RTOS 适配器实现 `send_emergency`：将紧急切换命令（[`TransferCommand`]）
/// 同步下发至 RTOS 快平面控制任务，返回命令执行耗时 `elapsed_ms`。
///
/// 不要求 `Send + Sync`（no_std 单线程）。失败时返回 [`TransferError`]：
/// - [`ChannelError`] — 通道下发失败（队列满/通信错误）
/// - [`ChannelTimeout`] — 通道下发超时
pub trait RtosChannel {
    /// 下发紧急切换命令，返回 `elapsed_ms`（命令执行耗时）.
    ///
    /// `cmd` 为切换命令，`now_ms` 为调用方提供的当前时戳（用于 RTOS 端日志/审计）。
    /// 成功返回 `Ok(elapsed_ms)`，失败返回 `Err(TransferError)`。
    fn send_emergency(&mut self, cmd: TransferCommand, now_ms: u64) -> Result<u64, TransferError>;
}

/// Mock RTOS 通道（用于测试）.
///
/// 持有 `elapsed_ms`（成功路径返回值）与 `fail`（是否模拟失败）。
#[derive(Debug, Clone)]
pub struct MockRtosChannel {
    /// 成功路径返回的命令执行耗时（ms）.
    pub elapsed_ms: u64,
    /// 是否模拟通道失败（`true` → `send_emergency` 返回 `Err(ChannelError)`）.
    pub fail: bool,
}

impl MockRtosChannel {
    /// 创建成功路径通道（返回给定 `elapsed_ms`，`fail = false`）.
    pub fn new(elapsed_ms: u64) -> Self {
        Self {
            elapsed_ms,
            fail: false,
        }
    }

    /// 创建失败路径通道（`fail = true`，`elapsed_ms = 0`）.
    pub fn new_failing() -> Self {
        Self {
            elapsed_ms: 0,
            fail: true,
        }
    }
}

impl RtosChannel for MockRtosChannel {
    fn send_emergency(
        &mut self,
        _cmd: TransferCommand,
        _now_ms: u64,
    ) -> Result<u64, TransferError> {
        if self.fail {
            Err(TransferError::ChannelError)
        } else {
            Ok(self.elapsed_ms)
        }
    }
}

// ===== GridTransfer =====

/// 并离网切换管理器（D4：独立组件，不嵌入 [`crate::GridAgent`]）.
///
/// 持有 [`IslandDetector`]（孤岛检测）与 [`RtosChannel`]（RTOS 快平面通道），
/// 维护当前 [`TransferState`] 与最近一次 [`TransferRecord`]。提供两类入口：
///
/// - [`transfer_to`](Self::transfer_to) — 手动切换（外部命令驱动）.
/// - [`check_and_transfer`](Self::check_and_transfer) — 自动切换（孤岛检测驱动）.
///
/// # 状态机
///
/// ```text
///   GridConnected ──transfer_to(Islanded)──> Transferring ──通道OK──> Islanded
///        ▲                                       │
///        │                                       └──通道Err──> 回滚到原状态
///        └──transfer_to(GridConnected)── Islanded
/// ```
///
/// 非 `Copy`（D14）：持有 `Box<dyn RtosChannel>` trait 对象。
pub struct GridTransfer {
    /// 孤岛检测器（用于 [`check_and_transfer`](Self::check_and_transfer)）.
    pub detector: IslandDetector,
    /// 当前切换状态.
    pub state: TransferState,
    /// 最近一次切换记录（`None` 表示尚未发生过切换）.
    pub last_transfer: Option<TransferRecord>,
    /// RTOS 快平面紧急命令通道（trait 对象）.
    pub rtos_channel: Box<dyn RtosChannel>,
}

impl GridTransfer {
    /// 创建切换管理器.
    ///
    /// 初始化：`state = GridConnected`、`last_transfer = None`。
    pub fn new(detector: IslandDetector, rtos_channel: Box<dyn RtosChannel>) -> Self {
        Self {
            detector,
            state: TransferState::GridConnected,
            last_transfer: None,
            rtos_channel,
        }
    }

    /// 返回当前切换状态.
    pub fn current_state(&self) -> TransferState {
        self.state
    }

    /// 返回最近一次切换记录（`None` 表示尚未发生过切换）.
    pub fn last_transfer(&self) -> Option<TransferRecord> {
        self.last_transfer
    }

    /// 手动切换到目标状态.
    ///
    /// # 切换流程
    ///
    /// 1. **同状态切换**：`self.state == target` → 返回 `Err(AlreadyInTarget)`，
    ///    状态不变.
    /// 2. **非法目标**：`target == Transferring` → 返回 `Err(InvalidTarget)`，
    ///    状态不变（D6：禁用 `unreachable!`，提前返回）.
    /// 3. **进入中间态**：`self.state = Transferring`，记录原状态 `from`.
    /// 4. **映射命令**：`Islanded → OpenPccAndIsland` / `GridConnected → ClosePccAndSync`.
    /// 5. **调用通道**：`rtos_channel.send_emergency(cmd, now_ms)`.
    ///    - 成功 → 记录 [`TransferRecord`]，状态切换为 `target`，返回 `Ok(record)`.
    ///    - 失败 → 回滚状态为 `from`（D11），返回 `Err(e)`.
    pub fn transfer_to(
        &mut self,
        target: TransferState,
        reason: TransferReason,
        now_ms: u64,
    ) -> Result<TransferRecord, TransferError> {
        // 1. 同状态切换
        if self.state == target {
            return Err(TransferError::AlreadyInTarget);
        }
        // 2. 不能显式切换到 Transferring（D6：提前返回，禁用 unreachable!）
        if target == TransferState::Transferring {
            return Err(TransferError::InvalidTarget);
        }
        // 3. 记录原状态，进入中间态
        let from = self.state;
        self.state = TransferState::Transferring;
        // 4. 映射 target → command（仅 Islanded / GridConnected 可达，D6）
        let cmd = if target == TransferState::Islanded {
            TransferCommand::OpenPccAndIsland
        } else {
            TransferCommand::ClosePccAndSync
        };
        // 5. 调用 RTOS 通道
        match self.rtos_channel.send_emergency(cmd, now_ms) {
            Ok(elapsed_ms) => {
                let record = TransferRecord {
                    timestamp: now_ms,
                    from,
                    to: target,
                    duration_ms: elapsed_ms as u32,
                    reason,
                };
                self.state = target;
                self.last_transfer = Some(record);
                Ok(record)
            }
            Err(e) => {
                // D11 回滚：保持原状态
                self.state = from;
                Err(e)
            }
        }
    }

    /// 自动切换（孤岛检测驱动）.
    ///
    /// # 检测流程
    ///
    /// 1. **重入保护**：`state == Transferring` → 返回 `None`（切换中不重入）.
    /// 2. **调用检测器**：`detector.detect(pcc, grid)` 返回 [`IslandResult`].
    /// 3. **匹配 (result, state)**：
    ///    - `(Islanded, GridConnected)` → 自动离网（`reason = IslandDetected`）.
    ///    - `(GridOk, Islanded)` → 自动并网（`reason = GridRecovered`）.
    ///    - 其他 → 返回 `None`（包括 `Uncertain` 与同状态）.
    ///
    /// 通道失败时（[`transfer_to`](Self::transfer_to) 返回 `Err`），归一为 `None`
    /// 返回（状态由 `transfer_to` 内部回滚，不向上传递错误）。
    pub fn check_and_transfer(
        &mut self,
        pcc: &PccState,
        grid: &GridState,
        now_ms: u64,
    ) -> Option<TransferRecord> {
        // 1. 切换中避免重入
        if self.state == TransferState::Transferring {
            return None;
        }
        // 2. 调用孤岛检测器
        let result = self.detector.detect(pcc, grid);
        // 3. 匹配 (result, state)
        match (result, self.state) {
            (IslandResult::Islanded, TransferState::GridConnected) => self
                .transfer_to(
                    TransferState::Islanded,
                    TransferReason::IslandDetected,
                    now_ms,
                )
                .ok(),
            (IslandResult::GridOk, TransferState::Islanded) => self
                .transfer_to(
                    TransferState::GridConnected,
                    TransferReason::GridRecovered,
                    now_ms,
                )
                .ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island_detect::{IslandConfig, IslandDetector};
    use crate::{DataQuality, GridState, PccState, PccStatus};

    // ===== 辅助函数 =====

    /// 构造 PCC Islanded 状态.
    fn pcc_islanded() -> PccState {
        PccState {
            status: PccStatus::Islanded,
            ..PccState::default()
        }
    }

    /// 构造 PCC GridConnected 状态.
    fn pcc_connected() -> PccState {
        PccState {
            status: PccStatus::GridConnected,
            ..PccState::default()
        }
    }

    /// 构造频率=50.0/电压=220.0 的正常电网状态.
    fn grid_normal() -> GridState {
        GridState {
            frequency: 50.0,
            voltage_a: 220.0,
            quality: DataQuality::Good,
            ..GridState::default()
        }
    }

    /// 构造一个默认 GridTransfer + MockRtosChannel(50ms).
    fn make_transfer() -> GridTransfer {
        GridTransfer::new(
            IslandDetector::new_default(),
            Box::new(MockRtosChannel::new(50)),
        )
    }

    // ===== T97: TransferState::default() == GridConnected =====
    #[test]
    fn t97_transfer_state_default() {
        assert_eq!(TransferState::default(), TransferState::GridConnected);
    }

    // ===== T98: 4 TransferReason 变体 Debug 输出非空 =====
    #[test]
    fn t98_transfer_reason_debug_nonempty() {
        for r in [
            TransferReason::Manual,
            TransferReason::IslandDetected,
            TransferReason::GridRecovered,
            TransferReason::Fault,
        ] {
            let s = alloc::format!("{:?}", r);
            assert!(!s.is_empty());
        }
    }

    // ===== T99: 2 TransferCommand 变体 Debug 输出非空 =====
    #[test]
    fn t99_transfer_command_debug_nonempty() {
        for c in [
            TransferCommand::OpenPccAndIsland,
            TransferCommand::ClosePccAndSync,
        ] {
            let s = alloc::format!("{:?}", c);
            assert!(!s.is_empty());
        }
    }

    // ===== T100: TransferRecord 构造 + 5 字段访问 =====
    #[test]
    fn t100_transfer_record_construction() {
        let rec = TransferRecord {
            timestamp: 1000,
            from: TransferState::GridConnected,
            to: TransferState::Islanded,
            duration_ms: 50,
            reason: TransferReason::IslandDetected,
        };
        assert_eq!(rec.timestamp, 1000);
        assert_eq!(rec.from, TransferState::GridConnected);
        assert_eq!(rec.to, TransferState::Islanded);
        assert_eq!(rec.duration_ms, 50);
        assert_eq!(rec.reason, TransferReason::IslandDetected);
    }

    // ===== T101: 4 TransferError 变体 PartialEq 相等 =====
    #[test]
    fn t101_transfer_error_partial_eq() {
        assert_eq!(TransferError::InvalidTarget, TransferError::InvalidTarget);
        assert_eq!(
            TransferError::AlreadyInTarget,
            TransferError::AlreadyInTarget
        );
        assert_eq!(TransferError::ChannelTimeout, TransferError::ChannelTimeout);
        assert_eq!(TransferError::ChannelError, TransferError::ChannelError);
        assert_ne!(TransferError::InvalidTarget, TransferError::AlreadyInTarget);
    }

    // ===== T102: MockRtosChannel::new(50) fail=false / elapsed_ms=50 =====
    #[test]
    fn t102_mock_rtos_channel_new() {
        let m = MockRtosChannel::new(50);
        assert!(!m.fail);
        assert_eq!(m.elapsed_ms, 50);
    }

    // ===== T103: MockRtosChannel::new_failing() fail=true / elapsed_ms=0 =====
    #[test]
    fn t103_mock_rtos_channel_new_failing() {
        let m = MockRtosChannel::new_failing();
        assert!(m.fail);
        assert_eq!(m.elapsed_ms, 0);
    }

    // ===== T104: MockRtosChannel 成功路径 send_emergency 返回 Ok(50) =====
    #[test]
    fn t104_mock_rtos_channel_send_emergency_ok() {
        let mut m = MockRtosChannel::new(50);
        assert_eq!(
            m.send_emergency(TransferCommand::OpenPccAndIsland, 1000),
            Ok(50)
        );
    }

    // ===== T105: MockRtosChannel 失败路径 send_emergency 返回 Err(ChannelError) =====
    #[test]
    fn t105_mock_rtos_channel_send_emergency_err() {
        let mut m = MockRtosChannel::new_failing();
        assert_eq!(
            m.send_emergency(TransferCommand::ClosePccAndSync, 1000),
            Err(TransferError::ChannelError)
        );
    }

    // ===== T106: GridTransfer::new 初始化 state=GridConnected / last_transfer=None =====
    #[test]
    fn t106_grid_transfer_new() {
        let t = make_transfer();
        assert_eq!(t.current_state(), TransferState::GridConnected);
        assert_eq!(t.last_transfer(), None);
    }

    // ===== T107: transfer_to(Islanded) 返回 Ok =====
    #[test]
    fn t107_transfer_to_islanded_ok() {
        let mut t = make_transfer();
        let r = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        assert!(r.is_ok());
    }

    // ===== T108: transfer_to(Islanded) 后 state == Islanded =====
    #[test]
    fn t108_transfer_to_islanded_state_changed() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        assert_eq!(t.current_state(), TransferState::Islanded);
    }

    // ===== T109: transfer_to(Islanded) 后 last_transfer 记录 from/to =====
    #[test]
    fn t109_transfer_to_islanded_last_transfer_record() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let opt = t.last_transfer();
        assert!(opt.is_some());
        let rec = opt.unwrap();
        assert_eq!(rec.to, TransferState::Islanded);
        assert_eq!(rec.from, TransferState::GridConnected);
    }

    // ===== T110: TransferRecord.duration_ms == 50 (来自 MockRtosChannel::new(50)) =====
    #[test]
    fn t110_transfer_record_duration_ms() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let rec = t.last_transfer().unwrap();
        assert_eq!(rec.duration_ms, 50);
    }

    // ===== T111: TransferRecord.timestamp == 1000 =====
    #[test]
    fn t111_transfer_record_timestamp() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let rec = t.last_transfer().unwrap();
        assert_eq!(rec.timestamp, 1000);
    }

    // ===== T112: TransferRecord.reason == IslandDetected =====
    #[test]
    fn t112_transfer_record_reason() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let rec = t.last_transfer().unwrap();
        assert_eq!(rec.reason, TransferReason::IslandDetected);
    }

    // ===== T113: 同状态切换 → Err(AlreadyInTarget) / 状态不变 =====
    #[test]
    fn t113_transfer_to_already_in_target() {
        let mut t = make_transfer();
        let r = t.transfer_to(TransferState::GridConnected, TransferReason::Manual, 1000);
        assert_eq!(r, Err(TransferError::AlreadyInTarget));
        assert_eq!(t.current_state(), TransferState::GridConnected);
    }

    // ===== T114: 切换到 Transferring → Err(InvalidTarget) =====
    #[test]
    fn t114_transfer_to_transferring_invalid_target() {
        let mut t = make_transfer();
        let r = t.transfer_to(TransferState::Transferring, TransferReason::Fault, 1000);
        assert_eq!(r, Err(TransferError::InvalidTarget));
    }

    // ===== T115: 通道失败 → Err(ChannelError) + 状态回滚 (D5) =====
    #[test]
    fn t115_transfer_to_channel_error_rollback() {
        let mut t = GridTransfer::new(
            IslandDetector::new_default(),
            Box::new(MockRtosChannel::new_failing()),
        );
        let r = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        assert_eq!(r, Err(TransferError::ChannelError));
        assert_eq!(t.current_state(), TransferState::GridConnected);
    }

    // ===== T116: Islanded → GridConnected 切换成功 =====
    #[test]
    fn t116_transfer_back_to_grid_connected() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let r2 = t.transfer_to(
            TransferState::GridConnected,
            TransferReason::GridRecovered,
            2000,
        );
        assert!(r2.is_ok());
        assert_eq!(t.current_state(), TransferState::GridConnected);
    }

    // ===== T117: check_and_transfer 正常电网 → None (GridOk, 无动作) =====
    #[test]
    fn t117_check_and_transfer_normal_grid_no_action() {
        let mut t = make_transfer();
        let r = t.check_and_transfer(&pcc_connected(), &grid_normal(), 1000);
        assert!(r.is_none());
        assert_eq!(t.current_state(), TransferState::GridConnected);
    }

    // ===== T118: 3 次 PCC Islanded → 第 3 次自动离网 =====
    #[test]
    fn t118_check_and_transfer_three_consecutive_auto_island() {
        let mut t = make_transfer();
        let pcc = pcc_islanded();
        let grid = grid_normal();
        let r1 = t.check_and_transfer(&pcc, &grid, 1000);
        assert!(r1.is_none());
        let r2 = t.check_and_transfer(&pcc, &grid, 2000);
        assert!(r2.is_none());
        let r3 = t.check_and_transfer(&pcc, &grid, 3000);
        assert!(r3.is_some());
        let rec = r3.unwrap();
        assert_eq!(rec.to, TransferState::Islanded);
        assert_eq!(t.current_state(), TransferState::Islanded);
    }

    // ===== T119: Islanded 状态下 GridOk → 首次即自动并网 (GridRecovered) =====
    #[test]
    fn t119_check_and_transfer_auto_recover_on_grid_ok() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let pcc = pcc_connected();
        let grid = grid_normal();
        // IslandDetector 对恢复方向无需连续确认：count=0 立即返回 GridOk，
        // 因此首次 check_and_transfer 即触发自动并网（与 Islanded 方向的 3 次确认不同）.
        let r1 = t.check_and_transfer(&pcc, &grid, 2000);
        assert!(r1.is_some());
        let rec = r1.unwrap();
        assert_eq!(rec.to, TransferState::GridConnected);
        assert_eq!(rec.reason, TransferReason::GridRecovered);
        assert_eq!(t.current_state(), TransferState::GridConnected);
        // 已并网后再次调用 → (GridOk, GridConnected) → None（无动作）
        let r2 = t.check_and_transfer(&pcc, &grid, 3000);
        assert!(r2.is_none());
    }

    // ===== T120: state=Transferring → check_and_transfer 返回 None (重入保护) =====
    #[test]
    fn t120_check_and_transfer_transferring_no_reentry() {
        let mut t = GridTransfer {
            detector: IslandDetector::new_default(),
            state: TransferState::Transferring,
            last_transfer: None,
            rtos_channel: Box::new(MockRtosChannel::new(50)),
        };
        let r = t.check_and_transfer(&pcc_islanded(), &grid_normal(), 1000);
        assert!(r.is_none());
    }

    // ===== T121: 1 次 PCC Islanded → count=1, 无动作 (Uncertain) =====
    #[test]
    fn t121_check_and_transfer_first_uncertain_count_one() {
        let mut t = make_transfer();
        let pcc = pcc_islanded();
        let grid = grid_normal();
        let r1 = t.check_and_transfer(&pcc, &grid, 1000);
        assert!(r1.is_none());
        assert_eq!(t.current_state(), TransferState::GridConnected);
        assert_eq!(t.detector.current_count(), 1);
    }

    // ===== T122: 2 次 PCC Islanded → count=2, 无动作 =====
    #[test]
    fn t122_check_and_transfer_second_uncertain_count_two() {
        let mut t = make_transfer();
        let pcc = pcc_islanded();
        let grid = grid_normal();
        let _ = t.check_and_transfer(&pcc, &grid, 1000);
        let r2 = t.check_and_transfer(&pcc, &grid, 2000);
        assert!(r2.is_none());
        assert_eq!(t.detector.current_count(), 2);
    }

    // ===== T123: TransferRecord 派生 Copy 可复制 =====
    #[test]
    fn t123_transfer_record_copy() {
        let rec = TransferRecord {
            timestamp: 1,
            from: TransferState::GridConnected,
            to: TransferState::Islanded,
            duration_ms: 50,
            reason: TransferReason::IslandDetected,
        };
        let rec2 = rec;
        assert_eq!(rec2, rec);
    }

    // ===== T124: Option<TransferRecord> 派生 Copy 可复制 =====
    #[test]
    fn t124_option_transfer_record_copy() {
        let rec = TransferRecord {
            timestamp: 1,
            from: TransferState::GridConnected,
            to: TransferState::Islanded,
            duration_ms: 50,
            reason: TransferReason::IslandDetected,
        };
        let opt: Option<TransferRecord> = Some(rec);
        let opt2 = opt;
        assert_eq!(opt2, opt);
    }

    // ===== T125: 多次切换序列 GridConnected → Islanded → GridConnected =====
    #[test]
    fn t125_multi_transfer_sequence() {
        let mut t = make_transfer();
        let _ = t.transfer_to(
            TransferState::Islanded,
            TransferReason::IslandDetected,
            1000,
        );
        let _ = t.transfer_to(
            TransferState::GridConnected,
            TransferReason::GridRecovered,
            2000,
        );
        assert_eq!(t.current_state(), TransferState::GridConnected);
        let last = t.last_transfer().unwrap();
        assert_eq!(last.to, TransferState::GridConnected);
        assert_eq!(last.from, TransferState::Islanded);
        assert_eq!(last.reason, TransferReason::GridRecovered);
        assert_eq!(last.timestamp, 2000);
    }

    // ===== T126: check_and_transfer 后 detector.current_count() 反映最新计数 =====
    #[test]
    fn t126_check_and_transfer_updates_detector_count() {
        let mut t = make_transfer();
        let _ = t.check_and_transfer(&pcc_islanded(), &grid_normal(), 1000);
        assert_eq!(t.detector.current_count(), 1);
        let _ = t.check_and_transfer(&pcc_islanded(), &grid_normal(), 2000);
        assert_eq!(t.detector.current_count(), 2);
    }

    // 防止未使用导入告警（IslandConfig 在测试中被引用）
    #[test]
    fn _ensure_imports_used() {
        let _cfg = IslandConfig::default();
    }
}
