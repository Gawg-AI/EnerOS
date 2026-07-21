//! PCC (Point of Common Coupling) 并网点管理模块 (v0.83.0).
//!
//! 实现 [`PccManager`] 周期性读取并网点开关状态 + 功率，计算功率方向/功率因数，
//! 判定并网/离网/过渡态，提供最小防抖逻辑。沿用 v0.82.0 [`crate::GridSampler`]
//! trait 抽象模式（trait + Mock 解耦具体采集源）。
//!
//! # 核心类型
//!
//! - [`PccManager`] — PCC 管理器（持有 reader，周期 `update(now_ms)` 更新状态）
//! - [`PccState`] / [`PccReading`] — 状态快照 / 一次性读取结果
//! - [`BreakerStatus`] / [`PowerDirection`] / [`PccStatus`] — 枚举
//! - [`PccReader`] / [`MockPccReader`] — 读取器 trait + Mock
//! - [`compute_power_direction`] / [`compute_power_factor`] — 辅助函数

use alloc::boxed::Box;

use crate::GridError;

// ===== Enums =====

/// 断路器状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BreakerStatus {
    /// 合闸（并网）.
    Closed,
    /// 分闸（离网，手动操作）.
    Open,
    /// 保护跳闸（离网，故障触发）.
    Tripped,
    /// 未知（数据丢失/通信中断）.
    #[default]
    Unknown,
}

/// 功率方向（约定 P>0 = Import 导入，§8.5）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerDirection {
    /// 导入（从主网吸收功率，P > 1.0）.
    Import,
    /// 导出（向主网送出功率，P < -1.0）.
    Export,
    /// 空载（|P| ≤ 1.0）.
    #[default]
    Idle,
}

/// PCC 并网点状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PccStatus {
    /// 并网（断路器合闸且防抖期已过）.
    GridConnected,
    /// 离网（断路器分闸/跳闸且防抖期已过）.
    Islanded,
    /// 过渡态（防抖期内或断路器状态未知）.
    #[default]
    Transitioning,
}

// ===== Reading / State structs =====

/// PCC 一次性读取结果（D14：原子性读取，避免多次 read() + format!() 调用）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PccReading {
    /// 断路器状态.
    pub breaker_status: BreakerStatus,
    /// 有功功率（W，约定 P>0 = Import）.
    pub active_power: f32,
    /// 无功功率（var）.
    pub reactive_power: f32,
}

/// PCC 状态快照（D13：全 Copy，`pcc_id: u32` 而非 String）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PccState {
    /// PCC 标识（D2：u32 而非 String）.
    pub pcc_id: u32,
    /// 当前断路器状态.
    pub breaker_status: BreakerStatus,
    /// 功率方向.
    pub power_direction: PowerDirection,
    /// 功率因数（|P|<0.1 时返回 1.0）.
    pub power_factor: f32,
    /// 有功功率（W）.
    pub active_power: f32,
    /// 无功功率（var）.
    pub reactive_power: f32,
    /// PCC 状态（含防抖过渡态）.
    pub status: PccStatus,
}

// ===== PccReader trait + MockPccReader =====

/// PCC 数据采集源接口（D3：trait 抽象，避免依赖 eneros-protocol-abstract）.
///
/// 由具体实现（RTU/IED/PMU/保护装置适配器）提供 `read()` 方法，
/// 返回 [`PccReading`] 原子读取结果。不要求 `Send + Sync`（no_std 单线程）。
pub trait PccReader {
    /// 读取指定 PCC 的当前状态.
    ///
    /// `pcc_id` 标识 PCC 点（多 PCC 场景），`now_ms` 为调用方提供的时戳。
    fn read(&mut self, pcc_id: u32, now_ms: u64) -> Result<PccReading, GridError>;
}

/// Mock PCC 读取器（测试用）.
#[derive(Debug, Clone)]
pub struct MockPccReader {
    /// 下一次读取返回的读数.
    pub next_reading: PccReading,
    /// 是否模拟读取失败.
    pub fail: bool,
}

impl MockPccReader {
    /// 创建成功路径读取器，返回给定读数（`fail = false`）.
    pub fn new(reading: PccReading) -> Self {
        MockPccReader {
            next_reading: reading,
            fail: false,
        }
    }

    /// 创建失败路径读取器（`read` 恒返回 `Err(SampleFailed)`）.
    pub fn new_failing() -> Self {
        MockPccReader {
            next_reading: PccReading::default(),
            fail: true,
        }
    }

    /// Builder：替换下一次读取返回的读数.
    pub fn with_reading(mut self, reading: PccReading) -> Self {
        self.next_reading = reading;
        self
    }
}

impl PccReader for MockPccReader {
    fn read(&mut self, _pcc_id: u32, _now_ms: u64) -> Result<PccReading, GridError> {
        if self.fail {
            return Err(GridError::SampleFailed);
        }
        Ok(self.next_reading)
    }
}

// ===== 辅助函数 =====

/// 计算功率方向（D11：P>1.0 Import / P<-1.0 Export / |P|≤1.0 Idle）.
pub fn compute_power_direction(active_power: f32) -> PowerDirection {
    if active_power > 1.0 {
        PowerDirection::Import
    } else if active_power < -1.0 {
        PowerDirection::Export
    } else {
        PowerDirection::Idle
    }
}

/// 计算平方根（D7 修正：no_std 下 `f32::sqrt` 不可用，改用牛顿迭代法手写实现）.
///
/// `x <= 0.0` 时返回 `0.0`；否则用 Newton's method（Heron 法）迭代收敛。
/// 二次收敛，~7 次迭代即达 f32 精度上限，取 20 次确保极端输入稳定。
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    for _ in 0..20 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

/// 计算功率因数（D7：no_std 下 `f32::sqrt` 不可用，改用 [`sqrt_f32`] 牛顿迭代法）.
///
/// `|active_power| < 0.1` 时返回 `1.0`（避免除零与微小值噪声）；
/// 否则返回 `active_power / sqrt(P² + Q²)`。
pub fn compute_power_factor(active_power: f32, reactive_power: f32) -> f32 {
    if active_power.abs() < 0.1 {
        return 1.0;
    }
    let denom = sqrt_f32(active_power * active_power + reactive_power * reactive_power);
    active_power / denom
}

/// 计算断路器稳定状态映射（私有，仅供 [`PccManager`] 内部使用）.
fn compute_stable_status(breaker: BreakerStatus) -> PccStatus {
    match breaker {
        BreakerStatus::Closed => PccStatus::GridConnected,
        BreakerStatus::Open => PccStatus::Islanded,
        BreakerStatus::Tripped => PccStatus::Islanded,
        BreakerStatus::Unknown => PccStatus::Transitioning,
    }
}

// ===== PccManager =====

/// PCC 管理器（D5：独立组件，不嵌入 v0.82.0 [`crate::GridAgent`]）.
///
/// 持有 [`PccReader`] trait 对象，由外部调度器周期性调用
/// [`update`](Self::update) 更新 [`PccState`]。提供最小防抖逻辑（D6）：
/// 断路器状态变化后 `debounce_ms` 内报告 [`PccStatus::Transitioning`]，
/// 过期后稳定为 [`GridConnected`](PccStatus::GridConnected) 或
/// [`Islanded`](PccStatus::Islanded)。
pub struct PccManager {
    /// PCC 标识.
    pub pcc_id: u32,
    /// PCC 读取器（trait 对象）.
    pub reader: Box<dyn PccReader>,
    /// 当前 PCC 状态快照.
    pub state: PccState,
    /// 防抖时长（ms）.
    pub debounce_ms: u64,
    /// 上一次断路器状态（用于变化检测）.
    pub last_breaker_status: BreakerStatus,
    /// 上一次断路器状态变化时戳（ms）.
    pub last_change_ms: u64,
}

impl PccManager {
    /// 创建 PCC 管理器.
    ///
    /// 初始化：`state.pcc_id = pcc_id` + 其余字段默认（含 `status = Transitioning`）、
    /// `last_breaker_status = Unknown`、`last_change_ms = 0`。
    pub fn new(pcc_id: u32, reader: Box<dyn PccReader>, debounce_ms: u64) -> Self {
        PccManager {
            pcc_id,
            reader,
            state: PccState {
                pcc_id,
                ..PccState::default()
            },
            debounce_ms,
            last_breaker_status: BreakerStatus::Unknown,
            last_change_ms: 0,
        }
    }

    /// 返回当前 PCC 状态的引用.
    pub fn current(&self) -> &PccState {
        &self.state
    }

    /// 是否处于离网状态.
    pub fn is_islanded(&self) -> bool {
        self.state.status == PccStatus::Islanded
    }

    /// 周期性更新 PCC 状态.
    ///
    /// 1. 调用 `reader.read(pcc_id, now_ms)`，失败返回 `Err(SampleFailed)`，`state` 不变。
    /// 2. 防抖逻辑（D6 + T82）：
    ///    - 断路器状态变化 → 重置 `last_change_ms = now_ms`，进入 `Transitioning`
    ///      （`debounce_ms == 0` 时立即稳定，T82）。
    ///    - 状态未变且 `now_ms - last_change_ms >= debounce_ms` →
    ///      `compute_stable_status(new_breaker)`。
    ///    - 状态未变但防抖期内 → 保持 `Transitioning`。
    /// 3. 更新 `state` 的 `breaker_status` / `active_power` / `reactive_power` /
    ///    `power_direction` / `power_factor` 字段（`pcc_id` 不变，T85）。
    /// 4. 返回 `Ok(self.state)`。
    pub fn update(&mut self, now_ms: u64) -> Result<PccState, GridError> {
        let reading = self.reader.read(self.pcc_id, now_ms)?;
        let new_breaker = reading.breaker_status;
        // 防抖逻辑（D6 + T82：debounce_ms == 0 时立即稳定，无 Transitioning 中间态）
        if new_breaker != self.last_breaker_status {
            self.last_breaker_status = new_breaker;
            self.last_change_ms = now_ms;
            if self.debounce_ms == 0 {
                self.state.status = compute_stable_status(new_breaker);
            } else {
                self.state.status = PccStatus::Transitioning;
            }
        } else if now_ms - self.last_change_ms >= self.debounce_ms {
            self.state.status = compute_stable_status(new_breaker);
        } else {
            self.state.status = PccStatus::Transitioning;
        }
        // 更新 state 电气量字段（pcc_id 不被覆盖，T85）
        self.state.breaker_status = new_breaker;
        self.state.active_power = reading.active_power;
        self.state.reactive_power = reading.reactive_power;
        self.state.power_direction = compute_power_direction(reading.active_power);
        self.state.power_factor =
            compute_power_factor(reading.active_power, reading.reactive_power);
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== 辅助函数 =====

    /// 构造 Closed 断路器 + 给定功率的 PccReading.
    fn make_closed_reading(active_power: f32, reactive_power: f32) -> PccReading {
        PccReading {
            breaker_status: BreakerStatus::Closed,
            active_power,
            reactive_power,
        }
    }

    /// 构造给定断路器状态的 PccReading（功率为零）.
    fn make_reading(breaker: BreakerStatus) -> PccReading {
        PccReading {
            breaker_status: breaker,
            active_power: 0.0,
            reactive_power: 0.0,
        }
    }

    // ===== T47: PccState::default() 全字段默认值 =====
    #[test]
    fn t47_pcc_state_default() {
        let s = PccState::default();
        assert_eq!(s.pcc_id, 0);
        assert_eq!(s.breaker_status, BreakerStatus::Unknown);
        assert_eq!(s.power_direction, PowerDirection::Idle);
        assert!(s.power_factor.abs() < 1e-9);
        assert!(s.active_power.abs() < 1e-9);
        assert!(s.reactive_power.abs() < 1e-9);
        assert_eq!(s.status, PccStatus::Transitioning);
    }

    // ===== T48: BreakerStatus::default() == Unknown =====
    #[test]
    fn t48_breaker_status_default_unknown() {
        assert_eq!(BreakerStatus::default(), BreakerStatus::Unknown);
    }

    // ===== T49: PowerDirection::default() == Idle =====
    #[test]
    fn t49_power_direction_default_idle() {
        assert_eq!(PowerDirection::default(), PowerDirection::Idle);
    }

    // ===== T50: PccStatus::default() == Transitioning =====
    #[test]
    fn t50_pcc_status_default_transitioning() {
        assert_eq!(PccStatus::default(), PccStatus::Transitioning);
    }

    // ===== T51: PccReading::default() 全默认 =====
    #[test]
    fn t51_pcc_reading_default() {
        let r = PccReading::default();
        assert_eq!(r.breaker_status, BreakerStatus::Unknown);
        assert!(r.active_power.abs() < 1e-9);
        assert!(r.reactive_power.abs() < 1e-9);
    }

    // ===== T52: MockPccReader::new(reading) fail == false =====
    #[test]
    fn t52_mock_pcc_reader_new_not_fail() {
        let reader = MockPccReader::new(make_closed_reading(10.0, 0.0));
        assert!(!reader.fail);
        assert_eq!(reader.next_reading.breaker_status, BreakerStatus::Closed);
    }

    // ===== T53: MockPccReader::new_failing() fail == true =====
    #[test]
    fn t53_mock_pcc_reader_new_failing() {
        let reader = MockPccReader::new_failing();
        assert!(reader.fail);
    }

    // ===== T54: MockPccReader::with_reading builder =====
    #[test]
    fn t54_mock_pcc_reader_with_reading_builder() {
        let reader =
            MockPccReader::new(PccReading::default()).with_reading(make_closed_reading(5.0, 1.0));
        assert!(!reader.fail);
        assert!((reader.next_reading.active_power - 5.0).abs() < 1e-6);
    }

    // ===== T55: MockPccReader::read() 成功路径 =====
    #[test]
    fn t55_mock_pcc_reader_read_ok() {
        let mut reader = MockPccReader::new(make_closed_reading(10.0, 0.0));
        let result = reader.read(1, 1000);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.breaker_status, BreakerStatus::Closed);
        assert!((r.active_power - 10.0).abs() < 1e-6);
    }

    // ===== T56: MockPccReader::read() 失败路径 =====
    #[test]
    fn t56_mock_pcc_reader_read_err() {
        let mut reader = MockPccReader::new_failing();
        let result = reader.read(1, 1000);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), GridError::SampleFailed);
    }

    // ===== T57: compute_power_direction(10.0) == Import =====
    #[test]
    fn t57_compute_power_direction_import() {
        assert_eq!(compute_power_direction(10.0), PowerDirection::Import);
    }

    // ===== T58: compute_power_direction(-10.0) == Export =====
    #[test]
    fn t58_compute_power_direction_export() {
        assert_eq!(compute_power_direction(-10.0), PowerDirection::Export);
    }

    // ===== T59: compute_power_direction(0.5) == Idle =====
    #[test]
    fn t59_compute_power_direction_idle_positive() {
        assert_eq!(compute_power_direction(0.5), PowerDirection::Idle);
    }

    // ===== T60: compute_power_direction(-0.5) == Idle (|P|≤1.0) =====
    #[test]
    fn t60_compute_power_direction_idle_negative() {
        assert_eq!(compute_power_direction(-0.5), PowerDirection::Idle);
    }

    // ===== T61: compute_power_factor(3.0, 4.0) ≈ 0.6 (3-4-5 triangle) =====
    #[test]
    fn t61_compute_power_factor_3_4_5() {
        let pf = compute_power_factor(3.0, 4.0);
        assert!((pf - 0.6).abs() < 1e-6);
    }

    // ===== T62: compute_power_factor(0.0, 0.0) == 1.0 (避免除零) =====
    #[test]
    fn t62_compute_power_factor_zero_power() {
        assert!((compute_power_factor(0.0, 0.0) - 1.0).abs() < 1e-9);
    }

    // ===== T63: compute_power_factor(0.05, 0.0) == 1.0 (|P|<0.1 阈值) =====
    #[test]
    fn t63_compute_power_factor_below_threshold() {
        assert!((compute_power_factor(0.05, 0.0) - 1.0).abs() < 1e-9);
    }

    // ===== T64: compute_stable_status(Closed) == GridConnected =====
    #[test]
    fn t64_compute_stable_status_closed() {
        assert_eq!(
            compute_stable_status(BreakerStatus::Closed),
            PccStatus::GridConnected
        );
    }

    // ===== T65: compute_stable_status(Open) == Islanded =====
    #[test]
    fn t65_compute_stable_status_open() {
        assert_eq!(
            compute_stable_status(BreakerStatus::Open),
            PccStatus::Islanded
        );
    }

    // ===== T66: compute_stable_status(Tripped) == Islanded =====
    #[test]
    fn t66_compute_stable_status_tripped() {
        assert_eq!(
            compute_stable_status(BreakerStatus::Tripped),
            PccStatus::Islanded
        );
    }

    // ===== T67: compute_stable_status(Unknown) == Transitioning =====
    #[test]
    fn t67_compute_stable_status_unknown() {
        assert_eq!(
            compute_stable_status(BreakerStatus::Unknown),
            PccStatus::Transitioning
        );
    }

    // ===== T68: PccManager::new 初始化 =====
    #[test]
    fn t68_pcc_manager_new() {
        let mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        assert_eq!(mgr.pcc_id, 1);
        assert_eq!(mgr.state.status, PccStatus::Transitioning);
        assert_eq!(mgr.last_breaker_status, BreakerStatus::Unknown);
        assert_eq!(mgr.last_change_ms, 0);
        assert!(!mgr.is_islanded());
    }

    // ===== T69: PccManager::current() 返回 &state =====
    #[test]
    fn t69_pcc_manager_current() {
        let mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        assert_eq!(mgr.current().status, PccStatus::Transitioning);
        assert_eq!(mgr.current().pcc_id, 1);
    }

    // ===== T70: 首次 update(1000) 返回 Closed → Transitioning (防抖期内) =====
    #[test]
    fn t70_pcc_manager_first_update_transitioning() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        let result = mgr.update(1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Transitioning);
    }

    // ===== T71: 第二次 update(1100) 防抖期过 → GridConnected =====
    #[test]
    fn t71_pcc_manager_stable_after_debounce() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        let result = mgr.update(1100);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::GridConnected);
    }

    // ===== T72: 防抖期后 Open → Islanded / is_islanded() == true =====
    #[test]
    fn t72_pcc_manager_open_islanded() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_reading(BreakerStatus::Open))),
            100,
        );
        mgr.update(1000).unwrap();
        let result = mgr.update(1100);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Islanded);
        assert!(mgr.is_islanded());
    }

    // ===== T73: 防抖期后 Tripped → Islanded =====
    #[test]
    fn t73_pcc_manager_tripped_islanded() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_reading(BreakerStatus::Tripped))),
            100,
        );
        mgr.update(1000).unwrap();
        let result = mgr.update(1100);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Islanded);
    }

    // ===== T74: 防抖期后 Unknown → Transitioning =====
    #[test]
    fn t74_pcc_manager_unknown_transitioning() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_reading(BreakerStatus::Unknown))),
            100,
        );
        // Unknown == last_breaker_status(Unknown) → 第二分支
        // 1000 - 0 = 1000 >= 100 → compute_stable_status(Unknown) = Transitioning
        let result = mgr.update(1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Transitioning);
    }

    // ===== T75: update reader 失败 → Err(SampleFailed) / state 不变 =====
    #[test]
    fn t75_pcc_manager_update_read_fail() {
        let mut mgr = PccManager::new(1, Box::new(MockPccReader::new_failing()), 100);
        let result = mgr.update(1000);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), GridError::SampleFailed);
        // state 未被修改
        assert_eq!(mgr.state.status, PccStatus::Transitioning);
        assert_eq!(mgr.last_breaker_status, BreakerStatus::Unknown);
        assert_eq!(mgr.last_change_ms, 0);
    }

    // ===== T76: 稳态 Closed 后切换 Open → Transitioning (防抖重置) =====
    #[test]
    fn t76_pcc_manager_breaker_change_resets_debounce() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap(); // Closed != Unknown → Transitioning
        mgr.update(1100).unwrap(); // Closed == Closed, 100>=100 → GridConnected
        assert_eq!(mgr.state.status, PccStatus::GridConnected);
        // 切换 reader 返回 Open
        mgr.reader = Box::new(MockPccReader::new(make_reading(BreakerStatus::Open)));
        let result = mgr.update(1200); // Open != Closed → Transitioning (防抖重置)
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Transitioning);
        assert_eq!(mgr.last_breaker_status, BreakerStatus::Open);
        assert_eq!(mgr.last_change_ms, 1200);
    }

    // ===== T77: update 功率方向 Import (P=10.0) =====
    #[test]
    fn t77_pcc_manager_power_direction_import() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(10.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        assert_eq!(mgr.state.power_direction, PowerDirection::Import);
    }

    // ===== T78: update 功率方向 Export (P=-10.0) =====
    #[test]
    fn t78_pcc_manager_power_direction_export() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(-10.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        assert_eq!(mgr.state.power_direction, PowerDirection::Export);
    }

    // ===== T79: update 功率方向 Idle (P=0.5) =====
    #[test]
    fn t79_pcc_manager_power_direction_idle() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.5, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        assert_eq!(mgr.state.power_direction, PowerDirection::Idle);
    }

    // ===== T80: update 功率因数 (P=3.0, Q=4.0 → ≈ 0.6) =====
    #[test]
    fn t80_pcc_manager_power_factor_3_4_5() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(3.0, 4.0))),
            100,
        );
        mgr.update(1000).unwrap();
        assert!((mgr.state.power_factor - 0.6).abs() < 1e-6);
    }

    // ===== T81: update 功率因数 (P=0.0, Q=0.0 → 1.0) =====
    #[test]
    fn t81_pcc_manager_power_factor_zero() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        assert!((mgr.state.power_factor - 1.0).abs() < 1e-9);
    }

    // ===== T82: debounce_ms=0 首次 update 立即稳定 (无 Transitioning 中间态) =====
    #[test]
    fn t82_pcc_manager_debounce_zero_immediate_stable() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            0,
        );
        let result = mgr.update(1000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::GridConnected);
    }

    // ===== T83: 两次连续相同 breaker_status，第二次防抖期内保持 Transitioning =====
    #[test]
    fn t83_pcc_manager_consecutive_same_in_debounce() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap(); // Closed != Unknown → Transitioning, last_change_ms=1000
        assert_eq!(mgr.state.status, PccStatus::Transitioning);
        let result = mgr.update(1050); // Closed == Closed, 50 < 100 → Transitioning
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, PccStatus::Transitioning);
    }

    // ===== T84: is_islanded() 在 GridConnected 时返回 false =====
    #[test]
    fn t84_pcc_manager_is_islanded_false_when_connected() {
        let mut mgr = PccManager::new(
            1,
            Box::new(MockPccReader::new(make_closed_reading(0.0, 0.0))),
            100,
        );
        mgr.update(1000).unwrap();
        mgr.update(1100).unwrap();
        assert_eq!(mgr.state.status, PccStatus::GridConnected);
        assert!(!mgr.is_islanded());
    }

    // ===== T85: update 后 state.pcc_id 始终等于构造时传入的 pcc_id =====
    #[test]
    fn t85_pcc_manager_pcc_id_unchanged() {
        let mut mgr = PccManager::new(
            42,
            Box::new(MockPccReader::new(make_closed_reading(10.0, 5.0))),
            100,
        );
        assert_eq!(mgr.state.pcc_id, 42);
        mgr.update(1000).unwrap();
        assert_eq!(mgr.state.pcc_id, 42);
        mgr.update(1100).unwrap();
        assert_eq!(mgr.state.pcc_id, 42);
    }

    // ===== T86: PccState 派生 Copy 可复制 =====
    #[test]
    fn t86_pcc_state_copy() {
        let s1 = PccState {
            pcc_id: 7,
            breaker_status: BreakerStatus::Closed,
            power_direction: PowerDirection::Import,
            power_factor: 0.95,
            active_power: 10.0,
            reactive_power: 3.0,
            status: PccStatus::GridConnected,
        };
        let s2 = s1; // Copy
        assert_eq!(s1, s2);
    }
}
