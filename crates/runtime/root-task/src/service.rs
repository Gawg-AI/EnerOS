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

/// WP-37.1 #8：readiness 判定结果（readiness 来自 IPC handshake/health
/// probe，非"spawn 返回成功"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessVerdict {
    /// 就绪通知在 deadline 内到达（handshake 通过）。
    Ready,
    /// deadline 内无就绪通知（服务未响应/未实现 IPC 循环）→ 显式降级，
    /// 不虚报 Ready（蓝图：optional 失败写 CapabilityUnavailable）。
    Degraded,
}

/// WP-37.1 #8：单服务 readiness 判定（**异步收集**，纯函数 host 可测）——
/// 每轮调用一次，非忙等轮询。语义：
/// - 就绪信号已到 → `Ready`；
/// - 未到且已超 deadline → `Degraded`（不虚报 Ready）；
/// - 未到且未超时 → `None`（继续轮询）。
///
/// target 主循环在 hello 等阻塞点之间调用（服务在此期间运行并 Signal
/// 就绪通知）；QEMU 实测 root 高优先级自旋 busy_wait 会饿死 EL0 服务，
/// 禁止用忙等实现 deadline。
pub fn check_readiness(
    start_ms: u64,
    deadline_ms: u64,
    now_ms: u64,
    signaled: bool,
) -> Option<ReadinessVerdict> {
    if signaled {
        Some(ReadinessVerdict::Ready)
    } else if now_ms.saturating_sub(start_ms) >= deadline_ms {
        Some(ReadinessVerdict::Degraded)
    } else {
        None
    }
}

/// WP-37.1 #8：readiness 探测 deadline（每服务；就绪 Signal 在服务 init
/// 完成后首个调度周期内到达，drv 占位服务自旋不 Signal → 超时降级）。
/// 单位毫秒（虚拟时钟；QEMU TCG 无 icount 时约慢 50-60×，1s 虚拟 ≈ 60s
/// 真实，落在 200s QEMU 预算内；真机虚拟≈真实时间时 1s 对首调度周期
/// 内到达的 Signal 仍属宽裕）。与 [`ReadinessContract::default_handshake`]
/// 同语义（超时 → 显式降级）。
/// 41：1_000 → 3_000——svc-config 启动含 1 次 SM2 bundle 验签（QEMU
/// TCG 实测 ~2s 虚拟），1s 窗口在验签期间耗尽 → required 误降级
/// （bundle 已装载、服务正常服务，仅 readiness 计数错过）。
#[allow(dead_code)] // 44.2：readiness 超时上限，当前路径未用（44.4 冻结登记）
const READINESS_DEADLINE_MS: u64 = 3_000;

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
    // 38（WP-38.8）：驱动所有权在 driver_supervisor——事件循环经 badge
    // 路由 handle_irq；存活期 = 本循环（驱动不再随局部变量 drop）。
    mut driver_supervisor: eneros_driver_framework::DriverSupervisor,
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
    // 44.3（探针可选）：svc-hello/svc-fault 是探针锚点——纯产品装配
    // （qemu-prod/board-prod）不 spawn 它们。主循环探针缺失时自定步幅
    // （busy_wait+yield），故障注入跳过；fail-closed 仅针对已 spawn 服务。
    let hello = match by_name("svc-hello") {
        Ok(h) => Some(h.clone()),
        Err(_) => {
            eneros_runtime::println!(
                "[eneros-root-task] DBG no svc-hello (probe absent) — self-paced supervisor"
            );
            None
        }
    };
    if let Some(h) = &hello {
        eneros_runtime::println!(
            "[eneros-root-task] DBG hello.ep={} hello.tcb={} ep_type={}",
            h.ep,
            h.tcb,
            eneros_sel4_sys::safe::debug_cap_identify(h.ep)
        );
    }
    let fault = match by_name("svc-fault") {
        Ok(h) => Some(h.clone()),
        Err(_) => {
            eneros_runtime::println!(
                "[eneros-root-task] DBG no svc-fault (probe absent) — fault injection disabled"
            );
            None
        }
    };
    if let Some(f) = &fault {
        eneros_runtime::println!(
            "[eneros-root-task] DBG fault.ep={} fault.ntfn={}",
            f.ep,
            f.ntfn
        );
    }

    // 22 §2.W1.3：supervisor——每服务故障状态机（按 handles 顺序）+ fault
    // sender(TCB cap) → 服务索引 映射。故障注入周期性执行（注入前检查
    // supervisor 状态：Suspended/Faulted 不注入——seL4_Send 同步语义，
    // 向挂起服务 send 会永久阻塞 root-task）。
    // 30 RT-029：真实服务名（旧实现 new_for_count 用占位名 `svc-<i>`，
    // SUPERVISOR 故障日志服务名无定位价值——故障重启真证据（WP-27.4）
    // 出证前必须关闭）。
    let service_names: alloc::vec::Vec<&str> = handles.iter().map(|h| h.name.as_str()).collect();
    let mut supervisor = crate::supervisor::Supervisor::new(&service_names, now_ms());
    // WP-37.2：能力授权填充——bundle flags 高 16 位（唯一授权来源）→
    // CapabilityGrantSet；未声明能力一律拒绝（构造保证）。打印 CAPS 行
    // 作为机器可读证据（声明即授予，无隐式授权）。
    for (idx, handle) in handles.iter().enumerate() {
        let caps = crate::bundle::capability_names(handle.capabilities)
            .unwrap_or_default()
            .join(",");
        for name in crate::bundle::capability_names(handle.capabilities).unwrap_or_default() {
            supervisor.services[idx].grants.grant(name);
        }
        eneros_runtime::println!(
            "[eneros-root-task] CAPS: {}={}",
            handle.name,
            if caps.is_empty() { "<none>" } else { &caps }
        );
    }
    // fault 识别：poll 的 sender = fault_ep 的 badge（服务索引+1，spawn 时
    // mint 设置；0 保留 = 未知）。23 G-I 总出口确认：seL4 fault IPC 的
    // sender = fault handler endpoint 的 badge（doFaultTransfer 写入 badge
    // 寄存器）——badge 必须设在 fault_ep mint（服务 CSpace 槽 1），而非
    // notification（1368180 的错位实现，QEMU 实测 sender=0）。
    // 22 §2.W1.6：看门狗接入（hello 心跳后第一轮喂狗；QEMU 无 SP805
    // 时软件模式 base=0，MMIO 不写）。
    let wdt_base = if crate::panic::WDT_MMIO_MAPPED.load(core::sync::atomic::Ordering::Relaxed) {
        crate::panic::WDT_BASE.load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    };
    eneros_watchdog::api::wdt_init(10_000, wdt_base);
    // 38（WP-38.8）：进入事件循环前打印驱动生命周期汇总（证据行）。
    eneros_runtime::println!(
        "[eneros-root-task] ENEROS stage=event-loop-drivers {} healthy={}",
        driver_supervisor.report_line(),
        driver_supervisor.healthy_count()
    );
    let mut fault_tick = 0u64;
    let mut tick = 0u64;
    // 33 WP-33.5：驱动 IRQ 计数（每驱动一槽；UART RX 注入时递增）。
    let mut drv_irq_counts: alloc::vec::Vec<u64> = alloc::vec![0; irq_drivers.len()];
    // 44.3：svc-fault 探针可选——缺失时 fault_idx=None，注入块跳过。
    let fault_idx = handles.iter().position(|h| h.name == "svc-fault");

    // WP-37.1 #8：readiness **异步收集**——readiness 必须来自 IPC
    // handshake/health probe，非"spawn 返回成功"（蓝图 WP-37.1 #8）。
    // 服务 resume 后在其首个调度周期内 Signal 就绪 ntfn；root 在主循环
    // 阻塞点（hello 往返）之间 poll 收集。QEMU 实测：root 高优先级
    // 自旋 busy_wait 饿死 EL0 服务，故禁止同步忙等探测。
    // - 就绪 → 生命周期 Started→Ready；
    // - deadline 内无信号（svc-drv-* 占位自旋）→ Started→Degraded，
    //   显式降级不虚报（35-00 P0-08 占位债务由 38/39 清偿）；
    // 全部判定后打印汇总行 PRODUCT-READY（Exit Gate #1 判据）。
    let mut readiness_start_ms: alloc::vec::Vec<u64> = alloc::vec![now_ms(); handles.len()];
    // None = 未判定（继续收集）；Some = 已判定。
    let mut readiness_state: alloc::vec::Vec<Option<ReadinessVerdict>> =
        alloc::vec![None; handles.len()];
    let mut readiness_summary_printed = false;

    let mut irq_count: u64 = 0;
    let mut irq_seq: u64 = 0;

    // ============================================================================
    // 蓝图 40 收尾：控制链驱动 + 心跳监控（WP-40.4 隔离面 + Exit #6/#10
    // 实弹）。root 持全部服务 ep——作为采集链代理与编排触发器。
    // ============================================================================
    let telemetry_handle = handles.iter().find(|h| h.name == "svc-telemetry").cloned();
    let energy_handle = handles
        .iter()
        .find(|h| h.name == "svc-energy-agent")
        .cloned();
    let hang_handle = handles.iter().find(|h| h.name == "svc-hang").cloned();
    // 41（WP-41.3 落盘）：svc-ota 经 EIPC 调 svc-storage 真落盘——
    // root 作为编排方触发一次 PERSIST+LOAD 往返验证（trial 状态写盘
    // 再读回；证据行 STORAGE-PERSIST）。
    let ota_handle = handles.iter().find(|h| h.name == "svc-ota").cloned();
    if let (Some(t), Some(e)) = (&telemetry_handle, &energy_handle) {
        eneros_runtime::println!(
            "[eneros-root-task] CONTROL-CHAIN: drivers bound telemetry.ep={} energy.ep={}",
            t.ep,
            e.ep
        );
    } else {
        eneros_runtime::println!(
            "[eneros-root-task] CONTROL-CHAIN: telemetry/energy-agent not spawned — chain idle"
        );
    }
    // 心跳监控状态：最近心跳轮次 / root 最近发请求轮次（-1 = 未发生）。
    // 判定 = "发过请求却无进展"（probe_sent ≥ 0 且 tick - last_beat 超限）
    // ——空闲服务无请求在途，不误判。
    let mut hb_last_beat: alloc::vec::Vec<i64> = alloc::vec![-1; handles.len()];
    let mut hb_probe_sent: alloc::vec::Vec<i64> = alloc::vec![-1; handles.len()];
    /// 心跳缺失容忍轮次。必须 **> 驱动周期**（正常服务在两驱动轮间无
    /// 请求无心跳——probe 已清除不误杀；svc-hang probe 未清除 → 超限
    /// kill）。150 轮 ≈ 0.2s 真实（hello ~760 轮/s）。
    const HEARTBEAT_MISS_LIMIT: i64 = 150;
    let mut collect_seq: u64 = 0;
    let mut chain_epoch: u64 = 0;
    let mut chain_rounds: u64 = 0;
    let mut hang_injected = false;
    /// 控制链驱动周期（轮）。QEMU 实弹验证（round 21）：100 轮 ≈ 0.13s
    /// 虚拟/次，hello 760 轮/s；dispatch rate gate 已按 demo profile 放宽
    /// （svc-control-dispatch 装配 DecisionGate 100000/h），不撞限。
    const CONTROL_CHAIN_PERIOD: u64 = 100;
    /// hang 注入轮次（链稳定后一次性）。
    const HANG_INJECT_ROUND: u64 = 200;
    // 41（WP-41.3 落盘）：存储持久化验证轮次（服务就绪后一次性；QEMU
    // 300s 预算内 tick 仅 ~100——60 轮时服务已 Ready，探针可触发）。
    let mut storage_persist_done = false;
    const STORAGE_PERSIST_ROUND: u64 = 60;
    let mut ota_download_done = false;
    const OTA_DOWNLOAD_ROUND: u64 = 70;

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
        // 33 WP-33.5：驱动 IRQ 计数判据行（pl011 RX 等驱动中断 → 计数
        // 递增打印；UART 输入注入时可见递增，不注入则保持 0 如实）。
        for (i, drv) in irq_drivers.iter_mut().enumerate() {
            let mut drv_sender = 0u64;
            if let Ok(_mi) = ipc_poll(drv.binding.ntfn_slot, Some(&mut drv_sender)) {
                if drv_sender != 0 {
                    // 38（WP-38.8）：驱动在 driver_supervisor 内（单所有权），
                    // 按 badge 路由 handle_irq。
                    if let Some(d) = driver_supervisor.driver_for_badge(drv.badge) {
                        d.handle_irq(drv.irq);
                    }
                    if let Err(e) = irq_table.ack(&drv.binding) {
                        ipc_fail_closed("driver irq ack", e);
                    }
                    drv_irq_counts[i] += 1;
                    if drv_irq_counts[i] % 8 == 1 || drv_irq_counts[i] < 4 {
                        eneros_runtime::println!(
                            "[eneros-root-task] DRIVER-IRQ: irq={} count={}",
                            drv.irq,
                            drv_irq_counts[i]
                        );
                    }
                }
            }
        }

        // 39 WP-39.7：virtio-net 外部 peer 回显（0x88b5 ENEROS-NET-RX →
        // TX；非阻塞，无网卡/无帧立即返回）。
        crate::driver_probe::net_echo_poll();

        // 1. 心跳/liveness 锚点往返（svc-hello 探针可选）。
        match &hello {
            Some(hello) => {
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
            }
            None => {
                // 探针缺失：自定步幅（root 优先级低于服务优先级，不饿死
                // EL0；yield 让出调度）。纯产品装配下的 supervisor 节拍。
                busy_wait_ms(1);
                eneros_sel4_sys::safe::yield_cpu();
            }
        }

        tick += 1;

        // WP-37.1 #8：readiness 异步收集（每轮在 hello 阻塞点后 poll；
        // 服务在此期间已运行并 Signal 就绪通知）。全部判定后打印一次
        // 汇总行 PRODUCT-READY。
        let mut any_pending = false;
        for (idx, handle) in handles.iter().enumerate() {
            if readiness_state[idx].is_some() {
                continue;
            }
            let mut sender = 0u64;
            let signaled = if let Ok(_mi) = ipc_poll(handle.ready_ntfn, Some(&mut sender)) {
                sender != 0
            } else {
                false
            };
            match check_readiness(
                readiness_start_ms[idx],
                READINESS_DEADLINE_MS,
                now_ms(),
                signaled,
            ) {
                Some(verdict) => {
                    // spawn 已完成 → 生命周期推进到 Started（readiness
                    // 判定的合法基线），再转 Ready/Degraded。
                    supervisor.services[idx].advance_lifecycle_to_started();
                    let _ = supervisor.services[idx].lifecycle_transition(match verdict {
                        ReadinessVerdict::Ready => crate::supervisor::LifecycleState::Ready,
                        ReadinessVerdict::Degraded => crate::supervisor::LifecycleState::Degraded,
                    });
                    readiness_state[idx] = Some(verdict);
                    eneros_runtime::println!(
                        "[eneros-root-task] READINESS: {}={:?}",
                        handle.name,
                        verdict
                    );
                }
                None => any_pending = true,
            }
        }
        if !any_pending && !readiness_summary_printed {
            readiness_summary_printed = true;
            let degraded: alloc::vec::Vec<&str> = handles
                .iter()
                .zip(readiness_state.iter())
                .filter(|(_, v)| **v == Some(ReadinessVerdict::Degraded))
                .map(|(h, _)| h.name.as_str())
                .collect();
            // WP-37.1：PRODUCT-READY 判据 = **required 服务**全 Ready
            // （root 运行到本行即 Ready——elfloader 装载，无 readiness
            // 轮询；optional 降级只写 CapabilityUnavailable，不阻断）。
            let required_degraded: alloc::vec::Vec<&str> = handles
                .iter()
                .zip(readiness_state.iter())
                .filter(|(h, v)| {
                    h.requirement == crate::bundle::Requirement::Required
                        && **v == Some(ReadinessVerdict::Degraded)
                })
                .map(|(h, _)| h.name.as_str())
                .collect();
            eneros_runtime::println!(
                "[eneros-root-task] PRODUCT-READY: {} (degraded: {}; required degraded: {})",
                required_degraded.is_empty(),
                degraded.join(","),
                required_degraded.join(",")
            );
        }

        // 蓝图 40 收尾：控制链驱动（采集代理 INGEST → 编排 TICK）。
        // 诚实边界：39 采集链设备句柄移交（CVR-39-02，41 Entry 前置）未
        // 完成——root 作为**采集链代理**注入样本（source=1 与 svc-telemetry
        // 覆盖声明一致）；样本经 svc-telemetry evaluator 全量校验
        // （coverage/freshness/quality/integrity），不绕过信任评估。
        if tick % CONTROL_CHAIN_PERIOD == 5 {
            if let (Some(t), Some(e)) = (&telemetry_handle, &energy_handle) {
                hb_probe_sent[handles
                    .iter()
                    .position(|h| h.name == "svc-telemetry")
                    .unwrap()] = tick as i64;
                hb_probe_sent[handles
                    .iter()
                    .position(|h| h.name == "svc-energy-agent")
                    .unwrap()] = tick as i64;
                match control_chain_tick(t.ep, e.ep, &mut collect_seq, &mut chain_epoch) {
                    Ok((accepted, ea_status, disp_state)) => {
                        chain_rounds += 1;
                        let ea = match ea_status {
                            0 => "OK",
                            1 => "NOACTION",
                            2 => "UNAVAILABLE",
                            _ => "INVALID",
                        };
                        // dispatch 终态码与 DispatchState 编码对齐
                        //（encode_receipt 布局：status1+state1+…）。
                        let disp = match disp_state {
                            Some(8) => "Committed",
                            Some(9) => "Rejected",
                            Some(10) => "Failed",
                            Some(11) => "Indeterminate",
                            Some(_) => "other",
                            None => "-",
                        };
                        eneros_runtime::println!(
                            "[eneros-root-task] CONTROL-CHAIN: round={} telemetry-ingest={} agent={} dispatch={}",
                            chain_rounds,
                            accepted,
                            ea,
                            disp
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] CONTROL-CHAIN: transport error ({e}) — fail-closed"
                        );
                    }
                }
            }
        }

        // 蓝图 40 收尾：hang 注入（一次性；svc-hang 为心跳超时注入参考
        // 服务——蓝图 §9 isolation 行实弹）。单向 Send 裸帧（svc-hang
        // 自定协议，非 el0 wire；卡死后 root 不阻塞）。
        if !hang_injected && tick == HANG_INJECT_ROUND {
            if let Some(h) = &hang_handle {
                hang_injected = true;
                if let Some(idx) = handles.iter().position(|x| x.name == "svc-hang") {
                    hb_probe_sent[idx] = tick as i64;
                }
                eneros_runtime::println!(
                    "[eneros-root-task] HEARTBEAT-TEST: inject hang to svc-hang (one-shot)"
                );
                use eneros_sel4_sys::safe::{ipc_send, message_info_new, set_mr};
                let _ = set_mr(0, u64::from_le_bytes(*b"hang\0\0\0\0"));
                let mi = match message_info_new(0, 0, 0, 1) {
                    Ok(m) => m,
                    Err(e) => ipc_fail_closed("hang mi", e),
                };
                if let Err(e) = ipc_send(h.ep, mi) {
                    ipc_fail_closed("send hang", e);
                }
            }
        }

        // 41（WP-41.3 落盘）：存储真落盘往返验证（一次性）——root 经
        // EIPC 调 svc-ota PERSIST-TRIAL（trial 状态经 svc-storage 块命令
        // 面真写数据盘）再 LOAD-TRIAL 读回。证据行 STORAGE-PERSIST。
        if !storage_persist_done && tick >= STORAGE_PERSIST_ROUND {
            if let Some(h) = &ota_handle {
                storage_persist_done = true;
                match storage_persist_probe(h.ep, &mut chain_epoch) {
                    Ok(()) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] STORAGE-PERSIST: OK (trial persisted + reloaded via svc-storage)"
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] STORAGE-PERSIST: FAILED ({e}) — fail-closed 不伪造"
                        );
                    }
                }
            }
        }

        // 41（OTA A/B）：真实签名镜像 download→verify→trial→commit 探针。
        if !ota_download_done && tick >= OTA_DOWNLOAD_ROUND {
            if let Some(h) = &ota_handle {
                ota_download_done = true;
                match ota_download_probe(h.ep, &mut chain_epoch) {
                    Ok(()) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] OTA-DOWNLOAD: OK (real A/B download/verify/trial/commit)"
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] OTA-DOWNLOAD: FAILED ({e}) — fail-closed 不伪造"
                        );
                    }
                }
            }
        }

        // 蓝图 40 收尾：心跳监控（WP-40.4）——poll 受监控服务心跳 ntfn；
        // "发过请求却无进展超限" → suspend + restart（无 fault 卡死也终止，
        // 不拖垮控制服务）。kill 后清 probe 记录（重启即翻篇）。
        for (idx, handle) in handles.iter().enumerate() {
            if handle.heartbeat_ntfn == 0 {
                continue;
            }
            let mut sender = 0u64;
            if let Ok(_mi) = ipc_poll(handle.heartbeat_ntfn, Some(&mut sender)) {
                if sender != 0 {
                    hb_last_beat[idx] = tick as i64;
                    // probe 后已响应（心跳 ≥ probe 轮）：本轮请求完成，
                    // 解除 pending——服务回到空闲不被误杀（QEMU 实弹：
                    // 驱动周期 > 判限时无此清除必误杀空闲服务）。svc-hang
                    // 注入轮无心跳（hang 判定先于 signal），probe 保留 →
                    // 超限 kill。
                    if hb_probe_sent[idx] >= 0 && hb_last_beat[idx] >= hb_probe_sent[idx] {
                        hb_probe_sent[idx] = -1;
                    }
                }
            }
            let sent = hb_probe_sent[idx];
            if sent >= 0 && (tick as i64) - hb_last_beat[idx] > HEARTBEAT_MISS_LIMIT {
                eneros_runtime::println!(
                    "[eneros-root-task] HEARTBEAT-TIMEOUT: {} no progress {} rounds since probe (last_beat={}, probe={}) — kill+restart",
                    handle.name,
                    HEARTBEAT_MISS_LIMIT,
                    hb_last_beat[idx],
                    sent
                );
                supervisor.advance(now_ms());
                match supervisor.on_service_fault(idx) {
                    Some(crate::supervisor::Action::Restart { delay_ms }) => {
                        busy_wait_ms(delay_ms);
                        if let Err(e) = restart_service(&handles[idx]) {
                            ipc_fail_closed("heartbeat restart", e);
                        }
                        supervisor.on_restart_done(idx);
                        eneros_runtime::println!(
                            "[eneros-root-task] HEARTBEAT-KILL: {} restarted (hang isolation verified)",
                            handle.name
                        );
                    }
                    Some(crate::supervisor::Action::MarkFaulted) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] HEARTBEAT-KILL: {} FAULTED (threshold) — no auto restart",
                            handle.name
                        );
                    }
                    None => {}
                }
                // 重启翻篇：清除 pending（重启后空闲不心跳不误杀）。
                hb_probe_sent[idx] = -1;
                hb_last_beat[idx] = tick as i64;
            }
        }

        // 2. supervisor：轮询 fault ep（非阻塞）——检测服务故障并决策。
        // 23 G-I 总出口修复：`seL4_Poll` 无消息时返回的 MessageInfo 未定义
        // （QEMU 实测 0x2239c8——垃圾值，length>0 误判为有消息 → 每轮
        // `unknown fault sender=0`）。fault IPC 必有 badge（= 服务索引+1），
        // 以 `sender != 0` 为有消息判据。
        let mut sender = 0u64;
        // 44.3：svc-fault 探针可选——缺失时不 poll fault ntfn（产品装配）。
        if let Some(fault) = &fault {
            if let Ok(_mi) = ipc_poll(fault.ntfn, Some(&mut sender)) {
                let has_msg = sender != 0;
                if has_msg {
                    let name = |i: usize| handles[i].name.as_str();
                    // WP-37.3：fault 详情（seL4 VMFault IPC 消息布局，
                    // aarch64 constants.h：MR0=fault IP, MR1=addr,
                    // MR2=PrefetchFault(1=取指故障), MR3=FSR）——NX 证据：
                    // ExecStack 注入后 MR2=1 且 addr 落在栈区 → 可归因取指
                    // 故障（NX 生效）。读取失败仅影响证据行，不阻断决策。
                    let fault_ip = get_mr(0).unwrap_or(u64::MAX);
                    let fault_addr = get_mr(1).unwrap_or(u64::MAX);
                    let prefetch = get_mr(2).unwrap_or(u64::MAX);
                    let fsr = get_mr(3).unwrap_or(u64::MAX);
                    if let Some(idx) = sender
                        .checked_sub(1)
                        .filter(|&b| (b as usize) < handles.len())
                        .map(|b| b as usize)
                    {
                        eneros_runtime::println!(
                        "[eneros-root-task] FAULT-INFO: {} ip=0x{:x} addr=0x{:x} prefetch={} fsr=0x{:x}",
                        name(idx),
                        fault_ip,
                        fault_addr,
                        prefetch,
                        fsr
                    );
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
                                // 复验 svc-hello 仍响应（隔离成立；探针缺失
                                // 时跳过——纯产品装配无 hello 锚点）。
                                if let Some(hello) = &hello {
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

        // 3. 周期故障注入（每 50 轮）：svc-fault 探针存在且 Running 时才
        //    注入（Suspended/Faulted 不注入，防 seL4_Send 阻塞 root-task）。
        //    WP-37.3：注入 ExecStack（栈页取指）——NX 生效时产生
        //    instruction-fetch vm fault（可归因：栈地址），supervisor
        //    restart 证明隔离；旧 IllegalAccess（null 写）不证明 NX。
        fault_tick += 1;
        if let (Some(fault), Some(fault_idx)) = (&fault, fault_idx) {
            if fault_tick % 50 == 0
                && supervisor.state_of(fault_idx) == Some(crate::supervisor::ServiceState::Running)
            {
                eneros_runtime::println!(
                    "[eneros-root-task] IPC: send fault=ExecStack to svc-fault (cycle)"
                );
                let req = encode_frame(El0FrameType::Report, b"fault=ExecStack");
                if let Err(e) = send_frame(fault.ep, &req) {
                    ipc_fail_closed("send fault", e);
                }
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

/// 绝对单调毫秒（CNTPCT_EL0 直接换算——与 EL0 服务 `drv_svc::now_ms`
/// 同基准；40 收尾：root 的相对时钟（MonotonicClock 减 boot_ns）与
/// 服务端绝对时钟混用导致跨进程样本 Stale 误判，QEMU 实弹暴露——
/// 遥测样本时间戳必须与 evaluator 的 now 同基准）。
#[cfg(target_os = "none")]
fn abs_now_ms() -> u64 {
    eneros_hal::arm64::timer::clock().now_ns() / 1_000_000
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
/// `tcb_write_registers` 重置 PC/SP/X0..X4（与 configure_service 相同的
/// 8 寄存器布局）+ `tcb_resume` 即从 `_start` 重新执行（无需
/// revoke/retype 对象；bss 未清零为已知语义——服务重启保留静态状态，
/// 生产服务由 supervisor 重启策略保证幂等性）。重启后服务重新执行
/// init → 重新 Signal 就绪 ntfn（X2=READY_NTFN_CSPACE_SLOT；root 侧
/// readiness 重新判定）。心跳超时 kill 路径同用本函数（蓝图 40 收尾：
/// 无 fault 卡死 worker → suspend + 本函数重启）。
#[cfg(target_os = "none")]
fn restart_service(
    handle: &crate::spawn::SpawnedHandle,
) -> Result<(), eneros_sel4_sys::safe::Sel4Error> {
    use eneros_sel4_sys::safe::{tcb_resume, tcb_suspend, tcb_write_registers};

    // 寄存器布局由 spawn::initial_registers 单一持有（W2.1）——与 configure_service
    // 共享同一构造点，count=8 契约由该函数保证。X3/X4 经 handle 记录的
    // mask/心跳槽推导（与 spawn 同源）；svc-storage 的 X3/X4/X5 经
    // handle.storage_handover 重建（41 设备移交——MMIO/DMA 映射 restart
    // 期间保持，物理地址不变）。
    let regs = crate::spawn::initial_registers(
        handle.entry,
        handle.stack_top,
        handle.buffer_vaddr,
        crate::spawn::READY_NTFN_CSPACE_SLOT,
        handle.eipc_mask,
        crate::spawn::heartbeat_slot_of(&handle.name),
        handle.storage_handover,
        handle.net_handover,
    );
    // 心跳超时路径：目标可能仍在运行（死循环无 fault）——先 suspend 再
    // 重写寄存器（fault 路径 TCB 已被内核挂起，suspend 幂等）。
    tcb_suspend(handle.tcb)?;
    // count=9：与 spawn 一致（X5 供 svc-storage 设备移交，其他服务 X5=0）。
    tcb_write_registers(handle.tcb, 0, 9, &regs)?;
    tcb_resume(handle.tcb)
}

/// 蓝图 40 收尾：一轮控制链驱动（root 为采集链代理 + 编排触发器）。
///
/// 1. INGEST：向 svc-telemetry 注入 6 点样本（source=1，quality Good，
///    integrity Verified，seq 单调）——39 采集链设备句柄移交（CVR-39-02，
///    41 Entry 前置）完成前的 QEMU 装配路径；样本经 evaluator 全量校验；
/// 2. TICK：触发 svc-energy-agent 编排（telemetry → worker → dispatch →
///    EIPC → device-agent 全链在 EL0 服务间完成）。
///
/// 返回 (ingest 接受数, EA 状态码, dispatch 终态码)；传输错误 → Err
/// （fail-closed 打印，不猜成功）。
#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn storage_persist_probe(ota_ep: u64, epoch: &mut u64) -> Result<(), alloc::string::String> {
    use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
    use eneros_agent_sdk::ChunkedClient;
    let mut ipc = Sel4IpcClient::new("root-storage-probe", ota_ep, eneros_time::get_monotonic_ns)
        .map_err(|e| alloc::format!("ota client init: {:?}", e))?;
    // OTA1 0x06 PERSIST-TRIAL：remaining(4)‖version(16) → gen(8)。
    // 服务循环为分块 EIPC（ChunkedReceiver）——须用 ChunkedClient 发送。
    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'O', b'T', b'A', b'1', 0x06];
    req.extend_from_slice(&2u32.to_le_bytes()); // remaining
    let mut ver = [0u8; 16];
    ver[..5].copy_from_slice(b"1.2.0");
    req.extend_from_slice(&ver);
    let mut transfer = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp = transfer
        .transfer("svc-ota", &req)
        .map_err(|e| alloc::format!("persist transport: {e:?}"))?;
    // 41 修复：svc-ota 失败响应以 "ERR:" 前缀返回（如存储未授予
    // "ERR:store-unavailable"）。不检查前缀就把错误串前 8 字节解成 gen
    // 会打印假成功行（"trial persisted gen=<'ERR:stor' 的 u64>"）——
    // 41 第二轮 QEMU 日志实锤，fail-closed 必须前置拦截。
    if resp.starts_with(b"ERR:") {
        return Err(alloc::format!(
            "persist rejected: {}",
            core::str::from_utf8(&resp).unwrap_or("<bad-utf8>")
        ));
    }
    if resp.len() < 8 {
        return Err(alloc::format!("persist resp short: {}", resp.len()));
    }
    let gen = u64::from_le_bytes(resp[..8].try_into().unwrap());
    eneros_runtime::println!(
        "[eneros-root-task] STORAGE-PERSIST: trial persisted gen={} (svc-ota -> svc-storage -> data disk)",
        gen
    );
    // OTA1 0x07 LOAD-TRIAL：→ found(1)‖remaining(4)‖version(16)。
    let mut req2: alloc::vec::Vec<u8> = alloc::vec![b'O', b'T', b'A', b'1', 0x07];
    let mut transfer2 = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp2 = transfer2
        .transfer("svc-ota", &req2)
        .map_err(|e| alloc::format!("load transport: {e:?}"))?;
    if resp2.starts_with(b"ERR:") {
        return Err(alloc::format!(
            "load rejected: {}",
            core::str::from_utf8(&resp2).unwrap_or("<bad-utf8>")
        ));
    }
    if resp2.len() < 5 || resp2[0] != 1 {
        return Err(alloc::format!(
            "trial not found after persist (len={})",
            resp2.len()
        ));
    }
    let remaining = u32::from_le_bytes(resp2[1..5].try_into().unwrap());
    let version = core::str::from_utf8(&resp2[5..])
        .unwrap_or("<bad-utf8>")
        .trim_end_matches('\0');
    eneros_runtime::println!(
        "[eneros-root-task] STORAGE-PERSIST: trial reloaded remaining={} version={}",
        remaining,
        version
    );
    if remaining != 2 || version != "1.2.0" {
        return Err(alloc::format!(
            "trial mismatch: remaining={} version={}",
            remaining,
            version
        ));
    }
    Ok(())
}

/// 47（WP-47.3）：OTA 探针签章测试键种子（qemu-test 专用；release 由
/// KMS 注入面提供，见 xtask keys 门禁）。
#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
const OTA_PROBE_TEST_SEED: [u8; 32] = [0x41; 32];

#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn ota_download_probe(ota_ep: u64, epoch: &mut u64) -> Result<(), alloc::string::String> {
    use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
    use eneros_agent_sdk::ChunkedClient;
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair, Sm2PrivateKey};

    // 47（WP-47.3 密钥注入链）：OTA 探针签章**测试键**（种子 0x41——与
    // config-bundle 测试键一致；仅 qemu-test 装配使用）。release 装配的
    // 探针签章键由 KMS_DIR 注入面提供（xtask keys 门禁 fail-closed：
    // 生产无显式键不构建/不装配——见 keys::resolve_signing_key）。
    let sk = Sm2PrivateKey::from_bytes(&OTA_PROBE_TEST_SEED)
        .map_err(|e| alloc::format!("ota test key invalid: {e:?}"))?;
    let kp = Sm2KeyPair::from_private_key(&sk)
        .map_err(|e| alloc::format!("ota test pubkey derive: {e:?}"))?;
    let image: alloc::vec::Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
    let hash = sm3_hash(&image);
    let mut rng = CsRng::from_seed(&[0x41u8; 32]);
    let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng)
        .map_err(|e| alloc::format!("ota sign: {e:?}"))?;

    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'O', b'T', b'A', b'1', 0x02];
    let mut target = [0u8; 32];
    target[..7].copy_from_slice(b"0.123.0");
    req.extend_from_slice(&target);
    let mut sku = [0u8; 16];
    sku[..4].copy_from_slice(b"qemu");
    req.extend_from_slice(&sku);
    req.extend_from_slice(&1u32.to_le_bytes()); // secver
    req.extend_from_slice(&(image.len() as u64).to_le_bytes());
    req.extend_from_slice(&hash);
    req.extend_from_slice(&sig.to_bytes());
    req.extend_from_slice(&512u32.to_le_bytes()); // chunk_size
    req.extend_from_slice(&(image.len() as u32).to_le_bytes());
    req.extend_from_slice(&image);

    let mut ipc = Sel4IpcClient::new("root-ota-download", ota_ep, eneros_time::get_monotonic_ns)
        .map_err(|e| alloc::format!("ota client init: {:?}", e))?;
    let mut transfer = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp = transfer
        .transfer("svc-ota", &req)
        .map_err(|e| alloc::format!("ota download transport: {e:?}"))?;
    if resp.starts_with(b"ERR:") {
        return Err(alloc::format!(
            "ota download rejected: {}",
            core::str::from_utf8(&resp).unwrap_or("<bad-utf8>")
        ));
    }
    if resp.len() < 1 || resp[0] != 2 {
        return Err(alloc::format!("ota verify unexpected resp: {resp:?}"));
    }
    eneros_runtime::println!(
        "[eneros-root-task] OTA-DOWNLOAD: verified via svc-ota (real A/B write+readback)"
    );

    // 0x03 TRIAL
    let mut req3: alloc::vec::Vec<u8> = alloc::vec![b'O', b'T', b'A', b'1', 0x03, 1u8];
    let mut ver = [0u8; 32];
    ver[..7].copy_from_slice(b"0.123.0");
    req3.extend_from_slice(&ver);
    let mut transfer3 = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp3 = transfer3
        .transfer("svc-ota", &req3)
        .map_err(|e| alloc::format!("ota trial transport: {e:?}"))?;
    if resp3.starts_with(b"ERR:") || resp3.len() < 1 || resp3[0] != 3 {
        return Err(alloc::format!("ota trial unexpected resp: {resp3:?}"));
    }
    eneros_runtime::println!("[eneros-root-task] OTA-DOWNLOAD: trial started");

    // 0x04 COMMIT
    let req4: alloc::vec::Vec<u8> = alloc::vec![b'O', b'T', b'A', b'1', 0x04];
    let mut transfer4 = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp4 = transfer4
        .transfer("svc-ota", &req4)
        .map_err(|e| alloc::format!("ota commit transport: {e:?}"))?;
    if resp4.starts_with(b"ERR:") || resp4.len() < 1 || resp4[0] != 4 {
        return Err(alloc::format!("ota commit unexpected resp: {resp4:?}"));
    }
    eneros_runtime::println!("[eneros-root-task] OTA-DOWNLOAD: committed");
    Ok(())
}

/// 45.5 合成遥测注入——**仅 qemu-test 介质**（feature `qemu-test-synth`）：
/// 6 点合成样本（与 svc-telemetry 覆盖声明/绑定一致：电压/电流/频率/SOC/
/// 并网/告警——source=1 为其 allowed_sources）INGEST → accepted。
///
/// TEL_INGEST = 1（TEL_SNAPSHOT = 2——QEMU 实弹曾误用 2 收到
/// SNAPSHOT_REJECTED，协议码以 control_services::telemetry 为准）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
fn control_chain_synth_ingest(
    telemetry_ep: u64,
    seq: u64,
    epoch: &mut u64,
    now: u64,
) -> Result<u8, alloc::string::String> {
    use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
    use eneros_agent_sdk::ChunkedClient;
    use eneros_safety_validator::{IntegrityStatus, SampleQuality, TelemetrySample};

    let mut req: alloc::vec::Vec<u8> = alloc::vec![1u8];
    for (point, value) in [
        (1u32, 380.0f64),
        (2, 10.0),
        (3, 50.0),
        (4, 0.6),
        (5, 1.0),
        (6, 0.0),
    ] {
        let sample = TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms: now.saturating_sub(50),
            received_at_ms: now,
            quality: SampleQuality::Good,
            sequence: seq,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        };
        // 45.5：58B 编码收归 agent-sdk sample_wire（单一事实）。
        req.extend_from_slice(&eneros_agent_sdk::sample_wire::encode_sample(&sample));
    }
    // INGEST（分块：349B → 4 块）。
    let mut ipc = Sel4IpcClient::new("root-collect", telemetry_ep, eneros_time::get_monotonic_ns)
        .map_err(|e| alloc::format!("telemetry client init: {:?}", e))?;
    let mut transfer = ChunkedClient::new(&mut ipc, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let resp = transfer.transfer("svc-telemetry", &req).map_err(|e| {
        let stats = ipc.stats();
        alloc::format!(
            "ingest transport: {:?} (sent={} recvd={} err={})",
            e,
            stats.0,
            stats.1,
            stats.2
        )
    })?;
    // INGEST 响应：[0, accepted]。
    if resp.len() < 2 || resp[0] != 0 {
        return Err(alloc::format!(
            "ingest rejected: {:02x?}",
            &resp[..resp.len().min(8)]
        ));
    }
    Ok(resp[1])
}

/// 45.5 合成源降级：release/qemu-prod 装配**零合成遥测路径**——closure gate
/// 强制 `qemu-test-synth` 不在 release 闭包；本占位无注入（accepted=0）。
#[cfg(all(
    target_os = "none",
    feature = "sel4-eipc",
    not(feature = "qemu-test-synth")
))]
fn control_chain_synth_ingest(
    _telemetry_ep: u64,
    _seq: u64,
    _epoch: &mut u64,
    _now: u64,
) -> Result<u8, alloc::string::String> {
    Ok(0)
}

#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn control_chain_tick(
    telemetry_ep: u64,
    energy_ep: u64,
    seq: &mut u64,
    epoch: &mut u64,
) -> Result<(u8, u8, Option<u8>), alloc::string::String> {
    let now = abs_now_ms();
    *seq = seq.saturating_add(1);
    // 45.5（合成源降级）：遥测注入经 `qemu-test-synth` 门控——feature 默认
    // 关；qemu-prod/board-prod 等 release 装配走 `control_chain_synth_ingest`
    // 的零合成路径（accepted=0，无注入）。真实采集由协议服务
    // （先行者 svc-modbus-tcp）经 EIPC 上报。TICK 触发段保留（编排触发器）。
    let accepted = control_chain_synth_ingest(telemetry_ep, *seq, epoch, now)?;
    // 2. TICK（1B 请求；响应 = EA 状态 + receipt wire 83B）。
    use eneros_agent_sdk::ipc_sel4::Sel4IpcClient;
    use eneros_agent_sdk::ChunkedClient;
    let mut ipc2 = Sel4IpcClient::new(
        "root-orchestrator",
        energy_ep,
        eneros_time::get_monotonic_ns,
    )
    .map_err(|e| alloc::format!("energy client init: {:?}", e))?;
    let mut transfer2 = ChunkedClient::new(&mut ipc2, *epoch, 5_000);
    *epoch = epoch.saturating_add(1);
    let tick_resp = transfer2
        .transfer("svc-energy-agent", &[1u8 /* EA_TICK */])
        .map_err(|e| alloc::format!("tick transport: {:?}", e))?;
    if tick_resp.is_empty() {
        return Err("empty tick response".into());
    }
    let ea_status = tick_resp[0];
    // EA_OK 布局：EA_OK(1) + receipt wire（status1+state1+…）。
    let disp_state = if ea_status == 0 && tick_resp.len() >= 3 {
        Some(tick_resp[2])
    } else {
        None
    };
    Ok((accepted, ea_status, disp_state))
}

/// host/无 sel4-eipc 构建的占位（控制链驱动不可用——如实报错，不伪造）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn control_chain_tick(
    _telemetry_ep: u64,
    _energy_ep: u64,
    _seq: &mut u64,
    _epoch: &mut u64,
) -> Result<(u8, u8, Option<u8>), alloc::string::String> {
    Err(alloc::string::String::from(
        "control chain driver requires sel4-eipc feature",
    ))
}

/// host/无 sel4-eipc 构建的占位（存储探针不可用——如实报错，不伪造）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
/// storage persist probe 占位（同 control_chain_tick 门控）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn storage_persist_probe(_ota_ep: u64, _epoch: &mut u64) -> Result<(), alloc::string::String> {
    Err(alloc::string::String::from(
        "storage persist probe requires sel4-eipc feature",
    ))
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

    // ==================== WP-37.1 #8：readiness 判定 ====================

    /// 就绪信号已到 → Ready（首轮即判定）。
    #[test]
    fn test_check_readiness_signaled_ready() {
        assert_eq!(
            check_readiness(0, 5_000, 100, true),
            Some(ReadinessVerdict::Ready)
        );
        // 信号优先于 deadline（即使已超时）。
        assert_eq!(
            check_readiness(0, 5_000, 9_999, true),
            Some(ReadinessVerdict::Ready)
        );
    }

    /// deadline 内无信号 → None（继续收集，不提前降级）。
    #[test]
    fn test_check_readiness_pending_before_deadline() {
        assert_eq!(check_readiness(0, 5_000, 4_999, false), None);
        assert_eq!(check_readiness(1_000, 5_000, 5_999, false), None);
    }

    /// 超过 deadline 无信号 → Degraded（不虚报 Ready）。
    #[test]
    fn test_check_readiness_timeout_degrades() {
        assert_eq!(
            check_readiness(0, 5_000, 5_000, false),
            Some(ReadinessVerdict::Degraded)
        );
        assert_eq!(
            check_readiness(1_000, 5_000, 6_001, false),
            Some(ReadinessVerdict::Degraded)
        );
    }

    /// 异步收集全流程：先 None（未到期），信号后 Ready；无信号超时
    /// Degraded（drv 占位服务语义）。
    #[test]
    fn test_check_readiness_async_collection_flow() {
        // 参考服务：第二轮信号到达 → Ready。
        let mut v = check_readiness(0, 5_000, 500, false);
        assert_eq!(v, None);
        v = check_readiness(0, 5_000, 1_000, true);
        assert_eq!(v, Some(ReadinessVerdict::Ready));
        // 占位服务：永不信号 → 超时 Degraded。
        v = check_readiness(0, 5_000, 4_999, false);
        assert_eq!(v, None);
        v = check_readiness(0, 5_000, 5_000, false);
        assert_eq!(v, Some(ReadinessVerdict::Degraded));
    }
}
