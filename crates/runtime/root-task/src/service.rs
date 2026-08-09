//! root-task 服务化主循环（KRN-004）：BootInfo 消费后进入 EL0 服务调度。
//!
//! M2.1（seL4 TCB/CSpace/VSpace FFI）前，服务循环以纯逻辑形式存在并
//! host 可测：接收 EL0 帧（ipc-wire el0），按类型分发——
//! - `Ready`：服务就绪上报 → 记录服务集（svc-hello/svc-pi/svc-hog/svc-fault）；
//! - `Report`：数据上报（含 config 请求，KRN-004 config 服务接入点）；
//! - `Control`：root-task 下发（spawn 确认 / 心跳）。
//!
//! QEMU 端到端（真实 seL4 Endpoint IPC）验证 BLOCKED（见
//! evidence/arch-convergence/krn-003.md 解锁条件）。

use eneros_ipc_wire::el0::El0FrameType;
pub use eneros_ipc_wire::mr::{
    frame_from_mrs, frame_into_mrs, FrameMrError, MAX_FRAME_BYTES, MR_FRAME_LEN_SLOT,
};

extern crate alloc;

use alloc::string::ToString;

/// 服务调度结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchAction {
    /// 记录就绪服务
    ServiceReady(alloc::string::String),
    /// 转发数据上报（config 服务入口，KRN-004）
    ForwardReport,
    /// 下发控制帧
    EmitControl,
    /// 未知帧类型（解码已拒绝，不扩散）
    Ignore,
}

/// 服务循环单步：解码一帧并分发。
///
/// `registered` 是来自 init bundle manifest 的真实服务注册表
/// （v0.199.0 P0-2：`service_name()` 硬编码表已替换）。
pub fn dispatch_frame(
    buf: &[u8],
    registered: &[&str],
) -> Result<DispatchAction, eneros_ipc_wire::el0::El0FrameError> {
    let frame = eneros_ipc_wire::el0::decode_frame(buf)?;
    Ok(match frame.frame_type {
        El0FrameType::Ready => {
            DispatchAction::ServiceReady(service_name(frame.payload, registered))
        }
        El0FrameType::Report => DispatchAction::ForwardReport,
        El0FrameType::Control => DispatchAction::EmitControl,
    })
}

/// 从 Ready 帧 payload 提取服务名（首个 NUL 截断），并在注册表中匹配。
///
/// 未注册的服务名返回 "unknown"（fail-closed：不将未知服务记为就绪）。
/// v0.199.0：注册表来自 bundle manifest 而非硬编码字节表。
pub fn service_name(payload: &[u8], registered: &[&str]) -> alloc::string::String {
    let end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let name = &payload[..end];
    match core::str::from_utf8(name) {
        Ok(name) if registered.contains(&name) => name.to_string(),
        _ => "unknown".to_string(),
    }
}

/// KRN-004：config 服务接入点——Report 帧携带 config 读请求时，
/// 返回当前服务集合规模（host 侧闭环演示；真实 ConfigStore 服务由
/// runtime 层在 M2.1 后以 EL0 服务形态接入）。
pub fn config_service_roundtrip(
    registered_services: usize,
    buf: &[u8],
) -> Result<alloc::vec::Vec<u8>, eneros_ipc_wire::el0::El0FrameError> {
    let frame = eneros_ipc_wire::el0::decode_frame(buf)?;
    if frame.frame_type != El0FrameType::Report {
        return Err(eneros_ipc_wire::el0::El0FrameError::BadType);
    }
    let resp = alloc::format!("config:services={}", registered_services);
    Ok(eneros_ipc_wire::el0::encode_frame(
        El0FrameType::Report,
        resp.as_bytes(),
    ))
}

/// 一个已校验的 EL0 服务（可 spawn）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    /// 服务名（bundle manifest）。
    pub name: alloc::string::String,
    /// 服务 ELF（经 W^X 校验）。
    pub elf: alloc::vec::Vec<u8>,
}

/// 从 init bundle 解析并校验可 spawn 的 EL0 服务（P0-2，v0.199.0）。
///
/// 遍历 bundle 的 service/driver 模块，对每个 ELF 做 W^X 校验
/// （`eneros-elf-loader`，与 boot 链一致）；通过者进入服务注册表。
/// 这是 `_start` 在 M2.1 后执行的真实装载逻辑的 host 可测形式。
pub fn load_services_from_bundle(
    bundle: &crate::bundle::InitBundle,
) -> alloc::vec::Vec<ServiceEntry> {
    let mut services = alloc::vec::Vec::new();
    for module in &bundle.modules {
        if module.kind != crate::bundle::MODULE_KIND_SERVICE
            && module.kind != crate::bundle::MODULE_KIND_DRIVER
        {
            continue;
        }
        // W^X 校验：ELF 可执行段不得同时可写。
        match eneros_elf_loader::LoadedElf::parse(&module.blob) {
            Ok(_) => services.push(ServiceEntry {
                name: module.name.clone(),
                elf: module.blob.clone(),
            }),
            Err(_) => {
                // 校验失败的服务不入注册表（fail-closed）。
                continue;
            }
        }
    }
    services
}

// ============================================================================
// K2.2：root-task IPC 调度主循环（21-A，替换 spin_loop）
// ============================================================================

/// IPC 循环 fail-closed 挂起（红线 4：任何 IPC 错误 = 挂起，禁止降级运行）。
#[cfg(target_os = "none")]
fn ipc_fail_closed(step: &str, e: impl core::fmt::Debug) -> ! {
    eneros_runtime::println!(
        "[eneros-root-task] IPC loop FAILED at {}: {:?} — 挂起（fail-closed）",
        step,
        e
    );
    loop {
        core::hint::spin_loop();
    }
}

/// K2.2/K2.3：root-task IPC 主循环（21-A §2.K2.2/K2.3）。
///
/// 首轮迭代（fault 注入只执行一次，svc-fault 挂起后不再接收消息）：
/// 1. 向 svc-hello 发 hello 请求帧（`ipc_call` 同步往返），打印响应；
/// 2. 向 svc-fault 发 `fault=IllegalAccess` 请求（`ipc_send`——aarch64
///    无除零异常，用 null 地址写触发 Data Abort）；
/// 3. 在 svc-fault 的故障 notification 上阻塞 `ipc_recv`——收到通知即
///    svc-fault 已被内核挂起（故障隔离：不扩散到其他服务）；
/// 4. 再次 hello 往返验证 svc-hello 仍响应（隔离成立证据）。
/// 此后循环仅 hello 心跳（svc-hello tick 递增可见）。
///
/// 任何 IPC 步骤失败 → [`ipc_fail_closed`] 挂起（红线 4）。
#[cfg(target_os = "none")]
pub fn run_ipc_loop(handles: &[crate::spawn::SpawnedHandle]) -> ! {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};
    use eneros_sel4_sys::safe::{get_mr, ipc_call, ipc_poll, ipc_send, message_info_new, set_mr};

    // 服务表：按名寻址。
    let by_name = |name: &str| {
        handles
            .iter()
            .find(|h| h.name == name)
            .ok_or_else(|| alloc::format!("{name} not spawned"))
    };
    let hello = match by_name("svc-hello") {
        Ok(h) => h.clone(),
        Err(e) => ipc_fail_closed("lookup svc-hello", e),
    };
    eneros_runtime::println!(
        "[eneros-root-task] DBG hello.ep={} hello.tcb={} ep_type={}",
        hello.ep,
        hello.tcb,
        eneros_sel4_sys::safe::debug_cap_identify(hello.ep)
    );
    let fault = match by_name("svc-fault") {
        Ok(h) => h.clone(),
        Err(e) => ipc_fail_closed("lookup svc-fault", e),
    };
    eneros_runtime::println!(
        "[eneros-root-task] DBG fault.ep={} fault.ntfn={}",
        fault.ep,
        fault.ntfn
    );

    // 22 §2.W1.3：supervisor——每服务故障状态机（按 handles 顺序）+ fault
    // sender(TCB cap) → 服务索引 映射。故障注入周期性执行（注入前检查
    // supervisor 状态：Suspended/Faulted 不注入——seL4_Send 同步语义，
    // 向挂起服务 send 会永久阻塞 root-task）。
    let mut supervisor = crate::supervisor::Supervisor::new_for_count(handles.len(), now_ms());
    // fault 识别：poll 的 sender = notification badge（服务索引+1，
    // spawn 时 mint 设置；0 保留 = 未知）。
    // 22 §2.W1.6：看门狗接入（hello 心跳后第一轮喂狗；QEMU 无 SP805
    // 时软件模式 base=0，MMIO 不写）。
    let wdt_base = if crate::panic::WDT_MMIO_MAPPED.load(core::sync::atomic::Ordering::Relaxed) {
        crate::panic::WDT_BASE
    } else {
        0
    };
    eneros_watchdog::api::wdt_init(10_000, wdt_base);
    let mut fault_tick = 0u64;
    let mut tick = 0u64;
    let fault_idx = match handles.iter().position(|h| h.name == "svc-fault") {
        Some(i) => i,
        None => ipc_fail_closed("svc-fault not in supervisor table", "no name"),
    };

    loop {
        // 22 §2.W1.6：每轮喂硬件狗（喂狗间隔 < 看门狗超时 10s）。
        eneros_watchdog::api::wdt_kick();

        // 1. hello 往返。
        let resp = match hello_roundtrip(hello.ep) {
            Ok(f) => f,
            Err(e) => ipc_fail_closed("hello roundtrip", e),
        };
        let payload = match eneros_ipc_wire::el0::decode_frame(&resp) {
            Ok(f) => f.payload,
            Err(e) => ipc_fail_closed("decode hello resp", e),
        };
        eneros_runtime::println!(
            "[eneros-root-task] IPC: recv {}",
            core::str::from_utf8(payload).unwrap_or("<non-utf8>")
        );

        tick += 1;
        // 2. supervisor：轮询 fault ep（非阻塞）——检测服务故障并决策。
        let mut sender = 0u64;
        if let Ok(mi) = ipc_poll(fault.ntfn, Some(&mut sender)) {
            let has_msg = eneros_sel4_sys::types::message_info_length(mi) > 0 || sender != 0;
            if has_msg {
                let name = |i: usize| handles[i].name.as_str();
                if let Some(idx) = sender
                    .checked_sub(1)
                    .filter(|&b| (b as usize) < handles.len())
                    .map(|b| b as usize)
                {
                    supervisor.advance(now_ms());
                    match supervisor.on_service_fault(idx) {
                        Some(crate::supervisor::Action::Restart { delay_ms }) => {
                            eneros_runtime::println!(
                                "[eneros-root-task] SUPERVISOR: {} fault → restart in {}ms (fault_count={})",
                                name(idx),
                                delay_ms,
                                supervisor
                                    .services
                                    .get(idx)
                                    .map(|s| s.fault_count)
                                    .unwrap_or(0)
                            );
                            busy_wait_ms(delay_ms);
                            if let Err(e) = restart_service(&handles[idx]) {
                                ipc_fail_closed("restart service", e);
                            }
                            supervisor.on_restart_done(idx);
                            eneros_runtime::println!(
                                "[eneros-root-task] SUPERVISOR: {} restarted (total={})",
                                name(idx),
                                supervisor
                                    .services
                                    .get(idx)
                                    .map(|s| s.restart_count)
                                    .unwrap_or(0)
                            );
                            // 复验 svc-hello 仍响应（隔离成立）。
                            match hello_roundtrip(hello.ep) {
                                Ok(f) => {
                                    if let Ok(fr) = eneros_ipc_wire::el0::decode_frame(&f) {
                                        eneros_runtime::println!(
                                            "[eneros-root-task] SUPERVISOR: svc-hello still responsive ({})",
                                            core::str::from_utf8(&fr.payload)
                                                .unwrap_or("<non-utf8>")
                                        );
                                    }
                                }
                                Err(e) => ipc_fail_closed("hello recheck", e),
                            }
                        }
                        Some(crate::supervisor::Action::MarkFaulted) => {
                            eneros_runtime::println!(
                                "[eneros-root-task] SUPERVISOR: {} FAULTED (≥{} faults/hour) — no auto restart, alert",
                                name(idx),
                                crate::supervisor::FAULT_THRESHOLD_PER_HOUR
                            );
                        }
                        None => {}
                    }
                } else {
                    eneros_runtime::println!(
                        "[eneros-root-task] SUPERVISOR: unknown fault sender={} — ignore (fail-closed)",
                        sender
                    );
                }
            }
        }

        // 22 §2.W1.6：定期检查分层看门狗状态（诊断；无 layer 时为 AllFed）。
        if tick % 100 == 0 {
            match eneros_watchdog::api::wdt_check() {
                eneros_watchdog::layered::WatchdogStatus::AllFed => {}
                eneros_watchdog::layered::WatchdogStatus::LayerTimeout(id) => {
                    eneros_runtime::println!(
                        "[eneros-root-task] WDT: layer {:?} timeout — supervisor path",
                        id
                    );
                }
                eneros_watchdog::layered::WatchdogStatus::HardReset => {
                    eneros_runtime::println!(
                        "[eneros-root-task] WDT: HARD RESET due — arming reset"
                    );
                }
            }
        }

        // 3. 周期故障注入（每 50 轮 hello）：svc-fault Running 时才注入
        // （Suspended/Faulted 不注入，防 seL4_Send 阻塞 root-task）。
        fault_tick += 1;
        if fault_tick % 50 == 0
            && supervisor.state_of(fault_idx) == Some(crate::supervisor::ServiceState::Running)
        {
            eneros_runtime::println!("[eneros-root-task] IPC: send fault to svc-fault (cycle)");
            let req = encode_frame(El0FrameType::Report, b"fault=IllegalAccess");
            if let Err(e) = send_frame(fault.ep, &req) {
                ipc_fail_closed("send fault", e);
            }
        }

        // 循环回到 1：hello 心跳持续（svc-hello tick 递增可见）。
    }
}

/// 当前毫秒时间（supervisor 退避/窗口用；无墙钟，用单调时钟折算）。
#[cfg(target_os = "none")]
fn now_ms() -> u64 {
    eneros_time::get_monotonic_ns() / 1_000_000
}

/// 忙等指定毫秒（root-task 单线程无睡眠原语；退避等待用）。
#[cfg(target_os = "none")]
fn busy_wait_ms(ms: u64) {
    let start = eneros_time::get_monotonic_ns();
    let target = start + ms.saturating_mul(1_000_000);
    while eneros_time::get_monotonic_ns() < target {
        core::hint::spin_loop();
    }
}

/// 重启服务（22 §2.W1.3）：fault 后 TCB 被内核挂起且对象保留——
/// `tcb_write_registers` 重置 PC/SP/X0/X1（与 configure_service 相同的
/// 5 寄存器布局）+ `tcb_resume` 即从 `_start` 重新执行（无需
/// revoke/retype 对象；bss 未清零为已知语义——服务重启保留静态状态，
/// 生产服务由 supervisor 重启策略保证幂等性）。
#[cfg(target_os = "none")]
fn restart_service(
    handle: &crate::spawn::SpawnedHandle,
) -> Result<(), eneros_sel4_sys::safe::Sel4Error> {
    use eneros_sel4_sys::safe::{tcb_resume, tcb_write_registers};
    use eneros_sel4_sys::types::UserContext;

    let mut regs = UserContext::zeroed();
    regs.set_pc(handle.entry);
    regs.set_sp(handle.stack_top);
    regs.set_x(0, 0); // ep cap 在服务 CSpace 槽 0
    regs.set_x(1, handle.buffer_vaddr);
    // count=5: pc/sp/spsr/x0/x1（同 configure_service；漏写 x1 →
    // ipcbuf=0 → set_ipc_buffer(NULL) → VMFAULT）。
    tcb_write_registers(handle.tcb, 0, 5, &regs)?;
    tcb_resume(handle.tcb)
}

/// hello 请求→响应往返（K2.2 契约）。返回完整响应帧字节。
#[cfg(target_os = "none")]
fn hello_roundtrip(ep: u64) -> Result<alloc::vec::Vec<u8>, eneros_sel4_sys::safe::Sel4Error> {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};
    use eneros_sel4_sys::safe::{get_mr, ipc_call, message_info_new, set_mr};
    use eneros_sel4_sys::types::message_info_length;

    let req = encode_frame(El0FrameType::Report, b"hello");
    let mrs = eneros_ipc_wire::mr::frame_into_mrs(&req)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)?;
    for (i, w) in mrs.iter().enumerate() {
        set_mr(i as u64, *w)?;
    }
    let mi = message_info_new(El0FrameType::Report as u64, 0, 0, mrs.len() as u64)?;
    eneros_runtime::println!("[eneros-root-task] IPC: send hello-req to svc-hello");
    // 阻塞 call 前让出 CPU：服务（优先级 10）有机会先运行并 recv。
    eneros_sel4_sys::safe::yield_cpu();
    let reply = ipc_call(ep, mi)?;
    let words = message_info_length(reply);
    let mut mr_buf = alloc::vec::Vec::with_capacity(words as usize);
    for i in 0..words {
        mr_buf.push(get_mr(i)?);
    }
    eneros_ipc_wire::mr::frame_from_mrs(&mr_buf)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)
}

/// 发送一帧（fire-and-forget，K2.3 故障注入用）。
#[cfg(target_os = "none")]
fn send_frame(ep: u64, frame: &[u8]) -> Result<(), eneros_sel4_sys::safe::Sel4Error> {
    use eneros_ipc_wire::el0::El0FrameType;
    use eneros_sel4_sys::safe::{ipc_send, message_info_new, set_mr};

    let mrs = eneros_ipc_wire::mr::frame_into_mrs(frame)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)?;
    for (i, w) in mrs.iter().enumerate() {
        set_mr(i as u64, *w)?;
    }
    let mi = message_info_new(El0FrameType::Report as u64, 0, 0, mrs.len() as u64)?;
    ipc_send(ep, mi)
}

#[cfg(test)]
mod tests {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};

    use super::*;

    const REGISTERED: &[&str] = &["svc-hello", "svc-pi", "svc-hog", "svc-fault"];

    #[test]
    fn test_krn004_ready_dispatch() {
        let buf = encode_frame(El0FrameType::Ready, b"svc-hello\0");
        assert_eq!(
            dispatch_frame(&buf, REGISTERED).unwrap(),
            DispatchAction::ServiceReady("svc-hello".to_string())
        );
    }

    #[test]
    fn test_krn004_report_dispatch() {
        let buf = encode_frame(El0FrameType::Report, b"data");
        assert_eq!(
            dispatch_frame(&buf, REGISTERED).unwrap(),
            DispatchAction::ForwardReport
        );
    }

    #[test]
    fn test_krn004_config_service_roundtrip() {
        let buf = encode_frame(El0FrameType::Report, b"get:/runtime/services");
        let resp = config_service_roundtrip(4, &buf).expect("config roundtrip");
        let frame = eneros_ipc_wire::el0::decode_frame(&resp).expect("resp frame");
        assert_eq!(frame.payload, b"config:services=4");
    }

    #[test]
    fn test_krn004_corrupt_frame_ignored() {
        let mut buf = encode_frame(El0FrameType::Ready, b"svc-pi\0");
        let n = buf.len();
        buf[n - 1] ^= 0xff;
        assert_eq!(
            dispatch_frame(&buf, REGISTERED),
            Err(eneros_ipc_wire::el0::El0FrameError::BadCrc)
        );
    }

    // v0.199.0 (P0-2)：服务注册表来自 bundle manifest。
    #[test]
    fn test_ready_unregistered_service_unknown() {
        let buf = encode_frame(El0FrameType::Ready, b"svc-malicious\0");
        assert_eq!(
            dispatch_frame(&buf, REGISTERED).unwrap(),
            DispatchAction::ServiceReady("unknown".to_string())
        );
    }

    #[test]
    fn test_service_name_from_manifest_registry() {
        let manifest: &[&str] = &["svc-drv-rng", "svc-storage"];
        assert_eq!(service_name(b"svc-drv-rng\0", manifest), "svc-drv-rng");
        assert_eq!(service_name(b"svc-hello\0", manifest), "unknown");
    }

    // 21-A K2.2：MR 帧编解码往返（root-task ↔ 服务共享契约）。
    #[test]
    fn test_frame_mr_roundtrip() {
        let frame = encode_frame(El0FrameType::Report, b"hello:eneros:tick=1");
        let mrs = frame_into_mrs(&frame).expect("encode");
        assert_eq!(mrs[MR_FRAME_LEN_SLOT as usize], frame.len() as u64);
        let out = frame_from_mrs(&mrs).expect("decode");
        assert_eq!(out, frame, "MR 往返必须无损");
    }

    #[test]
    fn test_frame_mr_oversize_rejected() {
        let big = alloc::vec![0u8; MAX_FRAME_BYTES + 1];
        assert_eq!(frame_into_mrs(&big), Err(FrameMrError::FrameTooLong));
        assert_eq!(
            frame_from_mrs(&[(MAX_FRAME_BYTES + 1) as u64]),
            Err(FrameMrError::FrameTooLong)
        );
        assert_eq!(frame_from_mrs(&[]), Err(FrameMrError::NoLengthSlot));
        // 长度声明 16 但只有 2 个 word（1 长度 + 1 载荷）→ BadLength。
        assert_eq!(frame_from_mrs(&[16u64, 0]), Err(FrameMrError::BadLength));
    }

    #[test]
    fn test_frame_mr_partial_word_padding() {
        // 1 字节帧：MR0=1，MR1 只有低 8 位有效。
        let mrs = frame_into_mrs(&[0xAB]).expect("encode");
        assert_eq!(mrs, vec![1, 0xAB]);
        let out = frame_from_mrs(&mrs).expect("decode");
        assert_eq!(out, vec![0xAB]);
    }
}
