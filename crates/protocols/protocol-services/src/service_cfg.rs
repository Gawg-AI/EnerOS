//! 协议服务配置拉取（46 WP-46.3 配置驱动装配）。
//!
//! svc-* 经 svc-config `GET_ENTRY("protocol-<proto>")`（EIPC，
//! `Sel4IpcClient` + `ChunkedClient`）取协议配置 canonical KV → `decode_protocol`
//! 还原 `ProtocolConfig` 喂插件 `start`——**配置即 peer 唯一来源**（消灭源码
//! 硬编码，46.3 DoD）。EIPC 未授予 / 解码失败 → `None` → 服务保持能力壳
//! （fail-closed，不猜默认 peer）。

use alloc::vec::Vec;

use eneros_protocol_abstract::ProtocolConfig;

/// 协议配置最大总长（与服务端 bundle 条目上限对齐；防御性截断检查）。
const MAX_PAYLOAD: usize = 2048;

/// 经 EIPC 从 svc-config 拉取协议配置。
///
/// `config_id`：`protocol-modbus-tcp` 等（`configs/protocol-*.toml`）。
/// `grant_slot`：该服务在 `EIPC_EP_BASE_SLOT` 基础上的授予槽偏移（首个
/// svc-config 授予即 +0；svc-modbus-tcp 因遥测先行需观察自身表序）。
pub fn fetch_protocol_config(
    client_name: &str,
    grant_slot: u64,
    config_id: &str,
) -> Option<ProtocolConfig> {
    let mut client = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        client_name,
        grant_slot,
        eneros_drv_svc::now_ns,
    )
    .ok()?;
    let mut req: Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(config_id.as_bytes());
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(&mut client, 0, 5_000);
    let payload = transfer.transfer("svc-config", &req).ok()?;
    // svc-config GET_ENTRY 响应 = 4B LE 长度 + canonical payload（与
    // svc-config 先例一致）；剥前缀后再解码。
    if payload.len() < 4 {
        eneros_runtime::println!("[{}] config response too short", client_name);
        return None;
    }
    let plen = u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) as usize;
    if 4 + plen > payload.len() || plen > MAX_PAYLOAD {
        eneros_runtime::println!("[{}] config length prefix malformed", client_name);
        return None;
    }
    match eneros_config_bundle::protocol::decode_protocol(&payload[4..4 + plen]) {
        Ok(cfg) => {
            eneros_runtime::println!(
                "[{}] PROTOCOL-CONFIG loaded {} keys (id={})",
                client_name,
                cfg.entries.len(),
                config_id
            );
            Some(cfg)
        }
        Err(e) => {
            eneros_runtime::println!("[{}] protocol config decode FAIL: {:?}", client_name, e);
            None
        }
    }
}

/// 46.5（点表驱动收口）：经 svc-config EIPC 取点表（ConfigGet "points" →
/// `decode_points`），返回全量注册表；插件按自身协议地址过滤（服务层不
/// 预过滤——valued 于插件侧自滤）。不可得 → 空注册表（插件如实降级：空
/// 映射 = 不连设备，不伪造协议 I/O）。
///
/// `client_name` 仅诊断；`grant_slot` = 该服务 svc-config 授予槽
/// （`fetch_protocol_config` 同槽——同端点可处理 config 与 points 两类
/// 请求）。
pub fn fetch_points_registry(
    client_name: &str,
    grant_slot: u64,
) -> eneros_protocol_abstract::VecPointRegistry {
    use eneros_protocol_abstract::VecPointRegistry;
    let Ok(mut client) = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        client_name,
        grant_slot,
        eneros_drv_svc::now_ns,
    ) else {
        eneros_runtime::println!("[{client_name}] config EIPC NOT granted -> points unavailable");
        return VecPointRegistry::new(alloc::vec::Vec::new());
    };
    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(b"points");
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(&mut client, 0, 5_000);
    let payload = match transfer.transfer("svc-config", &req) {
        Ok(p) => p,
        Err(e) => {
            eneros_runtime::println!("[{client_name}] ConfigGet(points) EIPC fail: {e:?}");
            return VecPointRegistry::new(alloc::vec::Vec::new());
        }
    };
    // svc-config GET_ENTRY 响应 = 4B LE 长度 + canonical payload（与
    // fetch_protocol_config 同构）。
    let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
    let body = if payload.len() >= 4 {
        let plen = u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) as usize;
        if 4 + plen <= payload.len() {
            &payload[4..4 + plen]
        } else {
            eneros_runtime::println!("[{client_name}] config length prefix malformed");
            return VecPointRegistry::new(alloc::vec::Vec::new());
        }
    } else {
        eneros_runtime::println!("[{client_name}] config response too short");
        return VecPointRegistry::new(alloc::vec::Vec::new());
    };
    match eneros_config_bundle::points::decode_points(body, &cfg_id) {
        Ok(table) => {
            let mappings: alloc::vec::Vec<_> =
                table.points.iter().filter_map(|p| p.to_mapping()).collect();
            eneros_runtime::println!(
                "[{client_name}] POINTS-LOADED {} points (registry {})",
                mappings.len(),
                table.len()
            );
            VecPointRegistry::new(mappings)
        }
        Err(e) => {
            eneros_runtime::println!("[{client_name}] points decode FAIL: {e:?}");
            VecPointRegistry::new(alloc::vec::Vec::new())
        }
    }
}
