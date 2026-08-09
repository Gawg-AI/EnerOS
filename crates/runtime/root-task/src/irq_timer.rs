//! WP-26.8：QEMU virt ARM 虚拟定时器使能（中断源的最小活体证据）。
//!
//! QEMU virt 默认使能 arch timer；EL1 访问虚拟 timer 需置
//! `CNTV_CTL_EL0.ENABLE` 并设置 `CNTV_TVAL_EL0` 倒数周期。
//! 频率：QEMU virt `timebase-frequency`（virt 默认 62.5 MHz）。
//! 周期取 100_000_000 ticks ≈ 1.6 s —— 运行数秒即可观察到中断。

/// 虚拟定时器倒数周期（ticks；62.5 MHz 下 ≈ 1.6 s）。
pub const VIRT_TIMER_TICKS: u64 = 100_000_000;

/// 使能 EL1 虚拟定时器（CNTV_CTL_EL0.ENABLE=1，CNTV_TVAL_EL0 设周期）。
#[cfg(target_os = "none")]
pub fn enable_arch_timer_virtual() {
    // SAFETY: 系统寄存器写（EL1 可访问）；ENABLE=1 位、TVAL 为周期倒数；
    // 不触碰其他控制位（ISTATUS/IMASK 保持 0——IMASK=0 即允许中断）。
    unsafe {
        core::arch::asm!(
            "msr cntv_ctl_el0, {ctl}",
            "msr cntv_tval_el0, {tval}",
            "isb",
            ctl = in(reg) 1u64,
            tval = in(reg) VIRT_TIMER_TICKS,
        );
    }
}
