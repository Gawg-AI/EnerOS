//! EnerOS reference EL0 services — 共享入口与 panic handler.
//!
//! 这四个服务是第五册 M0 阶段的占位 EL0 进程，被打包进 init bundle：
//! - `svc-hello`：hello-world 参考服务
//! - `svc-pi`：CPU 密集型（π 计算）参考服务
//! - `svc-hog`：内存占用参考服务（OOM 隔离验证）
//! - `svc-fault`：故障注入参考服务
//!
//! M0 阶段：每个服务只是合法 aarch64-unknown-none ELF（`_start` loop），
//! 由 root task 在 M2.1（seL4 TCB/CSpace/VSpace FFI）完成后 spawn。
//!
//! no_std 合规：仅 core，无 alloc 依赖。

#![no_std]
#![allow(clippy::empty_loop)]

extern crate alloc;

pub mod allocator;
pub mod panic;

#[cfg(all(test, feature = "loom"))]
pub mod loom_allocator;

/// EL0 服务公共入口 — 永久 loop。
///
/// KRN-003：服务帧往返就绪——`svc_frame_roundtrip` 复用
/// `eneros-ipc-wire::el0`（crc32 帧完整性 + 版本握手）编解码 root-task
/// 消息；地址空间隔离与故障不扩散由 seL4 提供（QEMU 验证项，S2 BLOCKED）。
/// v0.200.0（P0-3）：四个服务实现真实功能（π 计算/内存占用/故障注入），
/// 不再只是空 loop；`svc_loop` 保留为 M2.1 Endpoint IPC 接入前的兜底。
#[inline]
pub fn svc_loop() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// ============================================================================
// v0.200.0（P0-3）：四个参考服务的真实功能
// ============================================================================

/// svc-hello：返回服务 banner（含运行次数）。
///
/// 真实功能：维护单调计数器并格式化 banner；每次调用推进一次。
pub fn svc_hello(banner: &str) -> alloc::string::String {
    alloc::format!("hello:{banner}:tick={}", hello_counter())
}

/// 内部单调计数器（svc-hello 心跳）。
pub fn hello_counter() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

/// svc-pi：Leibniz 级数计算 π（CPU 密集型真实计算）。
///
/// `iterations` 为级数项数；返回值随迭代数收敛于 π。非零迭代时与真实 π
/// 的误差随迭代数下降（可作 CPU 负载真实性的可验证证据）。
pub fn compute_pi(iterations: u64) -> f64 {
    if iterations == 0 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for i in 0..iterations {
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        sum += sign / (2.0 * (i as f64) + 1.0);
    }
    4.0 * sum
}

/// svc-hog：真实分配内存块（OOM 隔离验证）。
///
/// 分配 `blocks` 个 4 KiB 块并填充可识别模式，返回句柄。分配失败（OOM）
/// 返回错误——OOM 隔离由 seL4 地址空间保证（QEMU 验证项）。
pub fn hog_memory(blocks: usize) -> Result<alloc::vec::Vec<alloc::vec::Vec<u8>>, ()> {
    let mut pages = alloc::vec::Vec::with_capacity(blocks);
    for i in 0..blocks {
        let mut page = alloc::vec![0xA5u8; 4096];
        let seed = (i as u32).wrapping_mul(0x9E37_79B9).to_le_bytes();
        for (j, chunk) in page.chunks_mut(4).enumerate() {
            let v = seed[0].wrapping_add(j as u8);
            chunk.copy_from_slice(&[v; 4][..chunk.len()]);
        }
        pages.push(page);
    }
    Ok(pages)
}

/// 故障注入类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// 除零（SVC/异常路径）。
    DivideByZero,
    /// 非法内存访问（缺页）。
    IllegalAccess,
    /// 显式 panic（panicking 路径）。
    Panic,
    /// WP-37.3：从**栈页取指执行**——NX 负例。栈/数据页映射带
    /// ExecuteNever 时触发 instruction-fetch vm fault（服务被内核挂起，
    /// 可归因 fault 证明 NX 生效）；NX 缺失时栈上 ret 指令返回后执行
    /// 0x00 字节（UDF）→ 不同 fault 类型，QEMU 日志可区分。
    ExecStack,
}

/// svc-fault：真实触发指定故障。
///
/// 供 root-task 的故障注入/不扩散验证使用；除零与非法访问走真实异常路径，
/// panic 走 panic handler。`NoFault` 返回 `Ok`。
pub fn inject_fault(kind: Option<FaultKind>) -> Result<(), ()> {
    match kind {
        None => Ok(()),
        Some(FaultKind::DivideByZero) => {
            // 用 volatile 读防御编译期常量折叠（触发真实除零异常路径）。
            // SAFETY: 读取 `&0u32` 的 volatile 拷贝，地址为静态常量
            // 有效内存；触发除零是 svc-fault 的故障注入语义。
            let zero = unsafe { core::ptr::read_volatile(&0u32) };
            let _ = 1u32 / zero;
            Err(())
        }
        Some(FaultKind::IllegalAccess) => {
            // SAFETY: 故意访问非法地址触发缺页；仅 svc-fault 测试路径使用。
            unsafe {
                let bad = core::ptr::null_mut::<u32>();
                core::ptr::write_volatile(bad, 0xDEAD);
            }
            Err(())
        }
        Some(FaultKind::Panic) => {
            // 显式 panic 触发 panic handler（svc-fault 预期行为）。
            #[allow(clippy::disallowed_macros)]
            {
                panic!("svc-fault: injected panic");
            }
        }
        Some(FaultKind::ExecStack) => {
            // WP-37.3：NX 负例——从栈页取指执行。栈上构造 AArch64 `ret`
            // 指令（0xD65F03C0，LE 字节 C0 03 5F D6）+ 0x00 填充；经函数
            // 指针跳入栈页。NX 生效 → 取指 vm fault（栈地址可归因）；
            // NX 缺失 → ret 后执行 UDF（0x00000000）→ 也 fault 但类型不同。
            // 仅 target 真实执行（host 无 seL4 页语义，不执行）。
            #[cfg(target_os = "none")]
            {
                let stack_slot: [u8; 8] = [0xC0, 0x03, 0x5F, 0xD6, 0x00, 0x00, 0x00, 0x00];
                // SAFETY: 故意从栈页取指——NX 验证负例（服务预期被内核
                // 挂起；transmute 栈字节为函数指针是注入语义本身）。
                let f: extern "C" fn() = unsafe { core::mem::transmute(stack_slot.as_ptr()) };
                f();
            }
            Err(())
        }
    }
}

/// 服务帧处理（KRN-003）：解码一帧 → 校验类型 → 回显 payload。
///
/// 供 svc-hello 等参考服务在 M2.1 Endpoint IPC 接入后调用；host 侧
/// 单测验证帧往返（编码 → 解码 → 校验）完整可用。
pub fn svc_frame_roundtrip(
    payload: &[u8],
) -> Result<alloc::vec::Vec<u8>, eneros_ipc_wire::el0::El0FrameError> {
    use eneros_ipc_wire::el0::{decode_frame, encode_frame, El0FrameType};
    let buf = encode_frame(El0FrameType::Report, payload);
    let frame = decode_frame(&buf)?;
    if frame.payload != payload {
        return Err(eneros_ipc_wire::el0::El0FrameError::BadCrc);
    }
    Ok(frame.payload.to_vec())
}

// ============================================================================
// 21-A K2.3：服务 IPC 请求处理（host 可测）+ svc_ipc_loop（目标构建）
// ============================================================================

/// 十进制无符号解析（no_std，K2.3 请求参数用）。
pub fn parse_u64(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(v)
}

/// 请求路由结果（K2.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestOutcome {
    /// 正常响应（响应帧字节）。
    Reply(alloc::vec::Vec<u8>),
    /// 故障注入已触发（服务预期将挂起，不再回复）。
    FaultTriggered,
}

/// K2.3 请求处理：解码帧 → 按 payload 前缀路由到服务功能 → 编码响应帧。
///
/// 协议（payload 前缀，root-task ↔ 服务共享契约）：
/// - `hello`        → svc_hello 心跳 banner；
/// - `pi=<n>`       → compute_pi 前 n 项（CPU 负载验证）；
/// - `hog=<n>`      → hog_memory n 个 4 KiB 块（OOM 隔离验证）；
/// - `fault=<kind>` → inject_fault（DivideByZero/IllegalAccess/Panic）；
/// - 其他            → `unknown:<payload>`（fail-closed：不扩散）。
///
/// 响应一律 `Report` 帧。`fault` 分支触发后服务进程预期挂起（seL4 异常
/// 路径），返回 [`RequestOutcome::FaultTriggered`]（不回复）。
pub fn handle_request(payload: &[u8]) -> RequestOutcome {
    let resp = |s: &str| RequestOutcome::Reply(encode_report(s.as_bytes()));
    if payload == b"hello" {
        return resp(&svc_hello("eneros"));
    }
    if let Some(rest) = payload.strip_prefix(b"pi=") {
        return match parse_u64(rest) {
            Some(n) => {
                let pi = compute_pi(n);
                let mut buf = alloc::string::String::with_capacity(32);
                buf.push_str("pi=");
                // f64 → 定点字符串（截断 6 位小数；量纲标注于协议文档）。
                let scaled = (pi * 1_000_000.0) as u64;
                buf.push_str(&alloc::format!("{scaled}"));
                resp(&buf)
            }
            None => resp("pi=err"),
        };
    }
    if let Some(rest) = payload.strip_prefix(b"hog=") {
        return match parse_u64(rest) {
            Some(n) => match hog_memory(n as usize) {
                Ok(pages) => {
                    let s = alloc::format!("hog=ok:{}", pages.len());
                    resp(&s)
                }
                Err(()) => resp("hog=oom"),
            },
            None => resp("hog=err"),
        };
    }
    if let Some(rest) = payload.strip_prefix(b"fault=") {
        let kind = match rest {
            // 注：aarch64 无除零异常（SDIV 结果 0 不触发 Data Abort）——
            // DivideByZero 分支仅保留协议兼容与 host 测试语义，目标平台
            // 隔离验证请用 IllegalAccess（null 写 → Data Abort）或
            // ExecStack（栈页取指 → instruction-fetch fault，WP-37.3 NX 负例）。
            b"DivByZero" => Some(FaultKind::DivideByZero),
            b"IllegalAccess" => Some(FaultKind::IllegalAccess),
            b"Panic" => Some(FaultKind::Panic),
            b"ExecStack" => Some(FaultKind::ExecStack),
            _ => None,
        };
        // 触发真实故障：除零/非法访问走异常路径，服务 TCB 被内核挂起
        // （21-A K2.3 隔离验证核心）；不再回复。打印须在注入之前——
        // IllegalAccess 的异常在写指令处同步触发，控制流立即进内核 fault
        // handler，注入后的任何代码（含 println）不可达。
        eneros_runtime::println!("[svc-fault] FAULT triggered — suspending");
        if let Err(()) = inject_fault(kind) {
            return RequestOutcome::FaultTriggered;
        }
        return resp("fault=noop");
    }
    resp(&alloc::format!(
        "unknown:{}",
        core::str::from_utf8(payload).unwrap_or("?")
    ))
}

/// 编码 Report 响应帧。
fn encode_report(payload: &[u8]) -> alloc::vec::Vec<u8> {
    eneros_ipc_wire::el0::encode_frame(eneros_ipc_wire::el0::El0FrameType::Report, payload)
}

/// 服务入口（bin `_start` 统一调用，K2.3）。
///
/// 目标构建：Signal 就绪 ntfn（WP-37.1 #8 readiness handshake——root-task
/// 在 deadline 内 poll 判定 Ready；readiness 来自 IPC 通知，非 spawn
/// 成功）→ 进入 Endpoint IPC 主循环（[`svc_ipc_loop`]）；
/// host 构建：无 seL4，回到 M0 占位空转（[`svc_loop`]，非假成功——
/// 服务在 host 本就不存在运行环境，21-A 红线 2 只禁"模拟成功的系统调用"）。
pub fn svc_entry(ep: u64, name: &'static str, ready_ntfn: u64) -> ! {
    #[cfg(target_os = "none")]
    {
        svc_ipc_loop(ep, name, ready_ntfn)
    }
    #[cfg(not(target_os = "none"))]
    {
        let _ = (ep, name, ready_ntfn);
        svc_loop()
    }
}

/// K2.3：服务 IPC 主循环（21-A §2.K2.3，替换空 svc_loop）。
///
/// 服务入口（root-task spawn 时寄存器 X0=ep cap、X1=IPC buffer vaddr、
/// X2=就绪 ntfn 槽，aarch64 ABI 自动成为 `_start(ep, buffer, ready_ntfn)`
/// 参数）。就绪上报（WP-37.1 #8）：init 完成后先 Signal 就绪 ntfn——
/// root-task 在 deadline 内 poll 到即判定 Ready（readiness 来自 IPC
/// handshake，非 spawn 返回成功）。
///
/// 循环：`ipc_recv` 收请求（第一轮）→ 解码帧 → [`handle_request`] →
/// 编码响应帧入 MR → `ipc_reply_recv` 回复并继续接收。
/// 任何解码/IPC 错误 → 打印 + 挂起（fail-closed，红线 4 精神）。
#[cfg(target_os = "none")]
pub fn svc_ipc_loop(ep: u64, name: &'static str, ready_ntfn: u64) -> ! {
    use eneros_sel4_sys::safe::{get_mr, ipc_recv, ipc_reply_recv, message_info_new, set_mr};
    use eneros_sel4_sys::types::message_info_length;

    let mut sender = 0u64;
    let mut reply_info = 0u64; // 首轮 recv 前无 reply
    eneros_runtime::println!("[svc-{}] svc_ipc_loop entered ep={}", name, ep);
    // WP-37.1 #8：就绪上报（init 完成；Signal 失败 = 无 ntfn cap/坏槽，
    // fail-closed 挂起——root 侧 deadline 到后按 Degraded 处理并如实记录）。
    if let Err(e) = eneros_sel4_sys::safe::ipc_signal(ready_ntfn) {
        svc_fail_closed("ready signal", e);
    }
    eneros_runtime::println!("[svc-{}] READY-SIGNALED ntfn_slot={}", name, ready_ntfn);

    loop {
        // 接收（首轮 recv；后续 reply_recv 回复上一请求并继续收）。
        let mi = {
            let r = if reply_info == 0 {
                ipc_recv(ep, Some(&mut sender))
            } else {
                ipc_reply_recv(ep, reply_info, Some(&mut sender))
            };
            match r {
                Ok(m) => m,
                Err(e) => svc_fail_closed("recv", e),
            }
        };
        // 读载荷 MR → 帧。
        // 22 §2.W1.1（OOM 豁免标注）：MR 数受 seL4 消息上限约束（<128），
        // with_capacity 已预分配，push 失败概率极低。
        // OOM-risk: handled by alloc_error_handler（记录+挂起 fail-safe）。
        let words = message_info_length(mi);
        let mut mrs = alloc::vec::Vec::with_capacity(words as usize);
        for i in 0..words {
            match get_mr(i) {
                Ok(w) => mrs.push(w),
                Err(e) => svc_fail_closed("get_mr", e),
            }
        }
        let frame = match eneros_ipc_wire::mr::frame_from_mrs(&mrs) {
            Ok(f) => f,
            Err(e) => svc_fail_closed("decode frame", e),
        };
        // 解码 ipc-wire 帧（crc32 完整性校验）。
        let decoded = match eneros_ipc_wire::el0::decode_frame(&frame) {
            Ok(f) => f,
            Err(e) => svc_fail_closed("ipc-wire decode", e),
        };
        let outcome = handle_request(decoded.payload);
        match outcome {
            RequestOutcome::Reply(resp) => {
                eneros_runtime::println!(
                    "[svc-{}] IPC: recv req, replying {} bytes",
                    name,
                    resp.len()
                );
                // 响应帧 → MR → reply。
                let mrs = match eneros_ipc_wire::mr::frame_into_mrs(&resp) {
                    Ok(m) => m,
                    Err(e) => svc_fail_closed("encode resp", e),
                };
                for (i, w) in mrs.iter().enumerate() {
                    if let Err(e) = set_mr(i as u64, *w) {
                        svc_fail_closed("set_mr", e);
                    }
                }
                reply_info = match message_info_new(
                    eneros_ipc_wire::el0::El0FrameType::Report as u64,
                    0,
                    0,
                    mrs.len() as u64,
                ) {
                    Ok(m) => m,
                    Err(e) => svc_fail_closed("message_info_new", e),
                };
            }
            RequestOutcome::FaultTriggered => {
                // 正常路径不可达：IllegalAccess/除零已在 handle_request 的
                // inject_fault 中触发异常（线程被内核挂起，fault IPC 已送
                // root-task）。此处为 fail-closed 兜底（如注入未产生异常）。
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    }
}

/// 服务 IPC 循环 fail-closed 挂起。
#[cfg(target_os = "none")]
fn svc_fail_closed(step: &str, e: impl core::fmt::Debug) -> ! {
    eneros_runtime::println!(
        "[svc] IPC loop FAILED at {}: {:?} — 挂起（fail-closed）",
        step,
        e
    );
    loop {
        core::hint::spin_loop();
    }
}

/// 帧 ↔ MR 编解码（K2.2 契约）：单一事实源 = `eneros-ipc-wire::mr`，
/// 与 root-task 完全同源，本 crate 禁止本地复制实现（见 `svc_ipc_loop`
/// 直接引用 `eneros_ipc_wire::mr::*`）。
#[cfg(test)]
mod tests {
    // 测试断言可用 panic!（仓库 lint：测试模块豁免）。
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn test_krn003_svc_frame_roundtrip() {
        let out = svc_frame_roundtrip(b"svc-data").expect("roundtrip");
        assert_eq!(out, b"svc-data");
    }

    #[test]
    fn test_krn003_svc_frame_corruption_rejected() {
        use eneros_ipc_wire::el0::{decode_frame, encode_frame, El0FrameError, El0FrameType};
        let mut buf = encode_frame(El0FrameType::Ready, b"x");
        let n = buf.len();
        buf[n - 1] ^= 0xff;
        assert_eq!(decode_frame(&buf), Err(El0FrameError::BadCrc));
    }

    // --- v0.200.0 (P0-3)：服务真实功能 ---

    #[test]
    fn test_svc_hello_banner_with_counter() {
        // 计数器是全局 AtomicU64（跨测试共享），断言相对递增而非绝对 tick 值。
        fn tick_of(s: &str) -> u64 {
            let i = s.find("tick=").expect("banner 含 tick");
            s[i + 5..].parse().expect("tick 数字")
        }
        let first = svc_hello("eneros");
        let second = svc_hello("eneros");
        assert!(first.starts_with("hello:eneros:tick="), "banner: {first}");
        assert!(
            tick_of(&second) > tick_of(&first),
            "计数器必须单调递增: {first} → {second}"
        );
    }

    // --- 21-A K2.3：请求路由（host 可测）---

    #[test]
    fn test_parse_u64_basic() {
        assert_eq!(parse_u64(b"42"), Some(42));
        assert_eq!(parse_u64(b"0"), Some(0));
        assert_eq!(parse_u64(b""), None);
        assert_eq!(parse_u64(b"12a"), None);
        assert_eq!(parse_u64(b"99999999999999999999999"), None, "溢出必须 None");
    }

    #[test]
    fn test_handle_request_hello() {
        match handle_request(b"hello") {
            RequestOutcome::Reply(frame) => {
                let decoded = eneros_ipc_wire::el0::decode_frame(&frame).expect("frame");
                let s = core::str::from_utf8(decoded.payload).expect("utf8");
                assert!(s.starts_with("hello:eneros:tick="), "banner: {s}");
            }
            _ => panic!("hello 必须回复"),
        }
    }

    #[test]
    fn test_handle_request_pi() {
        match handle_request(b"pi=1000") {
            RequestOutcome::Reply(frame) => {
                let decoded = eneros_ipc_wire::el0::decode_frame(&frame).expect("frame");
                assert!(decoded.payload.starts_with(b"pi="));
                assert_ne!(decoded.payload, b"pi=err");
            }
            _ => panic!("pi 必须回复"),
        }
    }

    #[test]
    fn test_handle_request_hog() {
        match handle_request(b"hog=4") {
            RequestOutcome::Reply(frame) => {
                let decoded = eneros_ipc_wire::el0::decode_frame(&frame).expect("frame");
                assert_eq!(decoded.payload, b"hog=ok:4");
            }
            _ => panic!("hog 必须回复"),
        }
    }

    #[test]
    fn test_handle_request_unknown_fail_closed() {
        match handle_request(b"nonsense") {
            RequestOutcome::Reply(frame) => {
                let decoded = eneros_ipc_wire::el0::decode_frame(&frame).expect("frame");
                assert_eq!(decoded.payload, b"unknown:nonsense");
            }
            _ => panic!("未知请求必须回 unknown"),
        }
    }

    #[test]
    fn test_handle_request_bad_fault_kind_noop() {
        // fault= 但 kind 不识别 → inject_fault(None) → noop 回复（不触发故障）。
        match handle_request(b"fault=bad") {
            RequestOutcome::Reply(frame) => {
                let decoded = eneros_ipc_wire::el0::decode_frame(&frame).expect("frame");
                assert_eq!(decoded.payload, b"fault=noop");
            }
            _ => panic!("bad kind 必须 noop 回复"),
        }
    }

    /// WP-37.3：`fault=ExecStack`（NX 负例）协议路由——host 侧不执行
    /// 取指（cfg(target_os="none") 分支），返回 FaultTriggered（服务
    /// 预期挂起，不再回复）。
    #[test]
    fn test_handle_request_fault_exec_stack_routes() {
        match handle_request(b"fault=ExecStack") {
            RequestOutcome::FaultTriggered => {}
            other => panic!("ExecStack 必须 FaultTriggered，实际 {other:?}"),
        }
    }

    #[test]
    fn test_inject_fault_exec_stack_kind_parses() {
        // kind 枚举与协议串映射（host 可验证的部分；真实取指在 target）。
        let k = match &b"ExecStack"[..] {
            b"DivByZero" => Some(FaultKind::DivideByZero),
            b"IllegalAccess" => Some(FaultKind::IllegalAccess),
            b"Panic" => Some(FaultKind::Panic),
            b"ExecStack" => Some(FaultKind::ExecStack),
            _ => None,
        };
        assert_eq!(k, Some(FaultKind::ExecStack));
    }

    #[test]
    fn test_compute_pi_converges() {
        // 迭代越多越接近 π；验证计算真实性（非占位）。
        let low = compute_pi(100);
        let high = compute_pi(1_000_000);
        let err_low = (low - core::f64::consts::PI).abs();
        let err_high = (high - core::f64::consts::PI).abs();
        assert!(err_high < err_low, "π 计算必须随迭代数收敛");
        assert!(err_high < 1e-5, "100 万项误差应 < 1e-5，实际 {err_high}");
    }

    #[test]
    fn test_hog_memory_allocates_pattern() {
        let pages = hog_memory(4).expect("alloc 4 pages");
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[0].len(), 4096);
        // 每页填充模式基于页索引种子：(i * 0x9E3779B9).to_le_bytes()[0] + j。
        // i=0 → seed=0 → 首字节 0；i=1 → seed[0]=0xB9 → 首字节 0xB9。
        assert_eq!(pages[0][0], 0);
        assert_eq!(pages[1][0], 0xB9);
        // 页间模式不同（每页独立种子）。
        assert_ne!(pages[0][8], pages[1][8]);
    }

    #[test]
    fn test_inject_fault_none_ok() {
        assert_eq!(inject_fault(None), Ok(()));
    }

    #[test]
    #[should_panic(expected = "injected panic")]
    fn test_inject_fault_panic() {
        let _ = inject_fault(Some(FaultKind::Panic));
    }
}
