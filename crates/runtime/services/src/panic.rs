//! 22 §2.W1.2：EL0 服务 panic 行为——审计 + 挂起（supervisor 兜底）。
//!
//! 分层语义（锁定 6 分级重启）：服务 panic **不**复位整机（隔离语义），
//! 而是：①写审计日志（SM3 哈希链，无堆）→ ②打印 → ③挂起。重启由
//! supervisor（W1.3）执行；整机看门狗（W1.6）是服务挂起且 supervisor
//! 失效时的最后防线。
//!
//! 审计链静态预分配（MemStore + 链头），panic 路径零堆分配。

use core::fmt::Write as _;
use core::panic::{Location, PanicInfo};
use core::sync::atomic::{AtomicUsize, Ordering};

use eneros_audit::{AuditEvent, AuditEventType, AuditLogger, AuditLoggerImpl, MemStore};
use heapless::String;
use spin::Mutex;

/// 服务 panic 审计链（预分配）。
static PANIC_LOGGER: Mutex<Option<AuditLoggerImpl<MemStore>>> = Mutex::new(None);

/// 服务 panic 总次数（指标）。
pub static SVC_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 30 RT-028①：创世根 = 真实摘要（运行时计算）。
/// 旧实现为 ASCII "svc-panic-genesis" 补零占位（可被离线伪造）。
/// 36 WP-36.2/SC-03：SM3 → SHA-256（eneros-crypto 无商业批准，不入产品闭包）。
fn genesis() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b"svc-panic-genesis").into()
}

/// 初始化服务 panic 审计链（svc_entry 启动时调用一次）。
pub fn init_svc_panic_audit() {
    *PANIC_LOGGER.lock() = Some(AuditLoggerImpl::new(MemStore::new(), genesis()));
}

/// 服务 panic 钩子：审计 → 打印 → 挂起（不返回）。
///
/// 由各 bin 的 `#[panic_handler]` 调用。禁止在此分配或做复杂逻辑
/// （堆可能已耗尽；heapless + println 无堆安全）。
pub fn svc_panic_hook(svc_name: &'static str, info: &PanicInfo) -> ! {
    let n = SVC_PANIC_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let detail = format_location(info.location());
    // 30 RT-028②：try_lock——panic 由持锁代码（logger 内部/重入）触发时
    // lock() 死锁挂起连打印都没有；拿不到锁跳过审计，保证打印与挂起可达。
    if let Some(mut guard) = PANIC_LOGGER.try_lock() {
        if let Some(logger) = guard.as_mut() {
            let ev = AuditEvent::new(AuditEventType::SecurityAlert, svc_name, "panic", &detail);
            let _ = logger.log(ev);
        }
    }
    eneros_runtime::println!(
        "[svc-{}] PANIC #{} at {} — suspending (supervisor restart, W1.3)",
        svc_name,
        n,
        detail
    );
    crate::svc_loop()
}

/// 提取 panic 位置的定容字符串（`file:line`；无堆）。
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

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use eneros_audit::AuditStore;

    use super::*;

    #[test]
    fn test_format_location_unknown_and_real() {
        assert_eq!(format_location(None).as_str(), "unknown");
        let here = Location::caller();
        let s = format_location(Some(here));
        assert!(s.contains("panic.rs"), "必须含文件路径: {s}");
    }

    #[test]
    fn test_init_and_count_chain() {
        init_svc_panic_audit();
        let before = PANIC_LOGGER
            .lock()
            .as_ref()
            .map(|l| l.chain_state().entry_count)
            .unwrap_or(0);
        if let Some(logger) = PANIC_LOGGER.lock().as_mut() {
            let ev = AuditEvent::new(AuditEventType::SecurityAlert, "svc-hello", "panic", "test");
            let _ = logger.log(ev);
        }
        let after = PANIC_LOGGER
            .lock()
            .as_ref()
            .map(|l| l.chain_state().entry_count)
            .unwrap_or(0);
        assert_eq!(after, before + 1, "审计链必须递增");
    }
}
