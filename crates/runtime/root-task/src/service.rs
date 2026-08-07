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

extern crate alloc;

/// K2.2/K2.3 消息契约：MR 布局。
///
/// seL4 IPC 载荷走 MR（message register）+ IPC buffer：
/// - MR0 = 帧总字节数（u64 小端）；
/// - MR1.. = 帧字节按 8 字节小端打包（末字不足补 0）；
/// - MessageInfo：label = 帧类型（[`El0FrameType`] 数值），length = MR words 数。
///
/// root-task 与服务两侧共用本布局（单一事实源）。
pub const MR_FRAME_LEN_SLOT: u64 = 0;
/// 单帧最大字节数（ipc-wire 默认 payload 1024 + 帧头 10 + crc 4，留裕量；
/// 须小于 IPC buffer 容量）。
pub const MAX_FRAME_BYTES: usize = 2048;

/// 帧编码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMrError {
    /// 帧超长（> [`MAX_FRAME_BYTES`]）。
    FrameTooLong,
    /// MR 列表为空（无长度槽）。
    NoLengthSlot,
    /// 长度与 MR 载荷不一致。
    BadLength,
}

/// 帧字节 → MR words（[`MR_FRAME_LEN_SLOT`] 布局）。
pub fn frame_into_mrs(frame: &[u8]) -> Result<alloc::vec::Vec<u64>, FrameMrError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(FrameMrError::FrameTooLong);
    }
    let mut mrs = alloc::vec![frame.len() as u64];
    let mut i = 0;
    while i < frame.len() {
        let mut word = 0u64;
        for j in 0..8 {
            if i + j < frame.len() {
                word |= u64::from(frame[i + j]) << (8 * j);
            }
        }
        mrs.push(word);
        i += 8;
    }
    Ok(mrs)
}

/// MR words → 帧字节（[`MR_FRAME_LEN_SLOT`] 布局）。
pub fn frame_from_mrs(mrs: &[u64]) -> Result<alloc::vec::Vec<u8>, FrameMrError> {
    let len = *mrs.first().ok_or(FrameMrError::NoLengthSlot)? as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameMrError::FrameTooLong);
    }
    let words = 1 + len.div_ceil(8);
    if mrs.len() < words {
        return Err(FrameMrError::BadLength);
    }
    let mut out = alloc::vec![0u8; len];
    for i in 0..len {
        let word = mrs[1 + i / 8];
        out[i] = (word >> (8 * (i % 8))) as u8;
    }
    Ok(out)
}

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

/// K2.2：root-task IPC 主循环（21-A §2.K2.2）。
///
/// 每轮迭代：
/// 1. 向 svc-hello 的 endpoint 发 hello 请求帧（ipc-wire::el0 编码，
///    MR 布局见 [`frame_into_mrs`]），`ipc_call` 同步往返收响应；
/// 2. 阻塞 `ipc_recv` 任意来源消息（seL4 Endpoint 同步语义，非轮询）。
///
/// 任何 IPC 步骤失败 → [`ipc_fail_closed`] 挂起（红线 4）。
/// 服务侧响应与故障隔离由 K2.3 的 `svc_ipc_loop` 提供（QEMU 验证）。
#[cfg(target_os = "none")]
pub fn run_ipc_loop(handles: &[crate::spawn::SpawnedHandle]) -> ! {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};
    use eneros_sel4_sys::safe::{get_mr, ipc_call, ipc_recv, message_info_new, set_mr};
    use eneros_sel4_sys::types::message_info_length;

    // 服务表：按名寻址 svc-hello 的 ep cap。
    let hello_ep = match handles.iter().find(|h| h.name == "svc-hello").map(|h| h.ep) {
        Some(ep) => ep,
        None => {
            eneros_runtime::println!(
                "[eneros-root-task] svc-hello not spawned — 挂起（fail-closed）"
            );
            loop {
                core::hint::spin_loop();
            }
        }
    };

    loop {
        // 1. hello 请求 → svc-hello（帧 → MR）。
        let req = encode_frame(El0FrameType::Report, b"hello");
        let mrs = match frame_into_mrs(&req) {
            Ok(m) => m,
            Err(e) => ipc_fail_closed("encode hello", e),
        };
        for (i, w) in mrs.iter().enumerate() {
            if let Err(e) = set_mr(i as u64, *w) {
                ipc_fail_closed("set_mr", e);
            }
        }
        let mi = match message_info_new(El0FrameType::Report as u64, 0, 0, mrs.len() as u64) {
            Ok(m) => m,
            Err(e) => ipc_fail_closed("message_info_new", e),
        };
        eneros_runtime::println!("[eneros-root-task] IPC: send hello-req to svc-hello");
        let reply = match ipc_call(hello_ep, mi) {
            Ok(r) => r,
            Err(e) => ipc_fail_closed("ipc_call hello", e),
        };
        let words = message_info_length(reply);
        let mut mr_buf = alloc::vec::Vec::with_capacity(words as usize);
        for i in 0..words {
            match get_mr(i) {
                Ok(w) => mr_buf.push(w),
                Err(e) => ipc_fail_closed("get_mr", e),
            }
        }
        let resp = match frame_from_mrs(&mr_buf) {
            Ok(f) => f,
            Err(e) => ipc_fail_closed("decode reply", e),
        };
        eneros_runtime::println!(
            "[eneros-root-task] IPC: recv {} bytes from svc-hello",
            resp.len()
        );

        // 2. 阻塞接收任意服务消息（Ready/Report 帧；K2.3 起按帧分发）。
        let mut sender = 0u64;
        match ipc_recv(0, Some(&mut sender)) {
            Ok(_info) => {}
            Err(e) => ipc_fail_closed("ipc_recv", e),
        }
    }
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
