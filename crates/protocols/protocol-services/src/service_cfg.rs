//! 协议服务配置拉取（46 WP-46.3 配置驱动装配；S1 起管道单源）。
//!
//! svc-* 经 svc-config `GET_ENTRY("protocol-<proto>")`（ConfigGet）取协议
//! 配置 canonical KV → `decode_protocol` 还原 `ProtocolConfig` 喂插件
//! `start`——**配置即 peer 唯一来源**（46.3 DoD）。
//!
//! S1（SPEC-SERIES-2026-08-29）收编：取数 seam（[`ConfigEntrySource`]）
//! 注入——target 由 `protocol_ladder::Sel4ServiceIo` 按名 `eipc_resolve`
//! 取槽，调用侧管道（前缀剥离 → 借切片解码 → 失败分类）单源 =
//! agent-sdk `config_gateway::config_get`（WP-S1.2）。本模块只保留
//! **呈现**：PROTOCOL-CONFIG / POINTS-LOADED 证据行与 too-short/
//! bad-prefix 词形（单源 = `protocol_ladder::line_*`，F-35 两抄收编），
//! 逐字节保持。取数失败 → `Err`/空注册表（fail-closed，不猜默认 peer）。

use eneros_agent_sdk::config_gateway::{config_get, ConfigEntrySource, ConfigStep};
use eneros_config_bundle::points::PointRecord;
use eneros_protocol_abstract::ProtocolConfig;

/// 协议配置最大总长（与服务端 bundle 条目上限对齐；防御性截断检查）。
const MAX_PAYLOAD: usize = 2048;

/// 取协议配置并解码（`decode_protocol`；呈现词形逐字节保持）。
///
/// `config_id`：`protocol-modbus-tcp` 等（`configs/protocol-*.toml`）。
/// wire 知识（请求构造/分块/4B 前缀）单一来源 = agent-sdk
/// `config_client`；调用侧管道单一来源 = `config_gateway::config_get`
/// （SPEC-B WP-B.1 + S1 WP-S1.2）。
pub fn fetch_protocol_config(
    io: &mut dyn ConfigEntrySource,
    client_name: &str,
    config_id: &str,
) -> Result<ProtocolConfig, ConfigStep> {
    config_get(io, config_id, |body| {
        if body.len() > MAX_PAYLOAD {
            eneros_runtime::println!("[{client_name}] config payload exceeds {MAX_PAYLOAD}");
            return Err(ConfigStep::DecodeFailed);
        }
        match eneros_config_bundle::protocol::decode_protocol(body) {
            Ok(cfg) => {
                eneros_runtime::println!(
                    "[{client_name}] PROTOCOL-CONFIG loaded {} keys (id={})",
                    cfg.entries.len(),
                    config_id
                );
                Ok(cfg)
            }
            Err(e) => {
                eneros_runtime::println!("[{client_name}] protocol config decode FAIL: {:?}", e);
                Err(ConfigStep::DecodeFailed)
            }
        }
    })
    .map_err(|step| {
        // wire 失败呈现（词形单源 = protocol_ladder::line_*；今日
        // service_cfg 两抄收编，R2 逐字节）。Unavailable/DecodeFailed
        // 今日不打印额外行。
        match step {
            ConfigStep::TooShort => {
                eneros_runtime::println!(
                    "{}",
                    crate::protocol_ladder::line_config_too_short(client_name)
                );
            }
            ConfigStep::BadPrefix => {
                eneros_runtime::println!(
                    "{}",
                    crate::protocol_ladder::line_config_bad_prefix(client_name)
                );
            }
            _ => {}
        }
        step
    })
}

/// 46.5（点表驱动收口）：取点表（ConfigGet "points" → `decode_points`），
/// 按 `keep` 过滤协议后返回注册表；插件按自身协议地址过滤（服务层不预
/// 过滤——`keep = |_| true` 即全量，iec104 用）。不可得 → 空注册表
/// （插件如实降级：空映射 = 不连设备，不伪造协议 I/O）。
///
/// `client_name` 仅诊断（与 `fetch_protocol_config` 同前缀——同端点可
/// 处理 config 与 points 两类请求）。wire/管道单源同上。
///
/// 点表取数结果（蓝图 61 v0.314.27：注册表 + 来源注册表 + 映射世代）。
pub struct PointsFetch {
    /// 协议点注册表（`keep` 过滤后；identity 已注入 generation）。
    pub registry: eneros_protocol_abstract::VecPointRegistry,
    /// 来源注册表（`[[source]]` 编译产物；旧 bundle = 空）。
    pub sources: alloc::vec::Vec<eneros_config_bundle::points::SourceRecord>,
    /// 原始点记录（`keep` 过滤后；蓝图 61 v0.314.30：调度面需要
    /// period_ms/deadband——映射不含调度字段）。
    pub records: alloc::vec::Vec<eneros_config_bundle::points::PointRecord>,
    /// 映射世代（active bundle generation；不可得 → 0 = 未定代，观测
    /// 不满足控制资格——fail-closed）。
    pub generation: u64,
    /// 解码点数（诊断）。
    pub total_points: usize,
}

/// `client_name` 仅诊断（与 `fetch_protocol_config` 同前缀——同端点可
/// 处理 config 与 points 两类请求）。wire/管道单源同上。
///
/// 返回 [`PointsFetch`]（蓝图 61：注册表 + 来源表 + 映射世代一次取数）。
pub fn fetch_points_registry<F>(
    io: &mut dyn ConfigEntrySource,
    client_name: &str,
    keep: F,
) -> eneros_protocol_abstract::VecPointRegistry
where
    F: Fn(&PointRecord) -> bool,
{
    fetch_points(io, client_name, keep).registry
}

/// 蓝图 61 v0.314.27：点表 + 来源注册表 + 映射世代一次取数。
///
/// generation 经 `GET_GENERATION`（服务端既有动词）；不可得 → 0（fail-
/// closed：世代未知的映射不满足控制资格，事件只可存档）。失败呈现词形
/// 与 [`fetch_points_registry`] 逐字节一致（历史 caller 行为不变）。
///
/// 69.3 WP-69.3.3（M3 #10 边界加固）：`pub` → `pub(crate)`——全仓零跨
/// crate 消费者（同 crate ladder run 两处调用不受影响），降级后 bin 在
/// 结构上不可达（E0603），主取数面由字符串锁升级为编译期边界；孪生
/// [`fetch_points_registry`] 因 svc-modbus-rtu 合法消费保持 pub（执行卡
/// 7.0 M3 表 #10）。
pub(crate) fn fetch_points<F>(
    io: &mut dyn ConfigEntrySource,
    client_name: &str,
    keep: F,
) -> PointsFetch
where
    F: Fn(&PointRecord) -> bool,
{
    use eneros_protocol_abstract::VecPointRegistry;
    let empty = |generation: u64| PointsFetch {
        registry: VecPointRegistry::new(alloc::vec::Vec::new()),
        sources: alloc::vec::Vec::new(),
        records: alloc::vec::Vec::new(),
        generation,
        total_points: 0,
    };
    let generation = io.config_generation().unwrap_or(0);
    if generation == 0 {
        eneros_runtime::println!("[{client_name}] OBS-GEN unknown (generation unavailable)");
    }
    match config_get(io, "points", |body| {
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        match eneros_config_bundle::points::decode_points(body, &cfg_id) {
            Ok(mut table) => {
                // 映射世代注入（apply 期同语义：generation = bundle 生效代）。
                for rec in table.points.iter_mut() {
                    rec.identity.mapping_generation = generation;
                }
                let kept: alloc::vec::Vec<eneros_config_bundle::points::PointRecord> =
                    table.points.iter().filter(|p| keep(p)).cloned().collect();
                let mappings: alloc::vec::Vec<_> =
                    kept.iter().filter_map(|p| p.to_mapping()).collect();
                eneros_runtime::println!(
                    "[{client_name}] POINTS-LOADED {} points (registry {})",
                    mappings.len(),
                    table.points.len()
                );
                Ok(PointsFetch {
                    registry: VecPointRegistry::new(mappings),
                    sources: table.sources,
                    records: kept,
                    generation,
                    total_points: table.points.len(),
                })
            }
            Err(e) => {
                eneros_runtime::println!("[{client_name}] points decode FAIL: {e:?}");
                Err(ConfigStep::DecodeFailed)
            }
        }
    }) {
        Ok(fetch) => fetch,
        Err(ConfigStep::TooShort) => {
            eneros_runtime::println!(
                "{}",
                crate::protocol_ladder::line_config_too_short(client_name)
            );
            empty(generation)
        }
        Err(ConfigStep::BadPrefix) => {
            eneros_runtime::println!(
                "{}",
                crate::protocol_ladder::line_config_bad_prefix(client_name)
            );
            empty(generation)
        }
        Err(ConfigStep::Unavailable) => {
            // S1 evidence M-3 登记：今日 client-new-fail 打印本行、
            // get_entry-fail 打印 `ConfigGet(points) EIPC fail: {e:?}`；
            // 取数 seam 收敛为 Option 后两态同呈本行（仅 svc-config
            // 已授予但请求超时的降级路径与旧词形不同）。
            eneros_runtime::println!(
                "[{client_name}] config EIPC NOT granted -> points unavailable"
            );
            empty(generation)
        }
        Err(ConfigStep::DecodeFailed) => empty(generation),
    }
}
