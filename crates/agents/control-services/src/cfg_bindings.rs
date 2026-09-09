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

#[cfg(any(test, feature = "sel4"))]
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
///
/// 69.3 WP-69.3.4：`ChunkedEndpoint<Sel4IpcClient>` 具体绑定 → `<C:
/// RawIpc>` 泛型（config_gateway blanket impl 同卷使能）——函数体内零
/// sel4 专有调用（执行卡 7.4 逐行核）；cfg 门 `feature="sel4"` →
/// `any(test, feature="sel4")`（catalog_gateway::fetch_catalog 同口径）：
/// 生产 sel4 路径字节不变，host 测试经内存 RawIpc 假件首次可达（c1–c6；
/// 本函数产出 = 控制链信封 HMAC-SM3 密钥，信任锚——改造前后 c4 合法
/// 用例返回值逐字节快照一致，执行卡在案）。
#[cfg(any(test, feature = "sel4"))]
pub fn fetch_control_chain_key<C: eneros_agent_sdk::chunked::RawIpc>(
    config: &mut eneros_agent_sdk::endpoint::ChunkedEndpoint<C>,
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

// 测试模块只随 host 测试构建编译（`#[cfg(test)]`）——此前误用
// `any(test, feature="sel4")` 会把测试体编进 target 镜像（aarch64 无
// std/alloc::vec 导入面，E0425/E0433 实弹咬出，2026-09-07）；被测函数
// 的可达门 `any(test, feature="sel4")` 在函数上，两者口径分离。
#[cfg(test)]
mod tests {
    use eneros_agent_sdk::chunked::{
        ack_bytes, is_pull_request, ChunkHeader, ChunkedEvent, ChunkedReceiver, PagedServerV2,
        CHUNK_MAGIC_FINAL, CHUNK_MAGIC_HEADER,
    };

    use super::*;

    /// 内存 svc-config 假件（RawIpc；chunked 分块状态机 = agent-sdk
    /// config_client FakeConfigServer 同款模式，c1–c6 测试面）。`security`
    /// 载荷可选（None = 缺节语义路径）。
    struct FakeConfigServer {
        rx: ChunkedReceiver,
        pending: Option<Vec<u8>>,
        pages: PagedServerV2,
        security: Option<Vec<u8>>,
    }

    impl FakeConfigServer {
        fn with_security(payload: Vec<u8>) -> Self {
            Self {
                rx: ChunkedReceiver::new(),
                pending: None,
                pages: PagedServerV2::new(),
                security: Some(payload),
            }
        }
    }

    impl eneros_agent_sdk::chunked::RawIpc for FakeConfigServer {
        fn request(
            &mut self,
            _t: &str,
            payload: &[u8],
            _to: u64,
        ) -> Result<Vec<u8>, eneros_agent_sdk::AgentError> {
            if &payload[..4] == CHUNK_MAGIC_HEADER {
                let h = ChunkHeader::decode(payload).unwrap();
                if let ChunkedEvent::Header { epoch, .. } = self.rx.on_message(&h.encode(), true) {
                    self.pages.set_txn(epoch);
                }
                return Ok(ack_bytes().to_vec());
            }
            if &payload[..4] == CHUNK_MAGIC_FINAL {
                match self.pending.take() {
                    Some(full) => {
                        assert_eq!(&full[..4], b"CFG1", "请求帧 magic");
                        // 节载荷 = security（4B 前缀在下方统一加——单前缀
                        // 词形；请求内容不回显——假件是 config 服务端不是
                        // 回环）。
                        let body = self.security.clone().unwrap_or_default();
                        let mut resp = Vec::new();
                        resp.extend_from_slice(&(body.len() as u32).to_le_bytes());
                        resp.extend_from_slice(&body);
                        self.pages.on_request(payload, resp).ok_or_else(|| {
                            eneros_agent_sdk::AgentError::IpcError(
                                heapless::String::try_from("paged-reject").unwrap_or_default(),
                            )
                        })
                    }
                    None => Ok(b"ERR:NoTransfer".to_vec()),
                }
            } else if is_pull_request(payload) {
                self.pages.on_request(payload, Vec::new()).ok_or_else(|| {
                    eneros_agent_sdk::AgentError::IpcError(
                        heapless::String::try_from("paged-reject").unwrap_or_default(),
                    )
                })
            } else {
                Err(eneros_agent_sdk::AgentError::IpcError(
                    heapless::String::try_from("unexpected").unwrap_or_default(),
                ))
            }
        }

        fn send(&mut self, _t: &str, payload: &[u8]) -> Result<(), eneros_agent_sdk::AgentError> {
            if let ChunkedEvent::Complete(full) = self.rx.on_message(payload, false) {
                self.pending = Some(full);
            }
            Ok(())
        }
    }

    /// security canonical payload（FIELD_CONTROL_CHAIN_KEY 帧）。
    fn security_payload(key: &[u8]) -> Vec<u8> {
        eneros_config_bundle::payload::encode_payload([(
            eneros_config_bundle::schema::security::FIELD_CONTROL_CHAIN_KEY,
            key,
        )])
    }

    fn endpoint(
        security: Option<Vec<u8>>,
    ) -> eneros_agent_sdk::endpoint::ChunkedEndpoint<FakeConfigServer> {
        eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(FakeConfigServer::with_security(
            security.unwrap_or_default(),
        ))
    }

    /// c4（信任锚快照）：合法 32B ⇒ Some(k) 且逐字节等于配置值。
    #[test]
    fn c4_chain_key_valid_yields_bytes() {
        let key: [u8; 32] = core::array::from_fn(|i| (i as u8) ^ 0xA5);
        let mut ep = endpoint(Some(security_payload(&key)));
        assert_eq!(fetch_control_chain_key(&mut ep), Some(key));
    }

    /// c1：缺 security 节（传输层无应答）⇒ None（fail-closed）。
    #[test]
    fn c1_chain_key_missing_section_yields_none() {
        let mut ep = endpoint(None);
        assert_eq!(fetch_control_chain_key(&mut ep), None);
    }

    /// c2：密钥长度 ≠ 32 ⇒ None（decode Err(InvalidValue len)）。
    #[test]
    fn c2_chain_key_wrong_length_yields_none() {
        let mut ep = endpoint(Some(security_payload(&[0xA5; 31])));
        assert_eq!(fetch_control_chain_key(&mut ep), None);
    }

    /// c3：全零密钥 ⇒ None（decode Err(ZeroSecret)——不伪造信任锚）。
    #[test]
    fn c3_chain_key_all_zero_yields_none() {
        let mut ep = endpoint(Some(security_payload(&[0u8; 32])));
        assert_eq!(fetch_control_chain_key(&mut ep), None);
    }

    /// c5：canonical payload 解码失败（截断帧）⇒ None。
    #[test]
    fn c5_chain_key_decode_error_yields_none() {
        let mut ep = endpoint(Some(vec![0x01, 0xFF, 0x00]));
        assert_eq!(fetch_control_chain_key(&mut ep), None);
    }

    /// c6（字面锁）：本文件只允许**一个** `fetch_control_chain_key` 定义
    /// ——防 sel4 专版分叉复发（泛型化前的原病灶形态）。
    #[test]
    fn c6_generic_and_sel4_paths_share_body() {
        let src = include_str!("cfg_bindings.rs");
        let needle = concat!("fn ", "fetch_control_chain_key");
        assert_eq!(
            src.matches(needle).count(),
            1,
            "fetch_control_chain_key 出现多定义（sel4 专版分叉复发）"
        );
        assert!(
            src.contains("C: eneros_agent_sdk::chunked::RawIpc"),
            "chain key 取数须保持泛型接口（host 可测）"
        );
    }
}
