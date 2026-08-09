//! root-task IPC 主循环（KRN-004 / K2.2 / 21-A）：BootInfo 消费后进入
//! EL0 服务 IPC 调度——hello 往返、故障注入、supervisor 重启。
//!
//! W2.2（蓝图23）：删除了"帧分发影子 API"（`dispatch_frame`/
//! `DispatchAction`/`config_service_roundtrip`/`service_name`/
//! `load_services_from_bundle`）——这套纯函数有测试但真实 [`run_ipc_loop`]
//! 从不调用它（用的是 `hello_roundtrip`/`send_frame` 直连 seL4 IPC），
//! 测的不是跑的。MR 帧编解码契约（真实路径共享）保留 host 测试覆盖。

pub use eneros_ipc_wire::mr::{
    frame_from_mrs, frame_into_mrs, FrameMrError, MAX_FRAME_BYTES, MR_FRAME_LEN_SLOT,
};

extern crate alloc;

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
pub fn run_ipc_loop(
    handles: &[crate::spawn::SpawnedHandle],
    irq_table: crate::irq::IrqTable,
    irq_binding: Option<crate::irq::IrqBinding>,
    mut irq_drivers: alloc::vec::Vec<crate::driver_probe::IrqDriver>,
) -> ! {
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
    // fault 识别：poll 的 sender = fault_ep 的 badge（服务索引+1，spawn 时
    // mint 设置；0 保留 = 未知）。23 G-I 总出口确认：seL4 fault IPC 的
    // sender = fault handler endpoint 的 badge（doFaultTransfer 写入 badge
    // 寄存器）——badge 必须设在 fault_ep mint（服务 CSpace 槽 1），而非
    // notification（1368180 的错位实现，QEMU 实测 sender=0）。
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

    let mut irq_count: u64 = 0;
    let mut irq_seq: u64 = 0;
    loop {
        // 22 §2.W1.6：每轮喂硬件狗（喂狗间隔 < 看门狗超时 10s）。
        eneros_watchdog::api::wdt_kick();

        // WP-26.8（26-C §2.2）：IRQ poll（方案乙：独立 ntfn）——中断触发 →
        // badge ntfn 置位；sender != 0 即有中断；处理后必须 Ack。
        if let Some(binding) = &irq_binding {
            let mut irq_sender = 0u64;
            if let Ok(_mi) = ipc_poll(binding.ntfn_slot, Some(&mut irq_sender)) {
                if irq_sender != 0 {
                    irq_count += 1;
                    if irq_count % 8 == 1 || irq_count < 4 {
                        eneros_runtime::println!(
                            "[eneros-root-task] IRQ: irq={} badge={} count={}",
                            binding.irq,
                            irq_sender,
                            irq_count
                        );
                    }
                    if let Err(e) = irq_table.ack(binding) {
                        ipc_fail_closed("irq ack", e);
                    }
                    irq_seq += 1;
                }
            }
        }

        // WP-27.3 D1：驱动 IRQ 收包接线（S4：IRQ ntfn 与服务 EP 分槽——
        // 每个驱动 IRQ 独立 ntfn，与既有 timer IRQ 并行走 poll；badge 由
        // IrqTable::route 语义保证唯一）。
        for drv in irq_drivers.iter_mut() {
            let mut drv_sender = 0u64;
            if let Ok(_mi) = ipc_poll(drv.binding.ntfn_slot, Some(&mut drv_sender)) {
                if drv_sender != 0 {
                    drv.driver.handle_irq(drv.irq);
                    if let Err(e) = irq_table.ack(&drv.binding) {
                        ipc_fail_closed("driver irq ack", e);
                    }
                }
            }
        }

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
        // 23 G-I 总出口修复：`seL4_Poll` 无消息时返回的 MessageInfo 未定义
        // （QEMU 实测 0x2239c8——垃圾值，length>0 误判为有消息 → 每轮
        // `unknown fault sender=0`）。fault IPC 必有 badge（= 服务索引+1），
        // 以 `sender != 0` 为有消息判据。
        let mut sender = 0u64;
        if let Ok(_mi) = ipc_poll(fault.ntfn, Some(&mut sender)) {
            let has_msg = sender != 0;
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

    // 寄存器布局由 spawn::initial_registers 单一持有（W2.1）——与 configure_service
    // 共享同一构造点，count=5 契约由该函数保证。
    let regs = crate::spawn::initial_registers(handle.entry, handle.stack_top, handle.buffer_vaddr);
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
    use eneros_ipc_wire::el0::{decode_frame, encode_frame, El0FrameType};

    use super::*;

    // 21-A K2.2：MR 帧编解码往返（root-task ↔ 服务共享契约）。
    // 真实路径 `hello_roundtrip`/`send_frame` 走的就是 frame_into_mrs/
    // frame_from_mrs——这是测的即跑的（W2.2 红线5）。
    #[test]
    fn test_frame_mr_roundtrip() {
        let frame = encode_frame(El0FrameType::Report, b"hello:eneros:tick=1");
        let mrs = frame_into_mrs(&frame).expect("encode");
        assert_eq!(mrs[MR_FRAME_LEN_SLOT as usize], frame.len() as u64);
        let out = frame_from_mrs(&mrs).expect("decode");
        assert_eq!(out, frame, "MR 往返必须无损");
    }

    // W2.2（蓝图23）：真实 run_ipc_loop 往返契约——hello 请求帧与 fault 注入帧
    // 经 MR 编码→解码后，帧类型与 payload 必须完整保留（hello_roundtrip 依赖此
    // 契约解析响应；send_frame 依赖此契约投递故障请求）。这是真实 IPC loop 的
    // host 可测覆盖，替代已删的影子 API 测试。
    #[test]
    fn test_ipc_loop_frame_contract_roundtrip() {
        // hello 请求（hello_roundtrip 发送/接收的帧）。
        let hello = encode_frame(El0FrameType::Report, b"hello");
        let hello_back = frame_from_mrs(&frame_into_mrs(&hello).expect("enc")).expect("dec");
        let f = decode_frame(&hello_back).unwrap();
        assert_eq!(f.frame_type, El0FrameType::Report);
        assert_eq!(f.payload, b"hello");

        // fault 注入请求（send_frame 每 50 tick 发送一次）。
        let fault = encode_frame(El0FrameType::Report, b"fault=IllegalAccess");
        let fault_back = frame_from_mrs(&frame_into_mrs(&fault).expect("enc")).expect("dec");
        let f2 = decode_frame(&fault_back).unwrap();
        assert_eq!(f2.payload, b"fault=IllegalAccess");
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
