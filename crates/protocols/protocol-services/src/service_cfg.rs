//! 协议服务配置拉取（46 WP-46.3 配置驱动装配）。
//!
//! svc-* 经 svc-config `GET_ENTRY("protocol-<proto>")`（EIPC，
//! `Sel4IpcClient` + `ChunkedClient`）取协议配置 canonical KV → `decode_protocol`
//! 还原 `ProtocolConfig` 喂插件 `start`——**配置即 peer 唯一来源**（消灭源码
//! 硬编码，46.3 DoD）。EIPC 未授予 / 解码失败 → `None` → 服务保持能力壳
//! （fail-closed，不猜默认 peer）。

use eneros_agent_sdk::config_client;
use eneros_config_bundle::points::PointRecord;
use eneros_protocol_abstract::ProtocolConfig;

/// 协议配置最大总长（与服务端 bundle 条目上限对齐；防御性截断检查）。
const MAX_PAYLOAD: usize = 2048;

/// 经 EIPC 从 svc-config 拉取协议配置。
///
/// `config_id`：`protocol-modbus-tcp` 等（`configs/protocol-*.toml`）。
/// `grant_slot`：该服务在 `EIPC_EP_BASE_SLOT` 基础上的授予槽偏移（首个
/// svc-config 授予即 +0；svc-modbus-tcp 因遥测先行需观察自身表序）。
/// wire 知识（请求构造/分块/4B 前缀）单一来源 = agent-sdk
/// `config_client`（SPEC-B WP-B.1，历史 10 处手抄收编）。
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
    let payload = config_client::get_entry(&mut client, config_id, 0, 5_000).ok()?;
    let body = match config_client::decode_response_body(&payload) {
        Ok(b) => b,
        Err(eneros_agent_sdk::config_client::ConfigError::TooShort) => {
            eneros_runtime::println!("[{client_name}] config response too short");
            return None;
        }
        Err(eneros_agent_sdk::config_client::ConfigError::LengthPrefix) => {
            eneros_runtime::println!("[{client_name}] config length prefix malformed");
            return None;
        }
        Err(eneros_agent_sdk::config_client::ConfigError::Transport) => return None,
    };
    if body.len() > MAX_PAYLOAD {
        eneros_runtime::println!("[{client_name}] config payload exceeds {MAX_PAYLOAD}");
        return None;
    }
    match eneros_config_bundle::protocol::decode_protocol(body) {
        Ok(cfg) => {
            eneros_runtime::println!(
                "[{client_name}] PROTOCOL-CONFIG loaded {} keys (id={})",
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
/// `decode_points`），按 `keep` 过滤协议后返回注册表；插件按自身协议地址
/// 过滤（服务层不预过滤——`keep = |_| true` 即全量，iec104 用）。不可得 →
/// 空注册表（插件如实降级：空映射 = 不连设备，不伪造协议 I/O）。
///
/// `client_name` 仅诊断；`grant_slot` = 该服务 svc-config 授予槽
/// （`fetch_protocol_config` 同槽——同端点可处理 config 与 points 两类
/// 请求）。wire 知识单一来源 = agent-sdk `config_client`（SPEC-B WP-B.1）。
pub fn fetch_points_registry<F>(
    client_name: &str,
    grant_slot: u64,
    keep: F,
) -> eneros_protocol_abstract::VecPointRegistry
where
    F: Fn(&PointRecord) -> bool,
{
    use eneros_protocol_abstract::VecPointRegistry;
    let Ok(mut client) = eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        client_name,
        grant_slot,
        eneros_drv_svc::now_ns,
    ) else {
        eneros_runtime::println!("[{client_name}] config EIPC NOT granted -> points unavailable");
        return VecPointRegistry::new(alloc::vec::Vec::new());
    };
    let payload = match config_client::get_entry(&mut client, "points", 0, 5_000) {
        Ok(p) => p,
        Err(e) => {
            eneros_runtime::println!("[{client_name}] ConfigGet(points) EIPC fail: {e:?}");
            return VecPointRegistry::new(alloc::vec::Vec::new());
        }
    };
    let body = match config_client::decode_response_body(&payload) {
        Ok(b) => b,
        Err(eneros_agent_sdk::config_client::ConfigError::TooShort) => {
            eneros_runtime::println!("[{client_name}] config response too short");
            return VecPointRegistry::new(alloc::vec::Vec::new());
        }
        Err(eneros_agent_sdk::config_client::ConfigError::LengthPrefix) => {
            eneros_runtime::println!("[{client_name}] config length prefix malformed");
            return VecPointRegistry::new(alloc::vec::Vec::new());
        }
        Err(eneros_agent_sdk::config_client::ConfigError::Transport) => {
            return VecPointRegistry::new(alloc::vec::Vec::new())
        }
    };
    let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
    match eneros_config_bundle::points::decode_points(body, &cfg_id) {
        Ok(table) => {
            let mappings: alloc::vec::Vec<_> = table
                .points
                .iter()
                .filter(|p| keep(p))
                .filter_map(|p| p.to_mapping())
                .collect();
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
