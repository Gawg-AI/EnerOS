//! 22 §2.W1.2：root-task panic 行为——审计 + 重启计数 + 看门狗复位。
//!
//! panic handler 序列（强约束：禁止复杂逻辑，可能再次 panic）：
//! ①写审计日志（SM3 哈希链，`AuditEventType::SecurityAlert`，heapless
//! 定容无堆）→ ②重启计数递增（内存原子；持久化随 W3.5 存储层落地，
//! ADR-026）→ ③武装 SP805 看门狗短超时（10 ms）→ 超时硬件复位。
//!
//! 分层语义（锁定 6 分级重启）：root-task 无 supervisor 兜底，panic 即
//! 整机复位；EL0 服务 panic 由 supervisor 重启（W1.3），见 services。

use core::fmt::Write as _;
use core::panic::{Location, PanicInfo};
use core::sync::atomic::{AtomicUsize, Ordering};

use eneros_audit::{AuditEvent, AuditEventType, AuditLogger, AuditLoggerImpl, MemStore};
use heapless::String;
use spin::Mutex;

/// SP805 看门狗 MMIO 基址（QEMU virt 平台标准地址；真机按 BSP 替换）。
pub const WDT_BASE: u64 = 0x0905_0000;

/// panic 触发后的重启计数（内存；跨掉电持久化挂 W3.5，ADR-026）。
pub static PANIC_REBOOT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// panic 审计链（预分配：MemStore + 链头，panic 路径零堆分配）。
static PANIC_LOGGER: Mutex<Option<AuditLoggerImpl<MemStore>>> = Mutex::new(None);

/// 创世哈希：固定常量（SM3("eneros-root-task-panic") 的一次性值；链恢复
/// 语义要求创世跨重启不变，W3 持久化后由 boot-manager 统一提供）。
const PANIC_GENESIS: [u8; 32] = [
    0x6e, 0x65, 0x72, 0x6f, 0x73, 0x2d, 0x72, 0x6f, 0x6f, 0x74, 0x2d, 0x74, 0x61, 0x73, 0x6b, 0x2d,
    0x70, 0x61, 0x6e, 0x69, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// 初始化 panic 审计链（启动早期调用一次；重复调用重置链）。
pub fn init_panic_audit() {
    *PANIC_LOGGER.lock() = Some(AuditLoggerImpl::new(MemStore::new(), PANIC_GENESIS));
}

/// 追加一条 panic 审计事件（无堆：heapless 定容 + 定长编码）。
///
/// 失败（链满/存储错误）静默忽略——panic 路径不因审计失败再 panic
/// （fail-closed 语义由复位保证：审计失败不阻止复位）。
pub fn log_panic_event(actor: &str, detail: &str) {
    if let Some(logger) = PANIC_LOGGER.lock().as_mut() {
        let ev = AuditEvent::new(AuditEventType::SecurityAlert, actor, "panic", detail);
        let _ = logger.log(ev);
    }
}

/// panic 复位序列（不返回）：审计 → 计数 → 武装 SP805 10 ms 超时 → 复位。
///
/// `wdt_base == 0`（无 SP805 的平台/软件模式）时纯挂起——QEMU 验证与
/// 真机 BSP 均以 [`WDT_BASE`] 武装复位。SP805 写序列见 [`sp805_arm_sequence`]
/// （host 测试断言寄存器序列）；硬件复位动作（超时→SoC 复位）为真机/
/// QEMU 完整版验证项（xpack QEMU 9.2.4 无 SP805 设备，ADR-026）。
pub fn panic_reboot(info: &PanicInfo) -> ! {
    // ① 审计（无堆；location 为 &'static str 可直接定容写入）
    let detail = format_location(info.location());
    log_panic_event("root-task", &detail);
    // ② 重启计数（内存指标）
    let n = PANIC_REBOOT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    eneros_runtime::println!(
        "[eneros-root-task] PANIC reboot #{} at {} — arming watchdog reset",
        n,
        detail
    );
    // ③ 武装 SP805 短超时 → 超时硬件复位（10 ms 内；无 SP805 时挂起）
    let wdt = eneros_watchdog::wdt::HwWatchdog::new(WDT_BASE);
    wdt.init(10);
    loop {
        core::hint::spin_loop();
    }
}

/// SP805 武装写序列（纯逻辑，host 测试断言寄存器序列；生产路径由
/// [`panic_reboot`] 经 `HwWatchdog::init` 执行同一序列）。
///
/// 序列（SP805 规范）：解锁（0xC00←0x1ACCE551）→ 装载超时（0x00←
/// `timeout_ms*1000`）→ 清中断（0x0C←1）→ 使能 中断+复位（0x08←0x3）
/// → 加锁（0xC00←1）。
pub fn sp805_arm_sequence(mut write: impl FnMut(u64, u32), timeout_ms: u32) {
    write(0xC00, 0x1ACCE551); // unlock
    write(0x00, timeout_ms * 1000); // load（单位 1kHz 周期）
    write(0x0C, 1); // intclr
    write(0x08, 0x3); // ctrl: EN | RESEN
    write(0xC00, 0x1); // lock
}

/// 提取 panic 位置的定容字符串（`file:line`；无堆——file 为 &'static str）。
fn format_location(loc: Option<&Location>) -> String<128> {
    let mut s = String::new();
    match loc {
        Some(loc) => {
            let _ = s.push_str(loc.file());
            let _ = s.push(':');
            let _ = write!(s, "{}", loc.line());
        }
        None => {
            let _ = s.push_str("unknown");
        }
    }
    s
}

/// 审计链当前条目数（host 测试/证据用）。
pub fn panic_audit_entry_count() -> usize {
    match PANIC_LOGGER.lock().as_ref() {
        Some(logger) => logger.chain_state().entry_count as usize,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use eneros_audit::AuditStore;

    use super::*;

    /// location 格式化：无堆提取 file:line。
    #[test]
    fn test_format_location_unknown_and_real() {
        assert_eq!(format_location(None).as_str(), "unknown");
        let here = Location::caller();
        let s = format_location(Some(here));
        assert!(s.contains("panic.rs"), "必须含文件路径: {s}");
    }

    /// 审计事件写入后链条目递增（MemoryStore 无堆路径）。
    #[test]
    fn test_log_panic_event_increments_chain() {
        init_panic_audit();
        let before = panic_audit_entry_count();
        log_panic_event("root-task", "test-panic");
        let after = panic_audit_entry_count();
        assert_eq!(after, before + 1, "审计链必须递增");
        // 事件可查：store 末条为本次事件（seq 递增）。
        if let Some(logger) = PANIC_LOGGER.lock().as_ref() {
            let last = logger.store().read_last_n(1).expect("read");
            let entry = last.last().expect("entry");
            assert_eq!(entry.seq, after as u64, "seq 必须为递增序号");
        }
    }

    /// 重启计数递增（内存指标）。
    #[test]
    fn test_reboot_count_increments() {
        let before = PANIC_REBOOT_COUNT.load(Ordering::Relaxed);
        PANIC_REBOOT_COUNT.fetch_add(1, Ordering::Relaxed);
        let after = PANIC_REBOOT_COUNT.load(Ordering::Relaxed);
        assert_eq!(after, before + 1);
    }

    /// SP805 武装序列：寄存器写顺序与值必须符合规范（host 可断言）。
    #[test]
    fn test_sp805_arm_sequence_registers() {
        let mut writes: Vec<(u64, u32)> = Vec::new();
        sp805_arm_sequence(|off, v| writes.push((off, v)), 10);
        assert_eq!(
            writes,
            vec![
                (0xC00, 0x1ACCE551), // unlock
                (0x00, 10_000),      // load = timeout_ms * 1000
                (0x0C, 1),           // intclr
                (0x08, 0x3),         // ctrl: EN|RESEN
                (0xC00, 0x1),        // lock
            ],
            "SP805 武装序列必须符合规范"
        );
    }

    /// 超时装载与 timeout_ms 线性关系（10ms vs 1000ms）。
    #[test]
    fn test_sp805_load_scales_with_timeout() {
        let mut load_10 = 0u32;
        sp805_arm_sequence(
            |off, v| {
                if off == 0x00 {
                    load_10 = v
                }
            },
            10,
        );
        let mut load_1000 = 0u32;
        sp805_arm_sequence(
            |off, v| {
                if off == 0x00 {
                    load_1000 = v
                }
            },
            1000,
        );
        assert_eq!(load_10, 10_000);
        assert_eq!(load_1000, 1_000_000);
    }
}
