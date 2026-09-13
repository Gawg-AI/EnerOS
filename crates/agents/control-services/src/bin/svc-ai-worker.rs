//! svc-ai-worker — 蓝图 49 WP-49.2：target AI worker（EL0，远程 dual-brain）。
//!
//! 决策供给（ADR-0045 方案 B）：System2 = 站端 LLM 服务（**不受信 peer**，
//! 经 MQTT QoS1 交互；应答经 intent-contract::peer 全量校验 + remote_brain
//! 状态机配对/超时/降级）；System1 = 静态启发式（SOC 死区，fail-operational
//! 兜底）。宣称纪律：「AI 辅助决策（需配套站端服务）」——非设备端本地推理。
//!
//! 线路：
//! - svc-energy-agent → 本服务 `[AI_PROPOSE=1][trusted wire]`（既有链）；
//! - svc-mqtt → 本服务 `[AI_PEER_MSG=0x10][应答 JSON]`（ChunkedClient 推送）；
//! - 本服务 → svc-mqtt `[EXECUTE][AI_REQ_BEGIN/CONT/END]`（请求 JSON 分片
//!   drv_call 发布）；
//! - 本服务 → svc-config `[CFG1 GET_ENTRY "ai-peer"]`（状态机参数，全必配）。
//!
//! 决策留痕（蓝图 WP-49.2 #5）：每次 AI 参与决策的 prompt 摘要/应答摘要/
//! 路径标记入 DecisionAudit 哈希链（截断 ≤48/≤64 字符，不落全文）+ 串口
//! REMOTE-BRAIN 证据行。
//!
//! SPEC-SERIES-2026-08-28/B WP-B.3：tick 编排（SOC 提取→阈值注入→propose→
//! 留痕→observe）、LEARN_* wire 组装、审计留痕钩子全部下沉
//! `eneros_control_services::ai_handler`（host 可测单源）；本 bin 只剩
//! 能力探测（授予/配置双条件）+ 分支选择 + 判定行。
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_drv_svc::allocator::HeapAllocator =
    eneros_drv_svc::allocator::HeapAllocator::new();

// ============================================================================
// 服务入口（能力探测：授予/配置双条件 → remote / heuristic-only 分支）
// ============================================================================

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    use eneros_control_services::ai_handler::target::{
        build_learner, fetch_ai_peer_config, run_heuristic_only, run_remote,
    };
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // WP-54.4：按名解析授予槽（agent-sdk 单一事实源）；SPEC-B：掩码位
    // 判定收编 eipc_granted_slot。任一未授予 → 对应能力 fail-closed
    // （config 缺 → 启发式兜底；mqtt 缺 → 无远程路径，纯启发式——诚实降级）。
    let slot_of = |target: &'static str| -> Option<u64> {
        eneros_agent_sdk::eipc_granted_slot("svc-ai-worker", eipc_mask, target)
    };
    // 蓝图 67 WP-67.2.3：config 传输面 = ChunkedEndpoint（epoch 单点
    // 纪律；标签与构造路径同源——bind_cap 内部同 Sel4IpcClient::new）。
    let mut config_client = slot_of("svc-config").and_then(|slot| {
        eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap("aiworker-config", slot)
    });
    // 蓝图 61 v0.314.35：目录版本装载（候选计划 resource_catalog_version
    // 与 dispatch PlannerView 同代——[08] §4.8 planner 输入只查 Registry）。
    // 蓝图 70.10：目录仓库**保留**（结构化计划输入源 = 同代目录派生；
    // 此前只取版本号即丢弃）。
    let fetched_catalog = config_client
        .as_mut()
        .and_then(eneros_control_services::catalog_gateway::fetch_catalog);
    let catalog_repo = fetched_catalog.as_ref().map(|c| c.repo.clone());
    let catalog_version = fetched_catalog
        .as_ref()
        .map(|c| {
            use eneros_semantic_catalog::SemanticCatalog;
            let v = c.repo.version_set().contract_version;
            let d = c.repo.digest();
            eneros_runtime::println!(
                "[svc-ai-worker] CATALOG-ACTIVE gen={} contract={} digest={:02x}{:02x}{:02x}{:02x}",
                c.generation,
                v,
                d[0],
                d[1],
                d[2],
                d[3]
            );
            v
        })
        .unwrap_or_else(|| {
            eneros_runtime::println!(
                "[svc-ai-worker] CATALOG-MISSING —— catalog_version=0（fail-closed：Registry 对照不了即拒批）"
            );
            0
        });
    let cfg = config_client.and_then(|mut c| match fetch_ai_peer_config(&mut c) {
        Ok(c) => Some(c),
        Err(reason) => {
            eneros_runtime::println!("[svc-ai-worker] AI-PEER-CFG FAIL {reason}");
            None
        }
    });
    let mqtt_client = slot_of("svc-mqtt").and_then(|slot| {
        eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
            "aiworker-mqtt",
            slot,
            eneros_drv_svc::now_ns,
        )
        .ok()
    });

    match (cfg, mqtt_client) {
        (Some(c), Some(mqtt)) => {
            eneros_runtime::println!(
                "[svc-ai-worker] AI-PEER-CFG loaded timeout={} unhealthy={} probe={} max_kw={:.1} ttl={} learn={}",
                c.timeout_ms,
                c.unhealthy_after,
                c.probe_every,
                c.max_power_kw,
                c.ttl_secs,
                c.learn_enabled
            );
            eneros_runtime::println!(
                "[svc-ai-worker] backend=remote-brain (System2=station peer via MQTT; System1=heuristic fallback; ADR-0045 plan B)"
            );
            let brain_cfg = eneros_control_services::remote_brain::RemoteBrainConfig {
                timeout_ms: c.timeout_ms,
                unhealthy_after: c.unhealthy_after,
                probe_every: c.probe_every,
                max_power_kw: c.max_power_kw,
                ttl_secs: c.ttl_secs,
                // 68.5 WP-68.5.3：D3 降级 lease 武装参数（ai-peer 配置注入
                // 链供给；peer 失联 → LEASE-ARMED → System1 兜底持签降级）。
                lease_key: c.degrade_lease_key,
                lease_ttl_secs: c.degrade_lease_ttl_secs,
            };
            // SPEC-E WP-E.3：学习器装配（闸+授予双条件；None ⇒ 出厂行为）。
            let learn = build_learner(eipc_mask, c.learn_enabled);
            // 结构化计划输入源：目录 + 站点侧计划参数（plan 参数缺省 ⇒
            // None ⇒ v1 迁移轨，行为与 70.10 前一致）。
            let plan_source = match (catalog_repo, c.plan.clone()) {
                (Some(repo), Some(params)) => Some((repo, params)),
                _ => None,
            };
            run_remote(
                ep,
                ready_ntfn,
                heartbeat_slot,
                brain_cfg,
                mqtt,
                learn,
                catalog_version,
                plan_source,
            )
        }
        (cfg, mqtt_client) => {
            eneros_runtime::println!(
                "[svc-ai-worker] backend=heuristic (cfg={} mqtt={} —— 远程路径 fail-closed)",
                cfg.is_some(),
                mqtt_client.is_some()
            );
            // config 不可得 ⇒ learn_enabled 键不可得 ⇒ 学习闸关（fail-closed）。
            let learn = cfg.and_then(|c| build_learner(eipc_mask, c.learn_enabled));
            run_heuristic_only(ep, ready_ntfn, heartbeat_slot, learn, catalog_version)
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // 死亡必须可见（svc-device-agent 词形先例；svc-telemetry 同款订正）。
    eneros_runtime::println!("[svc-ai-worker] PANIC: {info}");
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
