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
}
