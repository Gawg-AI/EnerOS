//! 47（WP-47.2）：控制写目标绑定——经 svc-config 取 45 卷点表（canonical
//! `points`）→ 选择 Control 点（设备/点）。**配置绑定**：写目标不再
//! 硬编码于产品装配（`PROD_DEVICE_ID`/`PROD_POINT_ID` 已退休）。
//!
//! 请求/响应 wire（`CFG1` ConfigGet + 4B LE 前缀）单一来源 = agent-sdk
//! `config_client`（SPEC-B WP-B.1 收编）；S1 WP-S1.2 起调用侧管道单源 =
//! `config_gateway::config_get`（借切片解码 + `ConfigStep` 失败分类）；
//! 本模块只保留 fail-closed 判定与诊断措辞（R2 逐字节保持）。任一步
//! 失败 → None（装配方不得自造写目标）。

#[cfg(feature = "sel4")]
use eneros_agent_sdk::config_gateway::{config_get, ConfigStep};

/// 获取控制点绑定（设备/点）。
///
/// `config` = svc-config EIPC 客户端（授予槽由装配方传入）；返回
/// `(device_id, point_id)`。
#[cfg(feature = "sel4")]
pub fn fetch_control_binding(
    config: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Option<(u32, u32)> {
    let got = config_get(config, "points", |body| {
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        match eneros_config_bundle::points::decode_points(body, &cfg_id) {
            Ok(table) => {
                let binding = table.control_binding();
                match binding {
                    Some((device, point)) => {
                        eneros_runtime::println!(
                            "[control-bindings] POINTS-LOADED {} points — CONTROL-BINDING device={device} point={point}",
                            table.len()
                        );
                    }
                    None => {
                        eneros_runtime::println!(
                            "[control-bindings] points 表无 Control 点 —— fail-closed（绑定缺失）"
                        );
                    }
                }
                Ok(binding.map(|(d, p)| (d as u32, p)))
            }
            Err(e) => {
                eneros_runtime::println!("[control-bindings] points decode FAIL: {e:?}");
                Err(ConfigStep::DecodeFailed)
            }
        }
    });
    // wire 失败呈现（词形 = 今日 `ConfigError` Debug 逐字：Transport/
    // TooShort/LengthPrefix——分类单点后按步映射，R2）。
    match got {
        Ok(binding) => binding,
        Err(ConfigStep::Unavailable) => {
            eneros_runtime::println!("[control-bindings] ConfigGet(points) EIPC fail: Transport");
            None
        }
        Err(ConfigStep::TooShort) => {
            eneros_runtime::println!("[control-bindings] config response malformed: TooShort");
            None
        }
        Err(ConfigStep::BadPrefix) => {
            eneros_runtime::println!("[control-bindings] config response malformed: LengthPrefix");
            None
        }
        Err(ConfigStep::DecodeFailed) => None,
    }
}

/// SPEC-SERIES-2026-08-27/A WP-A.1：获取控制链信封 HMAC-SM3 密钥
/// （ConfigGet("security") → canonical payload → 严格解码）。
///
/// 缺节/长度≠32/全零 ⇒ None（fail-closed：调用方打 `CHAIN-KEY-MISSING`
/// 判定行并拒载）。请求/响应 wire 与管道与 [`fetch_control_binding`]
/// 同源（agent-sdk `config_client` + `config_gateway`）。
#[cfg(feature = "sel4")]
pub fn fetch_control_chain_key(
    config: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Option<[u8; 32]> {
    let got = config_get(config, "security", |body| {
        match eneros_config_bundle::control_chain_key::ControlChainKeyConfig::decode(body) {
            Ok(k) => Ok(k.control_chain_key),
            Err(e) => {
                eneros_runtime::println!("[control-bindings] security decode FAIL: {e:?}");
                Err(ConfigStep::DecodeFailed)
            }
        }
    });
    match got {
        Ok(key) => Some(key),
        Err(ConfigStep::Unavailable) => {
            eneros_runtime::println!("[control-bindings] ConfigGet(security) EIPC fail: Transport");
            None
        }
        Err(ConfigStep::TooShort) => {
            eneros_runtime::println!("[control-bindings] security response malformed: TooShort");
            None
        }
        Err(ConfigStep::BadPrefix) => {
            eneros_runtime::println!(
                "[control-bindings] security response malformed: LengthPrefix"
            );
            None
        }
        Err(ConfigStep::DecodeFailed) => None,
    }
}
