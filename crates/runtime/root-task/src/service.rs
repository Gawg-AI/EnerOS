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
    use eneros_sel4_sys::safe::{get_mr, ipc_call, ipc_recv, ipc_send, message_info_new, set_mr};

    // 服务表：按名寻址。
    let by_name = |name: &str| {
        handles
            .iter()
            .find(|h| h.name == name)
            .ok_or_else(|| alloc::format!("{name} not spawned"))
    };
    let hello = match by_name("svc-hello") {
        Ok(h) => *h,
        Err(e) => ipc_fail_closed("lookup svc-hello", e),
    };
    let fault = match by_name("svc-fault") {
        Ok(h) => *h,
        Err(e) => ipc_fail_closed("lookup svc-fault", e),
    };

    // 故障注入只执行一次（svc-fault 触发真实异常后挂起，不再接收消息；
    // 重复 send 会永久阻塞——seL4_Send 是同步原语）。
    let mut fault_injected = false;

    loop {
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

        // 2. 故障注入（仅首轮）：aarch64 无除零异常（SDIV 结果 0），
        // 用非法内存访问（null 写 → Data Abort）触发真实故障。
        if !fault_injected {
            fault_injected = true;
            eneros_runtime::println!("[eneros-root-task] IPC: send fault to svc-fault");
            let req = encode_frame(El0FrameType::Report, b"fault=IllegalAccess");
            if let Err(e) = send_frame(fault.ep, &req) {
                ipc_fail_closed("send fault", e);
            }

            // 3. 等 svc-fault 故障通知（服务异常 → 内核挂起 TCB → 通知到达）。
            let mut sender = 0u64;
            match ipc_recv(fault.ntfn, Some(&mut sender)) {
                Ok(_) => {
                    eneros_runtime::println!("[eneros-root-task] svc-fault suspended (isolated)");
                }
                Err(e) => ipc_fail_closed("recv fault ntfn", e),
            }

            // 4. 复验 svc-hello 仍响应（隔离成立）。
            let resp = match hello_roundtrip(hello.ep) {
                Ok(f) => f,
                Err(e) => ipc_fail_closed("hello recheck", e),
            };
            let payload = match eneros_ipc_wire::el0::decode_frame(&resp) {
                Ok(f) => f.payload,
                Err(e) => ipc_fail_closed("decode recheck resp", e),
            };
            eneros_runtime::println!(
                "[eneros-root-task] IPC: svc-hello still responsive ({})",
                core::str::from_utf8(payload).unwrap_or("<non-utf8>")
            );
        }

        // 循环回到 1：hello 心跳持续（svc-hello tick 递增可见）。
    }
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
    use super::*;
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};

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
