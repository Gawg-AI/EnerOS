//! Power-loss detection via dual-redundant ADC + GPIO.
//!
//! On aarch64, the main power rail is monitored by two independent paths:
//! 1. **ADC comparison** — reads the main supply voltage and compares against
//!    a threshold (e.g. 4.75 V for a 5 V rail).
//! 2. **GPIO interrupt** — a dedicated power-fail signal from the PMIC triggers
//!    a level-triggered interrupt.
//!
//! Both paths must agree before power is considered OK. This dual-redundancy
//! prevents false positives from ADC noise or GPIO glitches.
//!
//! Detection latency target: **< 10 ms** from mains failure to callback.
//! (Host tests do not verify timing.)

use spin::Mutex;

// ============================================================================
// Callback registration
// ============================================================================

/// Callback registered by the system for power-loss interrupt handling.
/// Called by [`notify_power_loss`] when the hardware (or mock) detects loss.
pub(crate) static POWER_IRQ_CALLBACK: Mutex<Option<fn()>> = Mutex::new(None);

/// Register the power-loss interrupt callback.
///
/// The callback is invoked from [`notify_power_loss`] when power loss is
/// detected. It should initiate the shutdown sequence via `on_power_loss`.
pub fn register_power_irq(callback: fn()) {
    *POWER_IRQ_CALLBACK.lock() = Some(callback);
}

// ============================================================================
// Power state notification
// ============================================================================

/// Called by the hardware interrupt handler (or host mock) when main power
/// is lost. Updates global state and invokes the registered callback.
///
/// v0.216.0（B1-04）：记录主→备切换事件。
pub fn notify_power_loss() {
    record_power_switch(PowerSource::Backup);
    {
        let mut state = crate::POWER_STATE.lock();
        state.main_power_ok = false;
        state.in_shutdown = true;
    }
    // Invoke callback outside the state lock to avoid re-entrancy deadlock.
    if let Some(cb) = *POWER_IRQ_CALLBACK.lock() {
        cb();
    }
}

/// Called when main power is restored. Cancels the shutdown sequence by
/// clearing `in_shutdown`. This is the authorized cancellation path —
/// normal tasks cannot cancel via [`advance_sequence`](crate::advance_sequence).
///
/// v0.216.0（B1-04）：记录备→主切换事件。
pub fn notify_power_restored() {
    record_power_switch(PowerSource::Main);
    let mut state = crate::POWER_STATE.lock();
    state.main_power_ok = true;
    state.in_shutdown = false;
}

// ============================================================================
// Power status check — aarch64 hardware implementation
// ============================================================================

/// 电源域检测抽象（v0.216.0，B1-04）。
///
/// 替代此前的虚构 MMIO 地址（0x0906_0000/0x0907_0000 并非真实 ADC/GPIO，
/// 08 号文 B2）：电源域/电池检测经 [`PowerRail`] 注入，真机用寄存器级
/// 实现，host 用 [`RamPowerRail`] 模拟。
pub trait PowerRail: Send + Sync {
    /// 读取主供电电压（毫伏）。
    fn read_voltage_mv(&self) -> u32;
    /// 读取 power-fail 信号（true = 掉电标志断言）。
    fn read_power_fail(&self) -> bool;
    /// 读取电池电量（0–100 %），无电池返回 None。
    fn battery_soc(&self) -> Option<u8>;
}

/// RAM 模拟电源域（host 测试 / QEMU）。
pub struct RamPowerRail {
    /// 当前电压（毫伏）。
    pub voltage_mv: u32,
    /// power-fail 标志。
    pub power_fail: bool,
    /// 电池电量。
    pub battery_soc: Option<u8>,
}

impl RamPowerRail {
    /// 创建模拟电源域。
    pub fn new(voltage_mv: u32, battery_soc: Option<u8>) -> Self {
        Self {
            voltage_mv,
            power_fail: false,
            battery_soc,
        }
    }
}

impl PowerRail for RamPowerRail {
    fn read_voltage_mv(&self) -> u32 {
        self.voltage_mv
    }
    fn read_power_fail(&self) -> bool {
        self.power_fail
    }
    fn battery_soc(&self) -> Option<u8> {
        self.battery_soc
    }
}

/// 全局电源域（由板级初始化注入真实实现；默认 RAM 模拟）。
static POWER_RAIL: Mutex<Option<&'static dyn PowerRail>> = Mutex::new(None);

/// 注入电源域实现（板级初始化调用一次）。
pub fn register_power_rail(rail: &'static dyn PowerRail) {
    *POWER_RAIL.lock() = Some(rail);
}

/// 读取当前电压（毫伏）；未注入电源域时返回 0。
pub fn read_voltage_mv() -> u32 {
    match *POWER_RAIL.lock() {
        Some(rail) => rail.read_voltage_mv(),
        None => 0,
    }
}

/// 读取当前电池电量（0–100 %）。
pub fn read_battery_soc() -> Option<u8> {
    match *POWER_RAIL.lock() {
        Some(rail) => rail.battery_soc(),
        None => None,
    }
}

/// 主备电切换事件（v0.216.0，B1-04）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    /// 主电（AC/主供电轨）
    Main,
    /// 备电（电池/UPS）
    Backup,
}

/// 最近一次主备电切换事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSwitchEvent {
    /// 切换到的电源源。
    pub source: PowerSource,
    /// 切换时电压（毫伏）。
    pub voltage_mv: u32,
    /// 切换时电池电量（%）。
    pub battery_soc: u8,
}

/// 最近一次切换事件（初始为主电）。
static LAST_SWITCH_EVENT: Mutex<Option<PowerSwitchEvent>> = Mutex::new(None);

/// 查询最近一次主备电切换事件。
pub fn last_power_switch_event() -> Option<PowerSwitchEvent> {
    *LAST_SWITCH_EVENT.lock()
}

/// 记录主备电切换事件（由检测路径调用）。
pub fn record_power_switch(source: PowerSource) {
    let voltage_mv = read_voltage_mv();
    let battery_soc = read_battery_soc().unwrap_or(0);
    *LAST_SWITCH_EVENT.lock() = Some(PowerSwitchEvent {
        source,
        voltage_mv,
        battery_soc,
    });
}

/// 主电源 OK 电压阈值（4.75 V @ 5 V rail）。
pub const POWER_OK_THRESHOLD_MV: u32 = 4750;

// 31 DR-06：**零硬件依赖红线**——aarch64 虚构 ADC/GPIO 地址（0x0906_0000/
// 0x0907_0000 并非真实设备）已整体删除。电源检测唯一入口 =
// [`PowerRail`] 注入（真机 ADC/GPIO 作为 `impl PowerRail` 板级注入，
// 基址来自 FDT，走绑定表 map_mmio——对齐 PL011 模式）；未注入时
// fail-safe 回退缓存状态（不读虚构地址）。

// ============================================================================
// Power status check — PowerRail 注入（单一实现，host/真机一致）
// ============================================================================

/// 检查主电源是否正常（31 DR-06：单一实现，host/真机一致）。
///
/// 已注入 [`PowerRail`] 时以真实检测为准（电压阈值 + power-fail 信号）；
/// 未注入时 fail-safe 回退缓存状态。
pub fn is_main_power_ok() -> bool {
    let rail = *POWER_RAIL.lock();
    match rail {
        Some(rail) => rail.read_voltage_mv() >= POWER_OK_THRESHOLD_MV && !rail.read_power_fail(),
        None => crate::POWER_STATE.lock().main_power_ok,
    }
}

/// Host mock: simulate main power status change.
///
/// When `ok` is `false`, calls [`notify_power_loss`] (updates state + invokes
/// callback). When `ok` is `true`, calls [`notify_power_restored`] (cancels
/// shutdown).
#[cfg(not(target_arch = "aarch64"))]
pub fn set_main_power_ok(ok: bool) {
    if ok {
        notify_power_restored();
    } else {
        notify_power_loss();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());
    static CALLBACK_CALLED: StdMutex<bool> = StdMutex::new(false);

    fn reset_state() {
        *crate::POWER_STATE.lock() = crate::PowerState {
            main_power_ok: true,
            ups_soc: 100,
            last_checkpoint: core::time::Duration::from_millis(0),
            in_shutdown: false,
        };
        *POWER_IRQ_CALLBACK.lock() = None;
        // v0.216.0：清除注入的电源域，避免测试间泄漏。
        *POWER_RAIL.lock() = None;
        *LAST_SWITCH_EVENT.lock() = None;
    }

    fn test_callback() {
        *CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    #[test]
    fn test_register_power_irq() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        *CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()) = false;

        register_power_irq(test_callback);
        assert!(POWER_IRQ_CALLBACK.lock().is_some());

        // Trigger power loss — callback should fire.
        notify_power_loss();
        assert!(*CALLBACK_CALLED.lock().unwrap_or_else(|e| e.into_inner()));
    }

    #[test]
    fn test_notify_power_loss_updates_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        assert!(is_main_power_ok());
        notify_power_loss();
        assert!(!is_main_power_ok());
        assert!(crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_notify_power_restored_updates_state() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        notify_power_loss();
        assert!(!is_main_power_ok());

        notify_power_restored();
        assert!(is_main_power_ok());
        assert!(!crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_set_main_power_ok_host_mock() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        set_main_power_ok(false);
        assert!(!is_main_power_ok());
        assert!(crate::POWER_STATE.lock().in_shutdown);

        set_main_power_ok(true);
        assert!(is_main_power_ok());
        assert!(!crate::POWER_STATE.lock().in_shutdown);
    }

    #[test]
    fn test_no_callback_no_panic() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        // No callback registered — notify should not panic.
        notify_power_loss();
        assert!(!is_main_power_ok());
    }

    #[test]
    fn test_register_overwrites_previous_callback() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();

        fn first_cb() {}
        fn second_cb() {}

        register_power_irq(first_cb);
        register_power_irq(second_cb);

        // Latest registration wins.
        let cb = POWER_IRQ_CALLBACK.lock();
        assert!(cb.is_some());
    }

    // ===== v0.216.0（B1-04）：PowerRail 真实检测 + 主备电切换事件 =====

    /// 静态模拟电源域（测试注入）。
    static TEST_RAIL: RamPowerRail = RamPowerRail {
        voltage_mv: 5000,
        power_fail: false,
        battery_soc: Some(85),
    };

    #[test]
    fn test_power_rail_voltage_ok() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        register_power_rail(&TEST_RAIL);
        assert!(is_main_power_ok());
        assert_eq!(read_voltage_mv(), 5000);
        assert_eq!(read_battery_soc(), Some(85));
    }

    #[test]
    fn test_power_rail_voltage_below_threshold() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        // 电压低于 4.75V 阈值 → 主电不 OK（真实检测路径）。
        static LOW_RAIL: RamPowerRail = RamPowerRail {
            voltage_mv: 4500,
            power_fail: false,
            battery_soc: Some(80),
        };
        register_power_rail(&LOW_RAIL);
        assert!(!is_main_power_ok());
    }

    #[test]
    fn test_power_rail_power_fail_signal() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        static FAIL_RAIL: RamPowerRail = RamPowerRail {
            voltage_mv: 5000,
            power_fail: true,
            battery_soc: Some(80),
        };
        register_power_rail(&FAIL_RAIL);
        assert!(!is_main_power_ok(), "power-fail 信号断言 → 主电不 OK");
    }

    #[test]
    fn test_power_switch_event_recorded_on_loss_and_restore() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        register_power_rail(&TEST_RAIL);

        // 主电丢失：记录 Backup 切换事件（含电压/电量快照）。
        notify_power_loss();
        let ev = last_power_switch_event().expect("switch event");
        assert_eq!(ev.source, PowerSource::Backup);
        assert_eq!(ev.voltage_mv, 5000);
        assert_eq!(ev.battery_soc, 85);

        // 主电恢复：记录 Main 切换事件。
        notify_power_restored();
        let ev = last_power_switch_event().expect("switch event");
        assert_eq!(ev.source, PowerSource::Main);
        assert_eq!(ev.voltage_mv, 5000);
    }

    #[test]
    fn test_record_power_switch_without_rail_defaults() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_state();
        // 未注入 PowerRail：电压/电量回退 0。
        record_power_switch(PowerSource::Backup);
        let ev = last_power_switch_event().expect("switch event");
        assert_eq!(ev.source, PowerSource::Backup);
        assert_eq!(ev.voltage_mv, 0);
        assert_eq!(ev.battery_soc, 0);
    }
}
