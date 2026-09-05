//! 47（WP-47.2）：控制写目标绑定——经 svc-config 取 45 卷点表（canonical
//! `points`）→ 选择 Control 点（设备/点）。**配置绑定**：写目标不再
//! 硬编码于产品装配（`PROD_DEVICE_ID`/`PROD_POINT_ID` 已退休）。
//!
//! 蓝图 61 v0.314.33（[08] §2.5 迁移）：**「首个 Control 点」选择已删除**——
//! 绑定点由 SemanticCatalog 显式给出（点语义 role=control 且带控制合同，
//! 恰好一个；歧义/缺失 → None fail-closed），设备短 ID 仍为协议映射
//! （points.toml 点 → device）。退出 gate：生产无手写 first control
//! binding（[08] §2.7）。
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
/// `catalog` = 装配入口一次拉取的目录（62 v0.315.0 WP-62.1.4 S7：本函数
/// 不再自取 catalog——一次装配四视图同代）；`config` = svc-config EIPC
/// 客户端（授予槽由装配方传入，仍用于 `points` 一次往返）；返回
/// `(device_id, point_id)`。
///
/// v0.314.33 语义：目录显式绑定点（[`crate::catalog_gateway::
/// control_contract_point`]）+ 点表解析设备短 ID；目录缺失/歧义/点不在
/// 表 → None（fail-closed）。
#[cfg(feature = "sel4")]
pub fn fetch_control_binding<T: eneros_agent_sdk::config_gateway::ConfigEntrySource>(
    catalog: &crate::catalog_gateway::LoadedCatalog,
    config: &mut T,
) -> Option<(u32, u32)> {
    use crate::catalog_gateway;
    // 1. 目录 → 显式绑定点（role=control 且带合同，恰好一个；歧义拒绝）。
    let Some(point) = catalog_gateway::control_contract_point(&catalog.repo) else {
        eneros_runtime::println!(
            "[control-bindings] 目录无唯一控制绑定点 —— fail-closed（绑定缺失）"
        );
        return None;
    };
    // 2. 点表 → 设备短 ID（协议映射；点必须真实存在——不存在 = 装配
    //    错误，不允许目录凭空创建写目标）。
    let got = config_get(config, "points", |body| {
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        match eneros_config_bundle::points::decode_points(body, &cfg_id) {
            Ok(table) => Ok(table.find(point).map(|rec| (rec.device_id as u32, point))),
            Err(e) => {
                eneros_runtime::println!("[control-bindings] points decode FAIL: {e:?}");
                Err(ConfigStep::DecodeFailed)
            }
        }
    });
    // wire 失败呈现（词形 = 今日 `ConfigError` Debug 逐字：Transport/
    // TooShort/LengthPrefix——分类单点后按步映射，R2）。
    match got {
        Ok(Some((device, point))) => {
            eneros_runtime::println!(
                "[control-bindings] CATALOG-BINDING point={point} device={device} (explicit, gen={})",
                catalog.generation
            );
            Some((device, point))
        }
        Ok(None) => {
            eneros_runtime::println!(
                "[control-bindings] 目录绑定点 {point} 不在点表 —— fail-closed（绑定缺失）"
            );
            None
        }
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
    config: &mut eneros_agent_sdk::endpoint::ChunkedEndpoint<
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    >,
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
