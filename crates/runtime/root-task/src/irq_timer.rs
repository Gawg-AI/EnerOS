//! WP-26.8：QEMU virt ARM 定时器使能（中断源的最小活体证据）。
//!
//! 33 WP-33.5（中断计数转真）：**虚拟定时器 IRQ 27 被 seL4 内核占用**
//! （无 CONFIG_ARM_HYPERVISOR_SUPPORT 时 KERNEL_TIMER_IRQ=27，QEMU 活体
//! `IRQ bind failed: Sel4(Other(9))` = seL4_RevokeFirst = isIRQActive）——
//! 改用**物理定时器**（FDT phys-nonsecure PPI 14 → IRQ 30，内核未占用；
//! EL1 可访问 CNTP_CTL_EL0/CNTP_TVAL_EL0）。
//!
//! 频率：QEMU virt `timebase-frequency`（virt 默认 62.5 MHz）。
//! 周期取 100_000_000 ticks ≈ 1.6 s —— 运行数秒即可观察到中断。

/// 物理定时器倒数周期（ticks；62.5 MHz 下 ≈ 1.6 s）。
pub const PHYS_TIMER_TICKS: u64 = 100_000_000;

/// 使能 EL1 物理定时器（CNTP_CTL_EL0.ENABLE=1，CNTP_TVAL_EL0 设周期）。
#[cfg(target_os = "none")]
pub fn enable_arch_timer_phys() {
    // SAFETY: 系统寄存器写（EL1 可访问）；ENABLE=1 位、TVAL 为周期倒数；
    // 不触碰其他控制位（ISTATUS/IMASK 保持 0——IMASK=0 即允许中断）。
    unsafe {
        core::arch::asm!(
            "msr cntp_ctl_el0, {ctl}",
            "msr cntp_tval_el0, {tval}",
            "isb",
            ctl = in(reg) 1u64,
            tval = in(reg) PHYS_TIMER_TICKS,
        );
    }
}
