//! EnerOS v0.84.0 Grid Agent — 孤岛检测模块.
//!
//! 双源融合孤岛检测（PCC breaker 主源 + GridState 频率/电压辅源）+ 连续确认逻辑.
//!
//! 在 v0.82.0 [`crate::GridState`] + v0.83.0 [`crate::PccState`] 基础上实现孤岛
//! （Islanding）检测：主源为 PCC 断路器状态（[`PccStatus::Islanded`]），辅源为
//! 电网频率/电压越限（防主源失效或通信丢失场景）。为避免瞬态扰动导致误判，
//! 引入连续确认计数（`consecutive_count`），仅在连续 `confirmation_threshold`
//! 次检测到异常时才判定为 [`IslandResult::Islanded`]。
//!
//! # 核心类型
//!
//! - [`IslandResult`] — 检测结果（Islanded / GridOk / Uncertain，默认 GridOk）
//! - [`IslandConfig`] — 检测配置（频率/电压上下限 + 确认阈值）
//! - [`IslandDetector`] — 检测器（持有配置 + 连续计数，`detect()` 周期性调用）
//!
//! # no_std 合规
//!
//! 仅使用 `core::*`（无 `alloc`/`std`），无 `async` / `panic!` / `unsafe` /
//! `todo!` / `unimplemented!` / `Instant::now()`。`no_std` 属性继承自 `lib.rs`。
//!
//! # v0.84.0 偏差声明 (D1~D14)
//!
//! | 偏差 | 蓝图原文 | 本版本处理 | 理由 |
//! |------|---------|-----------|------|
//! | D1 | async fn detect() | sync detect(&PccState, &GridState) | no_std 无 async runtime；沿用 v0.82/v0.83 sync 模式 |
//! | D2 | 三源融合（PCC + 频率 + 电压 + ROCOF） | 双源（PCC + 频率/电压） | MVP 收敛；ROCOF 需历史窗口，后置 |
//! | D3 | 独立错误类型 | 无错误返回（IslandResult::Uncertain 兜底） | surgical — 不新增错误类型，复用 GridError 在调用侧 |
//! | D4 | 嵌入 GridAgent | 独立 IslandDetector 组件 | surgical — 不破坏 v0.82.0 GridAgent；与 PccManager 模式一致 |
//! | D5 | Instant::now() 时戳 | 外部驱动（调用方提供 GridState/PccState） | no_std 无 Instant；时戳由 sampler/pcc_manager 外部提供 |
//! | D6 | 阈值配置文件 | IslandConfig 结构体（编译期/构造期配置） | no_std 无文件系统；与 v0.83 debounce_ms 模式一致 |
//! | D7 | f32::sqrt / libm | 无（检测仅用比较运算） | 检测逻辑无开方需求 |
//! | D8 | docs/phase2/ + config/ | docs/agents/ + 内嵌单元测试 | 工作区规则 §2.3.3；沿用 v0.82/v0.83 测试模式 |
//! | D9 | 集成测试 tests/ | island_detect.rs 内嵌单元测试 | 沿用 v0.82/v0.83 模式 |
//! | D10 | IslandResult 4 变体（含 Timeout） | 3 变体 | MVP 收敛；超时在调用侧归一为 Uncertain |
//! | D11 | confirmation_threshold 默认 5 | 默认 3 | 工业场景 3 次确认足够防抖（10ms×3=30ms < 50ms 控制周期） |
//! | D12 | 频率阈值 ±0.5Hz (49.5~50.5) | 保持 | §8.5 电网频率规范 |
//! | D13 | 电压阈值 ±10% (198~242V) | 200~240V 简化 | MVP 取整；与 v0.82 is_valid_grid 阈值对齐 |
//! | D14 | IslandDetector Copy | 非 Copy（持有可变 count 状态） | 避免意外 Copy 后状态分裂 |

use crate::GridState;
use crate::PccState;
use crate::PccStatus;

// ===== Enums =====

/// 孤岛检测结果.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IslandResult {
    /// 离网（连续确认计数达到 `confirmation_threshold`）.
    Islanded,
    /// 并网正常（连续计数为 0）.
    #[default]
    GridOk,
    /// 不确定（检测到异常但未达确认阈值，`0 < count < threshold`）.
    Uncertain,
}

// ===== IslandConfig =====

/// 孤岛检测配置.
///
/// 频率/电压上下限定义辅源越限阈值；`confirmation_threshold` 定义连续确认次数。
/// 默认值：阈值 3 / 频率 49.5~50.5 Hz / 电压 200.0~240.0 V。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandConfig {
    /// 连续确认阈值（达到后判定为 [`IslandResult::Islanded`]）.
    pub confirmation_threshold: u32,
    /// 频率下限（Hz）.
    pub freq_min: f32,
    /// 频率上限（Hz）.
    pub freq_max: f32,
    /// 电压下限（V）.
    pub voltage_min: f32,
    /// 电压上限（V）.
    pub voltage_max: f32,
}

impl Default for IslandConfig {
    /// 默认配置：阈值 3 / 频率 49.5~50.5 Hz / 电压 200.0~240.0 V.
    ///
    /// 手动实现而非 `#[derive(Default)]`，因 `f32::default() == 0.0` 会给出
    /// 错误的频率/电压默认值（D11~D13）。
    fn default() -> Self {
        IslandConfig {
            confirmation_threshold: 3,
            freq_min: 49.5,
            freq_max: 50.5,
            voltage_min: 200.0,
            voltage_max: 240.0,
        }
    }
}

// ===== IslandDetector =====

/// 孤岛检测器（双源融合 + 连续确认）.
///
/// 持有 [`IslandConfig`] 配置与 `consecutive_count` 连续异常计数。由外部调度器
/// 周期性调用 [`detect`](Self::detect) 方法，输入当前 [`PccState`]（主源）与
/// [`GridState`]（辅源），返回 [`IslandResult`]。
///
/// 非 `Copy`（D14）：持有可变计数状态，意外 Copy 会导致状态分裂。
#[derive(Debug, Clone, PartialEq)]
pub struct IslandDetector {
    /// 检测配置.
    pub config: IslandConfig,
    /// 当前连续异常计数（每次 `detect` 检测到异常 +1，正常归 0）.
    pub consecutive_count: u32,
}

impl IslandDetector {
    /// 创建检测器（指定配置），`consecutive_count = 0`.
    pub fn new(config: IslandConfig) -> Self {
        IslandDetector {
            config,
            consecutive_count: 0,
        }
    }

    /// 创建检测器（使用 [`IslandConfig::default`]）.
    pub fn new_default() -> Self {
        IslandDetector::new(IslandConfig::default())
    }

    /// 返回当前连续异常计数.
    pub fn current_count(&self) -> u32 {
        self.consecutive_count
    }

    /// 重置连续异常计数为 0.
    pub fn reset(&mut self) {
        self.consecutive_count = 0;
    }

    /// 执行一次孤岛检测.
    ///
    /// # 检测逻辑
    ///
    /// 1. **主源**：`pcc.status == PccStatus::Islanded` → 主源判定离网。
    /// 2. **辅源**：`grid.frequency` 越限 `[freq_min, freq_max]` 或
    ///    `grid.voltage_a` 越限 `[voltage_min, voltage_max]` → 辅源判定离网。
    /// 3. **融合**：`raw_islanded = pcc_islanded || freq_out || volt_out`。
    /// 4. **连续确认**：`raw_islanded == true` → `consecutive_count += 1`；
    ///    否则 `consecutive_count = 0`（立即归零，无防抖恢复）。
    /// 5. **结果映射**：
    ///    - `count >= confirmation_threshold` → [`IslandResult::Islanded`]
    ///    - `0 < count < threshold` → [`IslandResult::Uncertain`]
    ///    - `count == 0` → [`IslandResult::GridOk`]
    pub fn detect(&mut self, pcc: &PccState, grid: &GridState) -> IslandResult {
        // 主源: PCC breaker
        let pcc_islanded = pcc.status == PccStatus::Islanded;
        // 辅源: 频率/电压越限
        let freq_out =
            grid.frequency < self.config.freq_min || grid.frequency > self.config.freq_max;
        let volt_out =
            grid.voltage_a < self.config.voltage_min || grid.voltage_a > self.config.voltage_max;
        let raw_islanded = pcc_islanded || freq_out || volt_out;

        if raw_islanded {
            self.consecutive_count += 1;
        } else {
            self.consecutive_count = 0;
        }

        if self.consecutive_count >= self.config.confirmation_threshold {
            IslandResult::Islanded
        } else if self.consecutive_count > 0 {
            IslandResult::Uncertain
        } else {
            IslandResult::GridOk
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // ===== T87: IslandConfig::default() 全字段默认值 =====
    #[test]
    fn t87_island_config_default() {
        let cfg = IslandConfig::default();
        assert_eq!(cfg.confirmation_threshold, 3);
        assert!((cfg.freq_min - 49.5).abs() < 1e-6);
        assert!((cfg.freq_max - 50.5).abs() < 1e-6);
        assert!((cfg.voltage_min - 200.0).abs() < 1e-6);
        assert!((cfg.voltage_max - 240.0).abs() < 1e-6);
    }

    // ===== T88: IslandResult::default() == GridOk =====
    #[test]
    fn t88_island_result_default_grid_ok() {
        assert_eq!(IslandResult::default(), IslandResult::GridOk);
    }

    // ===== T89: IslandDetector::new_default() consecutive_count == 0 =====
    #[test]
    fn t89_island_detector_new_default() {
        let d = IslandDetector::new_default();
        assert_eq!(d.consecutive_count, 0);
        assert_eq!(d.config, IslandConfig::default());
    }

    // ===== T90: PCC Islanded 首次 detect → Uncertain (count=1 < 3) =====
    #[test]
    fn t90_pcc_islanded_first_uncertain() {
        let mut d = IslandDetector::new_default();
        let r = d.detect(&pcc_islanded(), &grid_normal());
        assert_eq!(r, IslandResult::Uncertain);
        assert_eq!(d.current_count(), 1);
    }

    // ===== T91: PCC Islanded 连续 3 次 detect → 第 3 次返回 Islanded =====
    #[test]
    fn t91_pcc_islanded_three_consecutive_islanded() {
        let mut d = IslandDetector::new_default();
        for _ in 0..2 {
            let _ = d.detect(&pcc_islanded(), &grid_normal());
        }
        let r = d.detect(&pcc_islanded(), &grid_normal());
        assert_eq!(r, IslandResult::Islanded);
        assert_eq!(d.current_count(), 3);
    }

    // ===== T92: PCC GridConnected 但频率=49.0 → 连续 3 次 → Islanded (辅源 freq_min) =====
    #[test]
    fn t92_aux_source_freq_low_triggers_islanded() {
        let mut d = IslandDetector::new_default();
        let mut grid = grid_normal();
        grid.frequency = 49.0;
        for _ in 0..2 {
            let _ = d.detect(&pcc_connected(), &grid);
        }
        let r = d.detect(&pcc_connected(), &grid);
        assert_eq!(r, IslandResult::Islanded);
    }

    // ===== T93: PCC GridConnected 但电压=180.0 → 连续 3 次 → Islanded (辅源 voltage_min) =====
    #[test]
    fn t93_aux_source_voltage_low_triggers_islanded() {
        let mut d = IslandDetector::new_default();
        let mut grid = grid_normal();
        grid.frequency = 50.0;
        grid.voltage_a = 180.0;
        for _ in 0..2 {
            let _ = d.detect(&pcc_connected(), &grid);
        }
        let r = d.detect(&pcc_connected(), &grid);
        assert_eq!(r, IslandResult::Islanded);
    }

    // ===== T94: 2 次 Uncertain 后第 3 次输入 GridConnected + 正常电网 → GridOk (count 归零) =====
    #[test]
    fn t94_count_reset_on_normal_input() {
        let mut d = IslandDetector::new_default();
        let _ = d.detect(&pcc_islanded(), &grid_normal());
        let _ = d.detect(&pcc_islanded(), &grid_normal());
        let r = d.detect(&pcc_connected(), &grid_normal());
        assert_eq!(r, IslandResult::GridOk);
        assert_eq!(d.current_count(), 0);
    }

    // ===== T95: IslandConfig { confirmation_threshold: 1, .. } + PCC Islanded → 首次即 Islanded =====
    #[test]
    fn t95_threshold_one_first_islanded() {
        let cfg = IslandConfig {
            confirmation_threshold: 1,
            ..IslandConfig::default()
        };
        let mut d = IslandDetector::new(cfg);
        let r = d.detect(&pcc_islanded(), &grid_normal());
        assert_eq!(r, IslandResult::Islanded);
    }

    // ===== T96: PCC GridConnected + 正常电网 → GridOk (count 保持 0) =====
    #[test]
    fn t96_pcc_connected_normal_grid_ok() {
        let mut d = IslandDetector::new_default();
        let r = d.detect(&pcc_connected(), &grid_normal());
        assert_eq!(r, IslandResult::GridOk);
        assert_eq!(d.current_count(), 0);
    }

    // ===== T96b: current_count() 在 Uncertain 后正确; reset() 归零 =====
    #[test]
    fn t96b_current_count_and_reset() {
        let mut d = IslandDetector::new_default();
        let _ = d.detect(&pcc_islanded(), &grid_normal());
        assert_eq!(d.current_count(), 1);
        d.reset();
        assert_eq!(d.current_count(), 0);
    }

    // ===== T96c: 频率=51.0 (>50.5) 触发辅源检测 (3 次 → Islanded) =====
    #[test]
    fn t96c_aux_source_freq_high_triggers_islanded() {
        let mut d = IslandDetector::new_default();
        let mut grid = grid_normal();
        grid.frequency = 51.0;
        for _ in 0..2 {
            let _ = d.detect(&pcc_connected(), &grid);
        }
        let r = d.detect(&pcc_connected(), &grid);
        assert_eq!(r, IslandResult::Islanded);
    }

    // ===== T96d: 电压=260.0 (>240.0) 触发辅源检测 (3 次 → Islanded) =====
    #[test]
    fn t96d_aux_source_voltage_high_triggers_islanded() {
        let mut d = IslandDetector::new_default();
        let mut grid = grid_normal();
        grid.voltage_a = 260.0;
        for _ in 0..2 {
            let _ = d.detect(&pcc_connected(), &grid);
        }
        let r = d.detect(&pcc_connected(), &grid);
        assert_eq!(r, IslandResult::Islanded);
    }
}
