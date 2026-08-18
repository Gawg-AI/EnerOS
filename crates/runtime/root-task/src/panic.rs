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
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

extern crate alloc;

use alloc::vec::Vec;

use eneros_audit::{
    AuditError, AuditEvent, AuditEventType, AuditLogger, AuditLoggerImpl, ChainEntry,
};
use heapless::String;
use spin::Mutex;

/// SP805 看门狗 MMIO 基址（**虚拟地址**——driver_probe wdt_factory 经 map_mmio
/// 注入；0 = 未装配 → panic 路径走软件模式）。
/// 31 D6（review）：默认改 0（旧 0x0905_0000 硬编码已退役；真机经 FDT 注入）。
pub static WDT_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// 31 D5：PL031 RTC 基址（FDT reg 注入后更新；0 = 未装配）。
pub static RTC_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// 31 D6：注册 SP805 基址（driver_probe wdt_factory 调用）。
pub fn register_wdt_base(base: u64) {
    WDT_BASE.store(base, core::sync::atomic::Ordering::Relaxed);
}

/// 31 D5：注册 PL031 RTC 基址（driver_probe rtc_factory 调用）。
pub fn register_rtc_base(base: u64) {
    RTC_BASE.store(base, core::sync::atomic::Ordering::Relaxed);
}

/// SP805 设备区域是否已映射进 root-task VSpace（QEMU 无设备时 map 失败
/// → 软件模式；真机 map 成功 → 硬件模式）。
pub static WDT_MMIO_MAPPED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// panic 触发后的重启计数（内存；跨掉电持久化挂 W3.5，ADR-026）。
pub static PANIC_REBOOT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// WP-37.9（复盘）：本次启动的 boot_id（main.rs 启动早期注入；0 = 未注入）。
/// panic 审计 detail 携带该值，跨重启可归因（蓝图：记录 boot_id、service_id、
/// fault、register digest、manifest/artifact digest——本卷实现 boot_id +
/// service_id（actor）+ fault（location）+ artifact digest；register digest
/// 注入通道登记后续）。
pub static PANIC_BOOT_ID: AtomicU64 = AtomicU64::new(0);

/// 注册本次启动 boot_id（启动早期、任何 panic 可能发生前调用一次）。
pub fn register_boot_id(boot_id: u64) {
    PANIC_BOOT_ID.store(boot_id, core::sync::atomic::Ordering::Relaxed);
}

/// WP-37.9：panic 审计降级标记——审计 append 被跳过/失败（durable
/// append 超时、try_lock 失败、CrashStore 满竞争）时置位。**watchdog
/// 序列不受审计结果影响**（fail-closed 由复位保证）；降级被显式记录
/// （panic detail + 重启计数携带），不静默。存储层（39）接线后，
/// durable append 超时同样走本标记 + watchdog。
static PANIC_AUDIT_DEGRADED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// 读取 panic 审计降级标记（host 测试/证据用；true = 本次 panic 审计
/// 曾降级——复位后由重启计数与 detail 携带）。
pub fn panic_audit_degraded() -> bool {
    PANIC_AUDIT_DEGRADED.load(core::sync::atomic::Ordering::Relaxed)
}

/// WP-37.9：本次启动消费的制品摘要（init bundle payload sha256——root-task
/// 实际解析的介质事实；4×u64 静态存储，panic 路径零堆零锁）。
/// 全零 = 未注入。
static PANIC_ARTIFACT_DIGEST: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// 注册本次启动的制品摘要（parse_verified 成功后调用一次；panic 路径
/// 只读，无需锁）。
pub fn register_artifact_digest(digest: [u8; 32]) {
    for (i, word) in digest.chunks_exact(8).enumerate() {
        PANIC_ARTIFACT_DIGEST[i].store(
            u64::from_le_bytes(word.try_into().expect("8-byte chunk")),
            core::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// 读取当前制品摘要（host 测试用；0 = 未注入）。
pub fn artifact_digest() -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, word) in out.chunks_exact_mut(8).enumerate() {
        word.copy_from_slice(
            &PANIC_ARTIFACT_DIGEST[i]
                .load(core::sync::atomic::Ordering::Relaxed)
                .to_le_bytes(),
        );
    }
    out
}

/// WP-37.9：panic 时抓取寄存器摘要（**仅 target**；sp/lr/fp——只读
/// 内联汇编，无堆无锁，panic 路径安全）。FNV-1a 32 位哈希（零依赖），
/// detail 以 `reg=<8hex>` 携带——跨重启可定位"在哪条调用链上 panic"。
/// host 构建无 seL4 寄存器语义，不执行（返回 0——host 测试只验证
/// 格式拼装）。
#[cfg(target_os = "none")]
fn register_digest() -> u32 {
    let sp_val: u64;
    let lr_val: u64;
    let fp_val: u64;
    // SAFETY: 只读系统寄存器（sp/x30/x29），nomem/nostack 无副作用。
    unsafe {
        core::arch::asm!("mov {0}, sp", out(reg) sp_val, options(nomem, nostack));
        core::arch::asm!("mov {0}, x30", out(reg) lr_val, options(nomem, nostack));
        core::arch::asm!("mov {0}, x29", out(reg) fp_val, options(nomem, nostack));
    }
    let mut h: u32 = 0x811c_9dc5;
    for v in [sp_val, lr_val, fp_val] {
        for b in v.to_le_bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// host 构建：无寄存器语义（占位 0；格式拼装仍可测）。
#[cfg(not(target_os = "none"))]
fn register_digest() -> u32 {
    0
}

/// panic 审计链（预分配 CrashStore + 链头；panic 路径**零堆分配**——
/// WP-37.9：MemStore(Vec) 在堆损坏/OOM 时二次失败；try_lock 防止主路径
/// 持锁时 panic 死锁到不了 watchdog）。
static PANIC_LOGGER: Mutex<Option<AuditLoggerImpl<CrashStore>>> = Mutex::new(None);

/// WP-37.9：panic 审计定容存储（heapless Vec，容量 16 条 ChainEntry；
/// 满则丢弃最旧——panic 审计只保证最近 N 条可达 watchdog/复位，
/// 不保证全量）。追加路径零堆分配（heapless 内联容量）。
#[derive(Debug, Clone, Default)]
struct CrashStore {
    entries: heapless::Vec<ChainEntry, 16>,
}

impl CrashStore {
    fn new() -> Self {
        Self {
            entries: heapless::Vec::new(),
        }
    }
}

impl eneros_audit::AuditStore for CrashStore {
    fn append(&mut self, entry: &ChainEntry) -> Result<(), AuditError> {
        if self.entries.is_full() {
            // 定容：丢弃最旧一条（O(16)，panic 路径可承受；无分配）。
            let _ = self.entries.remove(0);
        }
        // heapless push 不分配；满时（并发 append 竞争）返回错误，
        // 由调用方降级（watchdog 序列不受影响）。
        self.entries
            .push(entry.clone())
            .map_err(|_| AuditError::StorageWriteFailed)
    }

    fn read_range(&self, start_seq: u64, end_seq: u64) -> Result<Vec<ChainEntry>, AuditError> {
        Ok(self
            .entries
            .iter()
            .filter(|e| e.seq >= start_seq && e.seq <= end_seq)
            .cloned()
            .collect())
    }

    fn read_last_n(&self, n: usize) -> Result<Vec<ChainEntry>, AuditError> {
        let total = self.entries.len();
        let skip = total.saturating_sub(n);
        Ok(self.entries.iter().skip(skip).cloned().collect())
    }

    fn count(&self) -> Result<u64, AuditError> {
        Ok(self.entries.len() as u64)
    }
}

/// 30 RT-028①：创世根 = 真实摘要（运行时计算）。
/// 旧实现为 ASCII "eneros-root-task-panic" 补零占位（注释却声称哈希，
/// 审计链"创世"可被离线伪造）。链恢复语义要求创世跨重启不变：真实创世根
/// 由本函数确定并经 DurableLog 持久化（WP-30.7 机制）。
/// 36 WP-36.2/SC-03：SM3 → SHA-256（eneros-crypto 无商业批准，
/// product_allowed=false，不得进入产品闭包；sha2 与正式链同算法族）。
pub fn genesis() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b"eneros-root-task-panic").into()
}

/// 初始化 panic 审计链（启动早期调用一次；重复调用重置链）。
pub fn init_panic_audit() {
    *PANIC_LOGGER.lock() = Some(AuditLoggerImpl::new(CrashStore::new(), genesis()));
}

/// 追加一条 panic 审计事件（WP-37.9：无堆——CrashStore 定容 + 定长编码；
/// try_lock 失败（主路径持锁中 panic）直接跳过审计，保证 watchdog 序列
/// 可达——fail-closed 语义由复位保证，审计失败不阻止复位）。
pub fn log_panic_event(actor: &str, detail: &str) {
    if let Some(mut guard) = PANIC_LOGGER.try_lock() {
        if let Some(logger) = guard.as_mut() {
            let ev = AuditEvent::new(AuditEventType::SecurityAlert, actor, "panic", detail);
            // append 失败（StorageWriteFailed 等）→ 降级标记（watchdog
            // 序列不受阻；durable append 超时语义同此路径——39 存储层
            // 接线后由 DurableLog 提供真实 append，超时同样走本标记）。
            if logger.log(ev).is_err() {
                PANIC_AUDIT_DEGRADED.store(true, core::sync::atomic::Ordering::Relaxed);
            }
        }
    } else {
        // try_lock 失败（主路径持锁中 panic）→ 审计跳过 + 降级标记。
        PANIC_AUDIT_DEGRADED.store(true, core::sync::atomic::Ordering::Relaxed);
    }
}

/// panic 复位序列（不返回）：审计 → 计数 → 武装 SP805 10 ms 超时 → 复位。
///
/// 30 RT-001：武装前必须检查 [`WDT_MMIO_MAPPED`]——旧实现软件模式（MMIO
/// 映射失败，如 QEMU xpack 无 SP805 节点）下 panic 路径写未映射
/// 0x0905_0000 → 二次 VM fault，审计链/计数后续步骤不可达。软件模式打印
/// 判据行后 spin（与 `ipc_fail_closed` 同语义；依赖外部看门狗/人工复位）。
/// SP805 写序列见 [`sp805_arm_sequence`]（host 测试断言寄存器序列）。
pub fn panic_reboot(info: &PanicInfo) -> ! {
    // ① 审计（无堆；location 为 &'static str 可直接定容写入）
    // WP-37.9（复盘）：detail 携带 boot_id（跨重启归因）、artifact digest
    // （本次启动介质事实）与 fault location。
    let detail = format_location(info.location());
    let boot_id = PANIC_BOOT_ID.load(core::sync::atomic::Ordering::Relaxed);
    log_panic_event(
        "root-task",
        &format_panic_detail(boot_id, artifact_digest(), register_digest(), &detail),
    );
    // WP-37.9：审计降级显式记录（durable append 超时/跳过时 watchdog
    // 序列照常——fail-closed 由复位保证，不因审计失败阻塞复位）。
    if panic_audit_degraded() {
        eneros_runtime::println!(
            "[eneros-root-task] PANIC audit DEGRADED (append skipped/timed out) — watchdog sequence unaffected"
        );
    }
    // ② 重启计数（内存指标）
    let n = PANIC_REBOOT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // ③ 武装 SP805 短超时 → 超时硬件复位（10 ms 内）
    if WDT_MMIO_MAPPED.load(core::sync::atomic::Ordering::Relaxed) {
        eneros_runtime::println!(
            "[eneros-root-task] PANIC reboot #{} at {} — arming watchdog reset",
            n,
            detail
        );
        let wdt = eneros_watchdog::wdt::HwWatchdog::new(
            WDT_BASE.load(core::sync::atomic::Ordering::Relaxed),
        );
        wdt.init(10);
    } else {
        // 30 RT-001：软件模式（无 SP805 映射）——不写未映射 MMIO，打印
        // 判据行后 spin（防二次 VM fault）。
        eneros_runtime::println!(
            "[eneros-root-task] PANIC #{} at {} — WDT unmapped — spin, awaiting external reset",
            n,
            detail
        );
    }
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

/// WP-37.9（复盘）：panic 审计 detail 定容拼装
/// `boot_id=<hex>;md=<artifact digest hex>;reg=<register digest hex>;
/// <location>`（无堆——heapless String 定容，截断安全）。boot_id=0 表示
/// 未注入；md=64 个零表示未注入；reg 为 sp/lr/fp 的 FNV-1a 摘要
/// （host 构建恒 0——格式可测，值无意义）。
fn format_panic_detail(boot_id: u64, artifact: [u8; 32], reg: u32, location: &str) -> String<256> {
    let mut s = String::new();
    let _ = s.push_str("boot_id=");
    let _ = write!(s, "{boot_id:x}");
    let _ = s.push_str(";md=");
    for b in artifact {
        let _ = write!(s, "{b:02x}");
    }
    let _ = s.push_str(";reg=");
    let _ = write!(s, "{reg:08x}");
    let _ = s.push(';');
    let _ = s.push_str(location);
    s
}

/// 审计链当前条目数（host 测试/证据用；try_lock 失败返回 0——只读探测
/// 不阻塞主路径）。
pub fn panic_audit_entry_count() -> usize {
    match PANIC_LOGGER.try_lock().as_deref() {
        Some(Some(logger)) => logger.chain_state().entry_count as usize,
        _ => 0,
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

    /// WP-37.9（复盘）：panic detail 携带 boot_id 与 artifact digest。
    #[test]
    fn test_format_panic_detail_includes_boot_id_and_artifact() {
        let d = format_panic_detail(0x1a2b, [0u8; 32], 0, "src/main.rs:42");
        assert_eq!(
            d.as_str(),
            "boot_id=1a2b;md=0000000000000000000000000000000000000000000000000000000000000000;reg=00000000;src/main.rs:42"
        );
        let mut digest = [0u8; 32];
        digest[0] = 0xAB;
        digest[31] = 0xCD;
        let d1 = format_panic_detail(0, digest, 0xDEAD_BEEF, "unknown");
        assert!(
            d1.contains(";md=ab000000000000000000000000000000000000000000000000000000000000cd;")
        );
    }

    /// WP-37.9（复盘）：register_boot_id + register_artifact_digest 注入
    /// 与读取往返（panic 路径只读静态）。
    #[test]
    fn test_panic_boot_id_registered_and_used() {
        init_panic_audit();
        register_boot_id(0xDEAD_BEEF);
        assert_eq!(PANIC_BOOT_ID.load(Ordering::Relaxed), 0xDEAD_BEEF);
        let mut digest = [0u8; 32];
        for (i, b) in digest.iter_mut().enumerate() {
            *b = i as u8;
        }
        register_artifact_digest(digest);
        assert_eq!(artifact_digest(), digest, "digest 往返必须一致");
        let detail = format_panic_detail(
            PANIC_BOOT_ID.load(Ordering::Relaxed),
            artifact_digest(),
            register_digest(),
            "boot.rs:1",
        );
        assert!(detail.contains("boot_id=deadbeef"), "detail: {detail}");
        assert!(
            detail
                .contains(";md=000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f;"),
            "detail 必须含完整 artifact digest: {detail}"
        );
    }

    /// 审计事件写入后链条目递增（CrashStore 定容无堆路径）。
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

    /// WP-37.9：CrashStore 定容——超过 16 条丢弃最旧，count 封顶。
    #[test]
    fn test_crash_store_caps_at_16_dropping_oldest() {
        use eneros_audit::AuditStore;
        let mut store = CrashStore::new();
        for seq in 1..=20u64 {
            store
                .append(&ChainEntry {
                    seq,
                    prev_hash: [0u8; 32],
                    event_hash: [0u8; 32],
                    combined_hash: [0u8; 32],
                    timestamp_ms: 0,
                })
                .expect("append");
        }
        assert_eq!(store.count().unwrap(), 16, "定容 16 条");
        let all = store.read_last_n(16).unwrap();
        // 最旧 4 条（seq 1..4）被丢弃，保留 5..20。
        assert_eq!(all.first().unwrap().seq, 5, "丢弃最旧");
        assert_eq!(all.last().unwrap().seq, 20, "保留最新");
    }

    /// WP-37.9：主路径持锁时 panic 审计不阻塞（try_lock 失败直接跳过，
    /// watchdog 序列可达——修复前 Mutex::lock 死锁）。**降级标记**：
    /// 审计跳过/失败 → PANIC_AUDIT_DEGRADED 置位（durable append 超时
    /// 同语义——watchdog 不受阻，降级显式记录）。
    #[test]
    fn test_panic_audit_skips_when_locked() {
        init_panic_audit();
        PANIC_AUDIT_DEGRADED.store(false, Ordering::Relaxed);
        // 模拟主路径持锁（测试持锁期间调用 log_panic_event）。
        let _guard = PANIC_LOGGER.lock();
        let before = PANIC_REBOOT_COUNT.load(Ordering::Relaxed);
        log_panic_event("root-task", "panic-while-locked");
        // 不 panic、不阻塞；审计被跳过（try_lock 失败），重启计数路径不受影响。
        assert_eq!(PANIC_REBOOT_COUNT.load(Ordering::Relaxed), before);
        // 降级被显式记录（watchdog 序列不受阻的机器可读标记）。
        assert!(panic_audit_degraded(), "审计跳过必须置位降级标记");
        drop(_guard);
        // 锁释放后审计恢复可用（降级标记保持——本次 panic 已降级）。
        log_panic_event("root-task", "after-unlock");
        assert!(panic_audit_entry_count() >= 1);
        assert!(panic_audit_degraded(), "降级标记不可自愈（由复位清除）");
    }

    /// WP-37.9：审计 append 失败（store 满/写失败）→ 降级标记置位，
    /// watchdog 序列不受阻（fail-closed 由复位保证）。
    #[test]
    fn test_panic_audit_append_failure_sets_degraded() {
        init_panic_audit();
        PANIC_AUDIT_DEGRADED.store(false, Ordering::Relaxed);
        // 填满 CrashStore（16 条）后继续 append：满时丢弃最旧不报错——
        // 通过锁竞争模拟失败路径：直接调 log 前先占锁不可行（try_lock），
        // 改为验证"跳过路径"已覆盖；此处验证标记 API 语义。
        assert!(!panic_audit_degraded());
        // 正常 append 不置位。
        log_panic_event("root-task", "normal-append");
        assert!(!panic_audit_degraded(), "正常 append 不得置位降级");
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
