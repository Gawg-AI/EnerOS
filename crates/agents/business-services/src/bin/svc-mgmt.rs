//! svc-mgmt — 严格管理 RPC EL0（蓝图 41 WP-41.11）。
//! EIPC 协议：magic "MGT1" ‖ session(principal 16B‖roles 4B‖tenant 4B‖channel 16B‖expiry 8B) ‖
//! request_id(8) ‖ action(≤16) ‖ arg(≤128)。
//! 真实动作经注入 executor；未接线动作 → Failed（不空成功）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
use alloc::string::ToString;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
extern crate eneros_business_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_business_services::HeapAllocator =
    eneros_business_services::HeapAllocator::new();

/// 蓝图60 WP-60.2：OTA 三动作在 mgmt 侧的降级词形——「服务未绑定」语义
/// 保持（fail-closed 不空成功），补 EIPC 授予边缺失原因，与 13 个
/// `action-not-wired` 裸词形区分层级（svc-mgmt→svc-ota 授予边未建立；
/// 升格接通属独立侦察项，蓝图60 §5 默认不做）。宿主测试面经
/// `cfg(any(target_os = "none", test))` 双侧可用。
#[cfg(any(target_os = "none", test))]
fn ota_channel_unbound_detail() -> &'static str {
    "ota-channel-unbound:mgmt→ota-grant-missing"
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
    // 70.5（WP-70.5.6）：x5 预留（spawn 侧恒 0）；x6 = 墙钟 epoch 秒
    //（SPEC-B X6 链——0 = 时间缺失 → v2 token 过期判据 fail-closed）。
    _reserved_x5: u64,
    wall_epoch_secs: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 41（WP-41.11）：ConfigGet 经 EIPC 查 svc-config（授予表按客户端
    // 独立计数：svc-mgmt 首个 grant = 槽 EIPC_EP_BASE_SLOT+0、mask bit 0。
    // SPEC-B：硬编码掩码位（`& 1`）收编 eipc_granted_slot（按名解析）。
    // 蓝图 67 WP-67.2.3：config 传输面 = ChunkedEndpoint（epoch 单点
    // 纪律——原 self.epoch 手递增 ×2 面删除）。
    let config_ep = eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-config")
        .and_then(|slot| {
            eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap("mgmt-config", slot)
        });
    if config_ep.is_some() {
        eneros_runtime::println!("[svc-mgmt] config EIPC bound (svc-config)");
    } else {
        eneros_runtime::println!("[svc-mgmt] config EIPC NOT granted -> ConfigGet unavailable");
    }
    // 70.5：unix 偏移 = 墙钟(ms) - 单调(ms)（启动期一次捕获；墙钟 0 =
    // 时间缺失 → v2 面拒绝——过期判据不可信即 fail-closed）。
    let unix_offset_ms = if wall_epoch_secs != 0 {
        Some(
            wall_epoch_secs
                .saturating_mul(1000)
                .saturating_sub(eneros_business_services::now_ms()),
        )
    } else {
        None
    };
    eneros_runtime::println!(
        "[svc-mgmt] management RPC up (session-authenticated; real actions only)"
    );
    if eneros_business_services::STATION_PUBKEY_SEC1.is_empty() {
        eneros_runtime::println!(
            "[svc-mgmt] MGT station key absent —— legacy principal accepted (dev-only mode, WP-70.5.6)"
        );
    } else {
        eneros_runtime::println!(
            "[svc-mgmt] MGT station key deployed —— signed principal v2 enforced"
        );
    }
    // SPEC-C（WP-C.2）：history-query 取数通道——svc-mgmt 第 2 授予边
    // （SPEC-B：硬编码掩码位 `& 2` 收编 eipc_granted_slot 按名解析）；
    // 未授予 → 执行器回 "telemetry-not-bound"（fail-closed）。
    // 蓝图 67 WP-67.2.3：取数端点启动期一次装配持有（跨请求 epoch 单调
    // 延续——原"每请求重建客户端恒 0"面删除；bind 失败词形区分保留）。
    let telemetry_slot =
        eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-telemetry");
    let history_ep = telemetry_slot.and_then(|slot| {
        eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap("mgmt-history", slot)
    });
    if telemetry_slot.is_some() {
        eneros_runtime::println!("[svc-mgmt] history EIPC bound (svc-telemetry)");
    } else {
        eneros_runtime::println!(
            "[svc-mgmt] history EIPC NOT granted -> history-query unavailable"
        );
    }
    // 70.6（WP-70.6.1）：操作账本持久连接 + 恢复（DataRegion::MgmtOps；
    // 表序第 3 条授予 = 槽 base+2、bit 2）。存储缺席 ⇒ 副作用动作背压
    // 拒绝（只读诊断不受限——fail-closed 不伪造幂等）。
    let mut ops_io = eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "svc-storage")
        .and_then(|slot| eneros_business_services::block_io::eipc_block_io(slot, 0, 5_000));
    // 70.6（WP-70.6.3）：root 管理端点（MGT2；受控重启/维护/实例查询；
    // 表序第 4 条 = 槽 base+3、bit 3）。
    let root = eneros_agent_sdk::eipc_granted_slot("svc-mgmt", eipc_mask, "root-mgmt")
        .and_then(eneros_agent_sdk::root_mgmt::RootMgmtClient::bind);
    if root.is_some() {
        eneros_runtime::println!("[svc-mgmt] root-mgmt EIPC bound (restart/maintenance)");
    } else {
        eneros_runtime::println!(
            "[svc-mgmt] root-mgmt NOT granted -> restart/maintenance unavailable"
        );
    }
    // 操作账本恢复：RegionStateStore 装载 → 在途 Running 诚实转 Unknown
    //（进程已换——冒称在跑 = Accepted 永不终结路径；Unknown 附对账号）。
    const MGMT_OPS_OFFSET: u64 = eneros_ota_ab::ab_partition::DataRegion::MgmtOps.base_byte();
    const MGMT_OPS_LEN: u64 = eneros_ota_ab::ab_partition::MGMT_OPS_REGION_BYTES;
    let mut restored = eneros_business_services::mgmt_ops::OperationLedger::new();
    let mut ops_store_ok = false;
    if let Some(io) = ops_io.as_mut() {
        use eneros_durable_state::{Decode, DurableStateStore, PersistIo};
        let mut store = eneros_durable_state::RegionStateStore::<
            eneros_business_services::mgmt_ops::MgmtOpsState,
            _,
        >::new(io, MGMT_OPS_OFFSET, MGMT_OPS_LEN);
        match store.open().and_then(|_| store.load()) {
            Ok(Some(versioned)) => {
                restored = versioned.state.ledger;
                ops_store_ok = true;
                let n = restored.records().len();
                eneros_runtime::println!("[svc-mgmt] MGMT-OPS ledger restored n={n}");
            }
            Ok(None) => {
                ops_store_ok = true;
                eneros_runtime::println!("[svc-mgmt] MGMT-OPS ledger fresh (empty region)");
            }
            Err(e) => {
                ops_store_ok = true; // 区段在位但内容损坏 → 全新账本（诚实词形）
                eneros_runtime::println!(
                    "[svc-mgmt] MGMT-OPS ledger load failed ({e:?}) — fresh ledger (old records unrecoverable)"
                );
            }
        }
    } else {
        eneros_runtime::println!(
            "[svc-mgmt] MGMT-OPS storage NOT granted — side-effect ops will be rejected (ops-store-unavailable)"
        );
    }
    restored.mark_unknown_inflight(eneros_business_services::now_ms());
    let mut handler = MgmtHandler {
        unix_offset_ms,
        rpc: eneros_business_services::mgmt_rpc::ManagementRpc::new(TargetExecutor {
            config_ep,
            telemetry_slot,
            history_ep,
            requests_seen: 0,
            bundle_seq: 0,
            restart_requests: 0,
            ops_io,
            ops_store_ok,
            ops_region_base: MGMT_OPS_OFFSET,
            ops_region_len: MGMT_OPS_LEN,
            root,
            snapshot: None,
            snapshot_seq: 0,
        }),
    };
    // 恢复账本注入（ManagementRpc 拥有状态；装配侧注入持久化前态）。
    handler.rpc.ops = restored;
    eneros_control_services::control_service_main(
        ep,
        "mgmt",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct TargetExecutor {
    // 蓝图 67 WP-67.2.3：config/history 传输面 = ChunkedEndpoint（epoch
    // 单点递增，跨请求单调延续；原 epoch 字段 + 手递增 ×2 删除）。
    config_ep: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// SPEC-C：svc-telemetry 授予槽（bind 失败/未授予两态词形区分——
    /// history_ep 为 None 且本槽在场 = 建端失败词形）。
    telemetry_slot: Option<u64>,
    /// SPEC-C：TELR 取数端点（启动期一次装配）。
    history_ep: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// 50（WP-50.2）：请求计数 / 诊断包序号 / 重启请求数（真实观测）。
    requests_seen: u64,
    bundle_seq: u64,
    restart_requests: u64,
    /// 70.6（WP-70.6.1）：操作账本持久化连接与坐标（DataRegion::MgmtOps）。
    ops_io: Option<
        eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    ops_store_ok: bool,
    ops_region_base: u64,
    ops_region_len: u64,
    /// 70.6（WP-70.6.3）：root 管理客户端（MGT2）。
    root: Option<eneros_agent_sdk::root_mgmt::RootMgmtClient>,
    /// 70.6（WP-70.6.2）：支持包快照分页缓存（仅最新快照；旧 seq 显式
    /// 过期）。pages 各 ≤512B。
    snapshot: Option<(u32, alloc::vec::Vec<alloc::vec::Vec<u8>>)>,
    snapshot_seq: u32,
}

/// 支持包单页大小（响应 = 详情 + 载荷经 chunked 分页；512B 页 × ≤8 页）。
#[cfg(target_os = "none")]
const BUNDLE_PAGE: usize = 512;
/// 快照总预算（4KiB——远低于服务堆 512KiB；分节各自有界）。
#[cfg(target_os = "none")]
const BUNDLE_BUDGET: usize = 8 * BUNDLE_PAGE;
/// 重启在途对账超时（毫秒，单调时基）。
#[cfg(target_os = "none")]
const RESTART_TIMEOUT_MS: u64 = 120_000;

#[cfg(target_os = "none")]
impl TargetExecutor {
    /// 账本终态写回持久化（RPC 层账本变更后由 svc-mgmt 侧调用——经
    /// rpc.ops 克隆落盘；此处不持有账本引用，避免双源）。
    fn persist_ops(&mut self, ops: &eneros_business_services::mgmt_ops::OperationLedger) {
        use eneros_durable_state::{DurableStateStore, Encode, PersistIo};
        let Some(io) = self.ops_io.as_mut() else {
            return;
        };
        let state = eneros_business_services::mgmt_ops::MgmtOpsState {
            ledger: ops.clone(),
        };
        let Ok(bytes) = state.encode() else {
            return;
        };
        if bytes.len() as u64 > self.ops_region_len {
            eneros_runtime::println!("[svc-mgmt] MGMT-OPS persist skipped (size overflow)");
            return;
        }
        let mut store = eneros_durable_state::RegionStateStore::<
            eneros_business_services::mgmt_ops::MgmtOpsState,
            _,
        >::new(io, self.ops_region_base, self.ops_region_len);
        let persisted = store.open().and_then(|_| {
            let expected = store.load()?.map(|v| v.generation);
            store.compare_and_swap(expected, &state)
        });
        match persisted {
            Ok(gen) => {
                eneros_runtime::println!(
                    "[svc-mgmt] MGMT-OPS persisted gen={} n={}",
                    gen.0,
                    state.ledger.records().len()
                );
            }
            Err(e) => {
                eneros_runtime::println!("[svc-mgmt] MGMT-OPS persist failed ({e:?})");
            }
        }
    }

    /// MGT2 ack 往返（None = 端点缺席/传输失败——词形由调用方定）。
    fn root_call(&mut self, req: &[u8]) -> Option<alloc::vec::Vec<u8>> {
        self.root.as_mut()?.call(req)
    }

    /// 当前配置代次（config 端点 GET_GENERATION；不可得 = 0 如实）。
    fn config_generation(&mut self) -> u64 {
        use eneros_agent_sdk::config_client;
        let Some(ep) = self.config_ep.as_mut() else {
            return 0;
        };
        config_client::get_generation(ep).unwrap_or(0)
    }

    /// 支持包快照构建（WP-70.6.2：可分节有界收集 + 缺项显式化）。
    fn build_snapshot(&mut self, now_ms: u64) -> alloc::vec::Vec<u8> {
        use alloc::format;
        use alloc::string::String;
        let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(1024);
        let mut section = |name: &str, body: &str, out: &mut alloc::vec::Vec<u8>| {
            let d = eneros_crypto::sm3_hash(body.as_bytes());
            let line = format!(
                "{name}|digest={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}|{body}\n",
                d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            );
            out.extend_from_slice(line.as_bytes());
        };
        // ① 启动身份（workspace 版本 + 管理面 epoch 观测）。
        section(
            "identity",
            &format!("product_version={}", env!("CARGO_PKG_VERSION")),
            &mut out,
        );
        // ② 服务状态（本服务真实计数 + root 维护态投影）。
        let maint = self
            .root_call(&eneros_agent_sdk::root_mgmt::encode_maint_query())
            .and_then(|r| eneros_agent_sdk::root_mgmt::parse_maint_query_resp(&r).ok())
            .map(|(active, tick)| format!("maintenance={} entered_tick={tick}", active))
            .unwrap_or_else(|| String::from("missing:root-unbound"));
        section(
            "status",
            &format!("requests_seen={}\n{maint}", self.requests_seen),
            &mut out,
        );
        // ③ 配置摘要（digest+generation；不可得 = 缺项词形）。CFG1
        // GET_DIGEST(0x02) raw 往返（config_client 无 helper）。
        let cfg = match self.config_ep.as_mut() {
            Some(ep) => {
                let mut req = alloc::vec::Vec::new();
                req.extend_from_slice(eneros_business_services::config_service::PROTO_MAGIC);
                req.push(eneros_business_services::config_service::CMD_GET_DIGEST);
                let digest_info = ep.call("svc-config", &req).ok().and_then(|r| {
                    let n = u32::from_le_bytes(r[..4].try_into().ok()?) as usize;
                    if r.len() < 4 + n + 8 {
                        return None;
                    }
                    Some((
                        r[4..4 + n].to_vec(),
                        u64::from_le_bytes(r[4 + n..4 + n + 8].try_into().ok()?),
                    ))
                });
                match digest_info {
                    Some((digest, g)) if digest.len() == 32 => {
                        let hex =
                            |b: &[u8]| -> String { b.iter().map(|x| format!("{x:02x}")).collect() };
                        format!(
                            "generation={g} digest={} source=svc-config",
                            hex(&digest[..16])
                        )
                    }
                    _ => String::from("missing:config-unreachable"),
                }
            }
            None => String::from("missing:config-unbound"),
        };
        section("config", &cfg, &mut out);
        // ④ 日志面（logging 配置摘要；运行日志未经支持包分页面——显式缺项）。
        use eneros_agent_sdk::config_client;
        let log = match self.config_ep.as_mut() {
            Some(ep) => match config_client::get_entry(ep, "logging") {
                Ok(raw) => format!(
                    "log-config-bytes={} runtime-logs=missing:support-bundle-page-only",
                    raw.len()
                ),
                Err(_) => String::from("missing:log-config-unreachable"),
            },
            None => String::from("missing:config-unbound"),
        };
        section("logging", &log, &mut out);
        // ⑤ 持久恢复状态（操作账本存储连通性 + 快照时刻）。
        section(
            "persistence",
            &format!(
                "ops_store={} at_ms={now_ms}",
                if self.ops_store_ok {
                    "ok"
                } else {
                    "missing:storage-unbound"
                }
            ),
            &mut out,
        );
        out
    }
}

#[cfg(target_os = "none")]
impl eneros_business_services::mgmt_rpc::ActionExecutor for TargetExecutor {
    fn current_config_generation(&mut self) -> u64 {
        self.config_generation()
    }

    fn observe_operation(
        &mut self,
        record: &eneros_business_services::mgmt_ops::OperationRecord,
        now_ms: u64,
    ) -> Option<(
        eneros_business_services::mgmt_ops::OpStatus,
        alloc::string::String,
    )> {
        use alloc::format;

        use eneros_business_services::mgmt_ops::OpStatus;
        // 受控重启在途对账：detail 形如 "restart <target> base_gen=<g>"。
        if record.action != "agent-restart" {
            return None;
        }
        let rest = record.detail.strip_prefix("restart ")?;
        let (target, base_text) = rest.split_once(" base_gen=")?;
        let base_gen: u64 = base_text.parse().ok()?;
        let resp = self.root_call(&eneros_agent_sdk::root_mgmt::encode_instance_query(target))?;
        let (gen, _lc, restarts, ready) =
            eneros_agent_sdk::root_mgmt::parse_instance_resp(&resp).ok()?;
        if gen > base_gen {
            return Some((
                OpStatus::Completed,
                format!(
                    "restart-complete:target={target} gen={gen} restarts={restarts} ready={ready}"
                ),
            ));
        }
        if now_ms.saturating_sub(record.created_ms) > RESTART_TIMEOUT_MS {
            return Some((
                OpStatus::Failed,
                format!(
                    "restart-timeout:target={target} base_gen={base_gen} last_gen={gen}; verify root serial (MGMT-RESTART words)"
                ),
            ));
        }
        None // 仍在途
    }

    fn execute(
        &mut self,
        action: &eneros_business_services::mgmt_rpc::MgmtAction,
        session: &eneros_business_services::mgmt_rpc::AuthenticatedSession,
        arg: &[u8],
        now_ms: u64,
    ) -> eneros_business_services::mgmt_rpc::ActionResult {
        use alloc::format;

        use eneros_business_services::mgmt_rpc::*;
        match action {
            // 50（WP-50.2）+ 70.6：服务状态（空 arg）/ op 回查在 RPC 层分派。
            MgmtAction::StatusQuery => {
                let seen = self.requests_seen;
                let maint = self
                    .root_call(&eneros_agent_sdk::root_mgmt::encode_maint_query())
                    .and_then(|r| eneros_agent_sdk::root_mgmt::parse_maint_query_resp(&r).ok())
                    .map(|(a, _)| a)
                    .unwrap_or(false);
                ActionResult {
                    status: RpcStatus::Completed,
                    detail: format!("status:ok:up@{now_ms} req={seen} maint={maint}"),
                    payload: None,
                }
            }
            // 70.6（WP-70.6.2）：ConfigGet 返回获准载荷 + 摘要 + 代次；
            // 敏感配置（ota/security 含密钥材料）只回摘要 + redacted 标记
            //（密钥字节不出服务边界——越权敏感字段负例的结构性拒绝）。
            MgmtAction::ConfigGet => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                let id = core::str::from_utf8(arg).unwrap_or("");
                if id.is_empty() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-get:empty-id".into(),
                        payload: None,
                    };
                }
                const SENSITIVE: [&str; 2] = ["ota", "security"];
                if SENSITIVE.contains(&id)
                    && !(session.has_role("admin") || session.has_role("superuser"))
                {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("config-sensitive-requires-admin:{id}"),
                        payload: None,
                    };
                }
                let Some(client) = &mut self.config_ep else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                        payload: None,
                    };
                };
                let resp = eneros_agent_sdk::config_client::get_entry(client, id);
                match resp {
                    Ok(raw) => {
                        let digest = eneros_crypto::sm3_hash(&raw);
                        let gen = self.config_generation();
                        if SENSITIVE.contains(&id) {
                            // 密钥材料脱敏：不回载荷，只回摘要与代次。
                            ActionResult {
                                status: RpcStatus::Completed,
                                detail: format!(
                                    "config-get:{id}:gen={gen}:digest={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}:len={}:redacted=secret",
                                    digest[0], digest[1], digest[2], digest[3],
                                    digest[4], digest[5], digest[6], digest[7],
                                    raw.len()
                                ),
                                payload: None,
                            }
                        } else {
                            ActionResult {
                                status: RpcStatus::Completed,
                                detail: format!(
                                    "config-get:{id}:gen={gen}:digest={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}:len={}",
                                    digest[0], digest[1], digest[2], digest[3],
                                    digest[4], digest[5], digest[6], digest[7],
                                    raw.len()
                                ),
                                payload: Some(raw),
                            }
                        }
                    }
                    Err(e) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("config-get-eipc:{e:?}"),
                        payload: None,
                    },
                }
            }
            // 70.6（WP-70.6.2）：logging **配置**查询（载荷 + 摘要）；
            // 运行日志请求显式未接实（"runtime"），与配置查询分层。
            MgmtAction::LogQuery => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                if core::str::from_utf8(arg).unwrap_or("") == "runtime" {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "log-runtime-unwired:support-bundle-only".into(),
                        payload: None,
                    };
                }
                let Some(client) = &mut self.config_ep else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                        payload: None,
                    };
                };
                let resp = eneros_agent_sdk::config_client::get_entry(client, "logging");
                match resp {
                    Ok(raw) => {
                        let digest = eneros_crypto::sm3_hash(&raw);
                        let gen = self.config_generation();
                        ActionResult {
                            status: RpcStatus::Completed,
                            detail: format!(
                                "log-config:gen={gen}:digest={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}:len={}",
                                digest[0], digest[1], digest[2], digest[3],
                                digest[4], digest[5], digest[6], digest[7],
                                raw.len()
                            ),
                            payload: Some(raw),
                        }
                    }
                    Err(e) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("log-query-eipc:{e:?}"),
                        payload: None,
                    },
                }
            }
            // 70.6（WP-70.6.2）：支持包 = 分节快照（seq 绑定代次）+ 分页
            // 读取。arg 空 = 新快照 + 第 0 页；arg = seq u32 ‖ page u32 =
            // 续取（seq 过期显式拒绝，不冒称同一快照）。
            MgmtAction::SupportBundleExport => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                if arg.len() == 8 {
                    let seq = u32::from_le_bytes(arg[0..4].try_into().unwrap());
                    let page = u32::from_le_bytes(arg[4..8].try_into().unwrap());
                    return match &self.snapshot {
                        Some((s, pages)) if *s == seq => match pages.get(page as usize) {
                            Some(p) => ActionResult {
                                status: RpcStatus::Completed,
                                detail: format!(
                                    "support-bundle:seq={seq} page={page}/{} len={}",
                                    pages.len(),
                                    p.len()
                                ),
                                payload: Some(p.clone()),
                            },
                            None => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!("support-bundle:page-out-of-range:{page}"),
                                payload: None,
                            },
                        },
                        Some((s, _)) => ActionResult {
                            status: RpcStatus::Failed,
                            detail: format!("support-bundle:stale-seq:{seq} (current={s})"),
                            payload: None,
                        },
                        None => ActionResult {
                            status: RpcStatus::Failed,
                            detail: "support-bundle:no-snapshot".into(),
                            payload: None,
                        },
                    };
                }
                // 新快照。
                self.bundle_seq = self.bundle_seq.saturating_add(1);
                let body = self.build_snapshot(now_ms);
                if body.len() > BUNDLE_BUDGET {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("support-bundle:budget-overflow:{}", body.len()),
                        payload: None,
                    };
                }
                let pages: alloc::vec::Vec<alloc::vec::Vec<u8>> =
                    body.chunks(BUNDLE_PAGE).map(|c| c.to_vec()).collect();
                let n = pages.len();
                let seq = self.bundle_seq;
                self.snapshot = Some((seq as u32, pages));
                let first = self
                    .snapshot
                    .as_ref()
                    .and_then(|(_, ps)| ps.first().cloned())
                    .unwrap_or_default();
                ActionResult {
                    status: RpcStatus::Completed,
                    detail: format!(
                        "support-bundle:seq={seq} pages={n} bytes={} (fetch: seq u32 ‖ page u32)",
                        body.len()
                    ),
                    payload: Some(first),
                }
            }
            // 70.6（WP-70.6.3）：受控重启经 root MGT2（allow/deny 在 root
            // 裁决）；操作账本在 RPC 层持有 Running 记录，观察面 =
            // status-query op:<id> 轮询（实例代次变更 + ready 重验）。
            MgmtAction::AgentRestart => {
                self.restart_requests = self.restart_requests.saturating_add(1);
                self.requests_seen = self.requests_seen.saturating_add(1);
                let target = core::str::from_utf8(arg)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if target.is_empty() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "restart:empty-target".into(),
                        payload: None,
                    };
                }
                // 基线代次（重启成功判据 = gen 越过基线）。
                let base_gen = self
                    .root_call(&eneros_agent_sdk::root_mgmt::encode_instance_query(target))
                    .and_then(|r| eneros_agent_sdk::root_mgmt::parse_instance_resp(&r).ok())
                    .map(|(g, _, _, _)| g);
                let Some(base_gen) = base_gen else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("restart:instance-unavailable:{target}").into(),
                        payload: None,
                    };
                };
                let resp = self.root_call(&eneros_agent_sdk::root_mgmt::encode_restart(target));
                let ack = resp.as_deref().map(eneros_agent_sdk::root_mgmt::parse_ack);
                match ack {
                    Some(Ok(())) => ActionResult {
                        status: RpcStatus::Accepted,
                        detail: format!("restart {target} base_gen={base_gen}"),
                        payload: None,
                    },
                    Some(Err(reason)) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("restart-denied:{reason}").into(),
                        payload: None,
                    },
                    None => ActionResult {
                        status: RpcStatus::Failed,
                        detail: "restart:root-mgmt-unavailable".into(),
                        payload: None,
                    },
                }
            }
            // 70.6（WP-70.6.3）：维护进入/退出（root 健康门裁决）。
            MgmtAction::MaintenanceEnter | MgmtAction::MaintenanceExit => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                let req = if matches!(action, MgmtAction::MaintenanceEnter) {
                    eneros_agent_sdk::root_mgmt::encode_maint_enter()
                } else {
                    eneros_agent_sdk::root_mgmt::encode_maint_exit()
                };
                let resp = self.root_call(&req);
                let ack = resp.as_deref().map(eneros_agent_sdk::root_mgmt::parse_ack);
                match ack {
                    Some(Ok(())) => ActionResult {
                        status: RpcStatus::Completed,
                        detail: format!(
                            "maintenance-{}:ok",
                            if matches!(action, MgmtAction::MaintenanceEnter) {
                                "enter"
                            } else {
                                "exit"
                            }
                        ),
                        payload: None,
                    },
                    Some(Err(reason)) => ActionResult {
                        status: RpcStatus::Failed,
                        detail: format!("maintenance-denied:{reason}").into(),
                        payload: None,
                    },
                    None => ActionResult {
                        status: RpcStatus::Failed,
                        detail: "maintenance:root-mgmt-unavailable".into(),
                        payload: None,
                    },
                }
            }
            // 70.6（WP-70.6.4）：签名配置换包事务（多段 stage → commit →
            // activate；C/A 相位要求维护窗口激活——DEC-70-06 停机换包）。
            MgmtAction::ConfigSet => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                use eneros_business_services::config_service;
                if self.config_ep.is_none() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-service-not-bound".into(),
                        payload: None,
                    };
                }
                let Some(phase) = arg.first() else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-set:empty-phase".into(),
                        payload: None,
                    };
                };
                match phase {
                    b'S' => {
                        // arg = 'S' ‖ seq(1) ‖ total(1) ‖ bytes。
                        if arg.len() < 3 {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "config-set:bad-part".into(),
                                payload: None,
                            };
                        }
                        let mut req = alloc::vec::Vec::with_capacity(arg.len());
                        req.extend_from_slice(config_service::PROTO_MAGIC);
                        req.push(config_service::CMD_STAGE_PART);
                        req.extend_from_slice(&arg[1..]);
                        let Some(client) = &mut self.config_ep else {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "config-service-not-bound".into(),
                                payload: None,
                            };
                        };
                        match client.call("svc-config", &req) {
                            Ok(r) if !r.is_empty() => ActionResult {
                                status: RpcStatus::Completed,
                                detail: format!("config-stage-part:{}/{} accepted", r[0], arg[2]),
                                payload: None,
                            },
                            Ok(r) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!(
                                    "config-stage-rejected:{}",
                                    core::str::from_utf8(&r).unwrap_or("?")
                                ),
                                payload: None,
                            },
                            Err(e) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!("config-stage-eipc:{e:?}"),
                                payload: None,
                            },
                        }
                    }
                    b'C' => {
                        // arg = 'C' ‖ 真实束长 u32 LE（暂存缓冲按 512B 对齐
                        // 分片，尾部零填充——svc-config 据此截断解码）。
                        if arg.len() < 5 {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "config-set:bad-commit".into(),
                                payload: None,
                            };
                        }
                        let mut req = alloc::vec::Vec::new();
                        req.extend_from_slice(config_service::PROTO_MAGIC);
                        req.push(config_service::CMD_STAGE_COMMIT);
                        req.extend_from_slice(&arg[1..5]);
                        // 宽松超时：commit 侧含 bundle 级 SM2 验签 + 消费者
                        // 兼容全检（TCG 下 SM2 ~2s/次——5s 默认必炸）。
                        let Some(client) = &mut self.config_ep else {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "config-service-not-bound".into(),
                                payload: None,
                            };
                        };
                        match client.call_with_timeout("svc-config", &req, 60_000) {
                            Ok(r) if r.len() == 32 => {
                                let hex: alloc::string::String =
                                    r[..8].iter().map(|b| format!("{b:02x}")).collect();
                                ActionResult {
                                    status: RpcStatus::Completed,
                                    detail: format!("config-staged:digest={hex}…"),
                                    payload: Some(r),
                                }
                            }
                            Ok(r) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!(
                                    "config-commit-rejected:{}",
                                    core::str::from_utf8(&r).unwrap_or("?")
                                ),
                                payload: None,
                            },
                            Err(e) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!("config-commit-eipc:{e:?}"),
                                payload: None,
                            },
                        }
                    }
                    b'A' => {
                        // 维护窗口闸（DEC-70-06）：未进维护 → 拒绝激活。
                        let maint = self
                            .root_call(&eneros_agent_sdk::root_mgmt::encode_maint_query())
                            .and_then(|r| {
                                eneros_agent_sdk::root_mgmt::parse_maint_query_resp(&r).ok()
                            })
                            .map(|(a, _)| a);
                        if maint != Some(true) {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "maintenance-required:activate".into(),
                                payload: None,
                            };
                        }
                        let Some(client) = &mut self.config_ep else {
                            return ActionResult {
                                status: RpcStatus::Failed,
                                detail: "config-service-not-bound".into(),
                                payload: None,
                            };
                        };
                        let mut req = alloc::vec::Vec::new();
                        req.extend_from_slice(config_service::PROTO_MAGIC);
                        req.push(config_service::CMD_ACTIVATE);
                        // 宽松超时同 commit 臂（盘写 8KiB + 启动级验签）。
                        match client.call_with_timeout("svc-config", &req, 60_000) {
                            Ok(r) if r.len() == 8 => {
                                let gen = u64::from_le_bytes(r[..8].try_into().unwrap());
                                ActionResult {
                                    status: RpcStatus::Completed,
                                    detail: format!("config-activated:gen={gen}"),
                                    payload: Some(r),
                                }
                            }
                            Ok(r) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!(
                                    "config-activate-rejected:{}",
                                    core::str::from_utf8(&r).unwrap_or("?")
                                ),
                                payload: None,
                            },
                            Err(e) => ActionResult {
                                status: RpcStatus::Failed,
                                detail: format!("config-activate-eipc:{e:?}"),
                                payload: None,
                            },
                        }
                    }
                    _ => ActionResult {
                        status: RpcStatus::Failed,
                        detail: "config-set:unknown-phase (S|C|A)".into(),
                        payload: None,
                    },
                }
            }
            // SPEC-C（WP-C.2）：北向历史取回（既有实路径，零回归）。
            MgmtAction::HistoryQuery => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                if arg.len() < 8 {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "history:bad-arg(point‖last_n 需 8B)".into(),
                        payload: None,
                    };
                }
                let point = u32::from_le_bytes(arg[0..4].try_into().unwrap_or([0; 4]));
                let last_n = u32::from_le_bytes(arg[4..8].try_into().unwrap_or([0; 4]));
                if self.telemetry_slot.is_none() {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "telemetry-not-bound".into(),
                        payload: None,
                    };
                }
                let Some(ep) = self.history_ep.as_mut() else {
                    return ActionResult {
                        status: RpcStatus::Failed,
                        detail: "history:eipc-bind-failed".into(),
                        payload: None,
                    };
                };
                match eneros_business_services::mgmt_history::fetch_history(
                    ep,
                    "svc-telemetry",
                    point,
                    last_n,
                ) {
                    Ok(f) => {
                        let mut detail = f.summary.clone();
                        if f.clamped {
                            detail.push_str(" clamp-hint:last_n>512-capped");
                        }
                        ActionResult {
                            status: RpcStatus::Completed,
                            detail,
                            payload: Some(f.rows_bin),
                        }
                    }
                    Err(e) => {
                        let reason = match e {
                            eneros_business_services::mgmt_history::HistoryError::NoSuchPoint => {
                                format!("history:no-such-point:{point}")
                            }
                            eneros_business_services::mgmt_history::HistoryError::Transport => {
                                "history:eipc-transport".into()
                            }
                            eneros_business_services::mgmt_history::HistoryError::BadFrame => {
                                "history:bad-frame".into()
                            }
                        };
                        ActionResult {
                            status: RpcStatus::Failed,
                            detail: reason,
                            payload: None,
                        }
                    }
                }
            }
            MgmtAction::UpdateCheck | MgmtAction::UpdateApply | MgmtAction::UpdateRollback => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                ActionResult {
                    status: RpcStatus::Failed,
                    detail: ota_channel_unbound_detail().into(),
                    payload: None,
                }
            }
            _ => {
                self.requests_seen = self.requests_seen.saturating_add(1);
                ActionResult {
                    status: RpcStatus::Failed,
                    detail: format!("action-not-wired:{action:?}"),
                    payload: None,
                }
            }
        }
    }
}

#[cfg(target_os = "none")]
struct MgmtHandler {
    /// unix 偏移（墙钟秒*1000 - 启动单调 ms；None = 时间缺失）。
    unix_offset_ms: Option<u64>,
    rpc: eneros_business_services::mgmt_rpc::ManagementRpc<TargetExecutor>,
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for MgmtHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::mgmt_principal::{self, SessionShape};
        use eneros_business_services::mgmt_rpc::*;
        let handled = self.handle_frame(req, resp, now_ms);
        // 70.6（WP-70.6.1）：账本变更落盘（dirty 旗单点——读请求零盘写）。
        if self.rpc.ops.take_dirty() {
            let ops = self.rpc.ops.clone();
            self.rpc.executor.persist_ops(&ops);
        }
        handled
    }
}

#[cfg(target_os = "none")]
impl MgmtHandler {
    fn handle_frame(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        use eneros_business_services::mgmt_principal::{self, SessionShape};
        use eneros_business_services::mgmt_rpc::*;
        const SESSION_LEN: usize = mgmt_principal::SESSION_V1_LEN;
        const V2_TAIL_LEN: usize = mgmt_principal::SESSION_V2_LEN - SESSION_LEN;
        // 70.5（WP-70.5.6）：station 公钥部署面——
        // 在位 → MGT1 v2（session 48B ‖ token 64B ‖ sig 字段 74B）强制：
        //   验签通过才构建会话（wire 角色/主字段不可信——签名绑定）；
        //   旧 48B 无签会话 = ERR:principal-unverified（不降级）。
        // 缺失 → 显式开发模式：48B 旧面保留（qemu-test 兼容；启动日志
        // 已明示）。
        let station_key = eneros_business_services::STATION_PUBKEY_SEC1;
        let station_deployed = !station_key.is_empty();
        // 形状判别单源 `classify_session`（token 魔数位置 + 长度下界）：
        // 三臂互斥完备——70.5 复查 P0：带魔数却尾段不足的 62..199B 帧曾
        // 三臂全不命中直落 v1 无签解析（伪造 48B 会话块 admin 直达
        // rpc.handle）；现任何模式下截断 v2 = 协议违例。
        match mgmt_principal::classify_session(req) {
            SessionShape::Short => {
                resp.extend_from_slice(b"ERR:proto");
                return false;
            }
            SessionShape::V2Truncated => {
                eneros_runtime::println!(
                    "[svc-mgmt] MGT-PRINCIPAL rejected (truncated v2 shape len={}) —— 不落回 v1 无签解析",
                    req.len()
                );
                resp.extend_from_slice(b"ERR:proto");
                return false;
            }
            // station 部署面：旧 48B 无签形 = 一律拒绝——静默回退 v1 =
            // 签名 principal 强制面开洞（70.5 实弹自咬修复）。
            SessionShape::V1 if station_deployed => {
                eneros_runtime::println!(
                    "[svc-mgmt] MGT-PRINCIPAL rejected (unverified session shape) —— station 部署面 v2 强制"
                );
                resp.extend_from_slice(b"ERR:principal-unverified");
                return false;
            }
            SessionShape::V2 if station_deployed => {
                let tail = &req[5 + SESSION_LEN..5 + SESSION_LEN + V2_TAIL_LEN];
                let Some(offset) = self.unix_offset_ms else {
                    eneros_runtime::println!(
                        "[svc-mgmt] MGT-PRINCIPAL rejected (wall clock unavailable) —— 过期判据 fail-closed"
                    );
                    resp.extend_from_slice(b"ERR:time-unavailable");
                    return false;
                };
                let now_unix_ms = now_ms.saturating_add(offset);
                match mgmt_principal::verify_v2_tail(tail, station_key, now_unix_ms) {
                    Ok(vp) => {
                        let request_id = u64::from_le_bytes(
                            req[5 + SESSION_LEN + V2_TAIL_LEN..5 + SESSION_LEN + V2_TAIL_LEN + 8]
                                .try_into()
                                .unwrap(),
                        );
                        let action_len = req[5 + SESSION_LEN + V2_TAIL_LEN + 8] as usize;
                        let action_start = 5 + SESSION_LEN + V2_TAIL_LEN + 9;
                        if action_start + action_len > req.len() {
                            resp.extend_from_slice(b"ERR:len");
                            return false;
                        }
                        let action =
                            core::str::from_utf8(&req[action_start..action_start + action_len])
                                .unwrap_or("")
                                .to_string();
                        let arg = &req[action_start + action_len..];
                        let session = AuthenticatedSession {
                            principal: vp.principal,
                            roles: vp.roles,
                            tenant_scope: vp.tenant_scope,
                            channel_binding: vp.channel_binding,
                            // 70.6（REG-70.5-MGMT-EXPIRES-UNIT 收口）：v2
                            // 会话过期判据 = verify_v2_tail（unix ms 单源，
                            // 已通过才到达此处）——本字段置 0 哨兵（永不
                            // 过期），消除"unix ms 值被当单调 ms 比较"的
                            // 公共合同单位陷阱。
                            expires_at_ms: 0,
                        };
                        let result = self.rpc.handle(&session, request_id, &action, arg, now_ms);
                        // 70.6（WP-70.6.1）：管理审计串口行（每请求一行；
                        // 响应词形在无下行应答通道的探针窗内亦可取证）。
                        eneros_runtime::println!(
                            "[svc-mgmt] MGT1: rid={request_id} {action} -> {:?} {}",
                            result.status,
                            result.detail
                        );
                        resp.push(match result.status {
                            RpcStatus::Completed => 1u8,
                            RpcStatus::Accepted => 2,
                            RpcStatus::Failed => 0,
                        });
                        resp.extend_from_slice(result.detail.as_bytes());
                        if let Some(p) = &result.payload {
                            if !p.is_empty() {
                                resp.extend_from_slice(&(p.len() as u32).to_le_bytes());
                                resp.extend_from_slice(p);
                            }
                        }
                        return true;
                    }
                    Err(e) => {
                        eneros_runtime::println!("[svc-mgmt] MGT-PRINCIPAL rejected ({e:?})");
                        resp.extend_from_slice(b"ERR:principal");
                        return false;
                    }
                }
            }
            // 开发模式（station 公钥未部署）：旧 48B 无签面保留——完整 v2
            // 帧亦按 v1 解析（既有语义；qemu-test 兼容探针同纪律）。
            SessionShape::V1 | SessionShape::V2 => {}
        }
        let p = &req[5..];
        let mut principal = [0u8; 16];
        principal.copy_from_slice(&p[0..16]);
        let mut roles = [0u8; 4];
        roles.copy_from_slice(&p[16..20]);
        // 50（WP-50.2）：4B 角色字段 = **mgmt-api 角色码**（Role::to_u8 LE
        // 首字节、余位补零——字段装不下完整角色名，run8 实弹根因）。兼容
        // 旧字符串面（首字节非 {0..3} 码 → 按 ASCII 解析；全零 = 旧缺省
        // viewer 同语义）。
        let role = match eneros_mgmt_api::Role::from_u8(roles[0]) {
            Some(r) if roles[1] == 0 && roles[2] == 0 && roles[3] == 0 => {
                alloc::string::String::from(r.name())
            }
            _ => {
                let legacy = alloc::string::String::from_utf8_lossy(&roles).into_owned();
                legacy.trim_end_matches('\0').to_string()
            }
        };
        let tenant = u32::from_le_bytes(p[20..24].try_into().unwrap());
        let mut channel = [0u8; 16];
        channel.copy_from_slice(&p[24..40]);
        let expiry = u64::from_le_bytes(p[40..48].try_into().unwrap());
        let request_id = u64::from_le_bytes(p[48..56].try_into().unwrap());
        let action_len = req[5 + SESSION_LEN + 8] as usize;
        let action_start = 5 + SESSION_LEN + 9;
        if action_start + action_len > req.len() {
            resp.extend_from_slice(b"ERR:len");
            return false;
        }
        let action = core::str::from_utf8(&req[action_start..action_start + action_len])
            .unwrap_or("")
            .to_string();
        let arg = &req[action_start + action_len..];

        let session = AuthenticatedSession {
            principal: alloc::string::String::from_utf8_lossy(&principal)
                .trim_end_matches('\0')
                .to_string(),
            roles: vec![if role.is_empty() {
                "viewer".into()
            } else {
                role
            }],
            tenant_scope: tenant,
            channel_binding: alloc::string::String::from_utf8_lossy(&channel)
                .trim_end_matches('\0')
                .to_string(),
            expires_at_ms: expiry,
        };
        let result = self.rpc.handle(&session, request_id, &action, arg, now_ms);
        eneros_runtime::println!(
            "[svc-mgmt] MGT1: rid={request_id} {action} -> {:?} {}",
            result.status,
            result.detail
        );
        resp.push(match result.status {
            RpcStatus::Completed => 1u8,
            RpcStatus::Accepted => 2,
            RpcStatus::Failed => 0,
        });
        resp.extend_from_slice(result.detail.as_bytes());
        // SPEC-C：二进制载荷扩展段（history-query 行区等）。旧消费方按
        // detail 文本前缀解析不受影响；新消费方读尾部 [len][bytes]。
        if let Some(p) = &result.payload {
            if !p.is_empty() {
                resp.extend_from_slice(&(p.len() as u32).to_le_bytes());
                resp.extend_from_slice(p);
            }
        }
        true
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}

#[cfg(test)]
mod ota_word_form_tests {
    use eneros_business_services::mgmt_rpc::MgmtAction;

    use crate::ota_channel_unbound_detail;

    /// 蓝图60 D60.2.2：OTA 三动作降级词形钉住——词形自带 EIPC 授予边
    /// 缺失原因（mgmt→ota-grant-missing），与 `action-not-wired` 裸词形分层。
    #[test]
    fn ota_actions_report_channel_unbound_reason() {
        let d = ota_channel_unbound_detail();
        assert_eq!(d, "ota-channel-unbound:mgmt→ota-grant-missing");
        assert!(d.starts_with("ota-channel-unbound:"), "{d}");
        assert!(d.contains("mgmt→ota-grant-missing"), "{d}");
        assert!(!d.contains("action-not-wired"), "{d}");
        // 三动作可解析且不在接实白名单（保证路由进 OTA 降级分支而非
        // 通用 action-not-wired 臂）。
        for raw in ["update-check", "update-apply", "update-rollback"] {
            let a = MgmtAction::parse(raw).expect("ota action must parse");
            assert!(!a.in_whitelist(), "{raw} 不应在 svc-mgmt 接实白名单");
        }
    }

    /// 蓝图60 D60.2.3 披露清单计数钉住：22 动作 = 6 接实 + 3 OTA 未绑定
    /// 以及 13 个 `action-not-wired` 裸词形未接（README「管理面动作接线
    /// 现状」小节的数字以本测试为单一事实源）。
    #[test]
    fn action_surface_counts_match_disclosure() {
        let all = [
            "status",
            "slo",
            "agent-list",
            "agent-start",
            "agent-stop",
            "agent-restart",
            "config-get",
            "config-set",
            "log-query",
            "support-bundle",
            "crash-symbolize",
            "update-check",
            "update-apply",
            "update-rollback",
            "maintenance-enter",
            "maintenance-exit",
            "backup-create",
            "restore",
            "revoke",
            "factory-reset",
            "secure-erase",
            "history-query",
        ];
        assert_eq!(all.len(), 22);
        let ota = ["update-check", "update-apply", "update-rollback"];
        let mut n_wired = 0usize;
        let mut n_ota = 0usize;
        for raw in all {
            let a = MgmtAction::parse(raw).expect("action must parse");
            if a.in_whitelist() {
                n_wired += 1;
            }
            if ota.contains(&raw) {
                n_ota += 1;
            }
        }
        assert_eq!(n_wired, 6, "接实白名单面");
        assert_eq!(n_ota, 3, "OTA 三动作（ota-channel-unbound 词形）");
        assert_eq!(22 - n_wired - n_ota, 13, "action-not-wired 裸词形面");
        // 穷尽性哨兵（code-review 加固）：MgmtAction 增删变体时本 match
        // 编译失败，强制同步上方名册与 README 披露清单，防止新增动作
        // 静默落在披露面之外。
        let _exhaustive_roster_guard = |a: &MgmtAction| match a {
            MgmtAction::StatusQuery
            | MgmtAction::SloQuery
            | MgmtAction::AgentList
            | MgmtAction::AgentStart
            | MgmtAction::AgentStop
            | MgmtAction::AgentRestart
            | MgmtAction::ConfigGet
            | MgmtAction::ConfigSet
            | MgmtAction::LogQuery
            | MgmtAction::SupportBundleExport
            | MgmtAction::CrashDumpSymbolize
            | MgmtAction::UpdateCheck
            | MgmtAction::UpdateApply
            | MgmtAction::UpdateRollback
            | MgmtAction::MaintenanceEnter
            | MgmtAction::MaintenanceExit
            | MgmtAction::BackupCreate
            | MgmtAction::Restore
            | MgmtAction::Revoke
            | MgmtAction::FactoryReset
            | MgmtAction::SecureErase
            | MgmtAction::HistoryQuery => (),
        };
    }
}
