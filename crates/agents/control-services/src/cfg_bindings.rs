//! 47（WP-47.2）：控制写目标绑定——经 svc-config 取 45 卷点表（canonical
//! `points`）→ 选择 Control 点（设备/点）。**配置绑定**：写目标不再
//! 硬编码于产品装配（`PROD_DEVICE_ID`/`PROD_POINT_ID` 已退休）。
//!
//! 请求/响应与 svc-modbus-tcp 先例同wire：`CFG1` ConfigGet("points") →
//! 4B LE 长度前缀 + canonical payload → `decode_points` →
//! [`PointsTable::control_binding`]。任一步失败 → None（fail-closed：
//! 装配方不得自造写目标）。

use alloc::string::String;

/// 获取控制点绑定（设备/点）。
///
/// `config` = svc-config EIPC 客户端（授予槽由装配方传入）；返回
/// `(device_id, point_id)`。
#[cfg(feature = "sel4")]
pub fn fetch_control_binding(
    config: &mut eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
) -> Option<(u32, u32)> {
    let mut req: alloc::vec::Vec<u8> = alloc::vec![b'C', b'F', b'G', b'1', 0x01];
    req.extend_from_slice(b"points");
    let mut transfer = eneros_agent_sdk::ChunkedClient::new(config, 0, 5_000);
    let payload = match transfer.transfer("svc-config", &req) {
        Ok(p) => p,
        Err(e) => {
            eneros_runtime::println!("[control-bindings] ConfigGet(points) EIPC fail: {e:?}");
            return None;
        }
    };
    // svc-config GET_ENTRY 响应 = 4B LE 长度 + canonical payload。
    if payload.len() < 4 {
        eneros_runtime::println!("[control-bindings] config response too short");
        return None;
    }
    let plen = u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) as usize;
    if 4 + plen > payload.len() {
        eneros_runtime::println!("[control-bindings] config length prefix malformed");
        return None;
    }
    let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
    match eneros_config_bundle::points::decode_points(&payload[4..4 + plen], &cfg_id) {
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
            binding.map(|(d, p)| (d as u32, p))
        }
        Err(e) => {
            eneros_runtime::println!("[control-bindings] points decode FAIL: {e:?}");
            None
        }
    }
}

/// 目标端点名（观测/日志）。
pub fn config_endpoint_name() -> String {
    String::from("svc-config")
}
