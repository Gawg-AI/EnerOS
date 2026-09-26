//! svc-telemetry — 蓝图 40 WP-40.2：可信采集服务（EL0）。
//!
//! 69.3 WP-69.3.2：T1 `EvaluatorConfig` 单源构造器、T2 来源身份装载、T3
//! SNAPSHOT 拒绝判定行词形全部收编 `eneros_control_services::telemetry`
//! 库（bin 只剩装配接线 + 请求路由；词形/语义零变更，执行卡 7.1）。

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

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    _eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 覆盖声明（与部署配置一致：电压/电流/频率/SOC/电网/过流 6 点。
    // 61 v0.314.26 §4.4：覆盖声明单源 = control-dispatch feedback
    // SAFETY_COVERAGE_POINTS（历史 6 点手抄收编）。61 复查矫正：点 7 =
    // 独立过温告警（peer 0x0106）入覆盖表，6/6 别名解除。
    // 69.3 T1：构造单源化——(mapping_version=1, allowed_sources=[1]) =
    // 兼容轨初值（root 合成设施；qemu-prod 闭包不含，[[source]] 装载后
    // 以注册来源集合覆盖——生产身份零硬编码）。
    let config = eneros_control_services::telemetry::evaluator_config(1, alloc::vec![1]);
    let mut service = eneros_control_services::telemetry::TelemetryService::new(config);
    // SPEC-C 磁盘 TSDB 复通（蓝图 68 卷 68.3 WP-68.3.4；E-DEF-01 转正，
    // 2026-09-05）：EIPC 授予边（svc-telemetry→svc-storage，eipc_grants
    // 表在册）→ BlockIo 块命令面（68.3 WP-68.3.1 扩容：写 452B/读 461B
    // 分片，4KiB 块 9-10 次往返，原 52B 分片 79 次）→ eneros-fs 持久
    // mount（68.3 WP-68.3.2：构造期挂载一次，操作零重挂载）——E-DEF-01
    // 两处性能病灶（每操作 mount_and_then × 52B 分片，单 LFS 操作 2k-13k
    // 次块命令往返）均已除，checkpoint 时长经 TSDB-CHECKPOINT-DUR 行观测。
    // 任一环失败（授予边缺失/区段打开失败）→ 显式降级回内存环，不伪造
    // 持久化（与 svc-control-dispatch DurableIntentStore 兜底纪律同构）。
    let disk_track = eneros_agent_sdk::eipc_resolve("svc-telemetry", "svc-storage")
        .and_then(|(_, slot)| eneros_agent_sdk::block_io::eipc_block_io(slot, 0, 5_000))
        .and_then(|io| {
            eneros_control_services::telemetry_history::open_disk_history(
                io,
                eneros_ota_ab::ab_partition::DATA_TSDB_BASE_BLOCK,
                eneros_ota_ab::ab_partition::DATA_TSDB_BLOCKS,
            )
            .ok()
        });
    let history_reader: Option<eneros_control_services::telemetry_history::TsdbHistoryReader> =
        match disk_track {
            Some((chain, reader)) => {
                // points = index.bin 重建条目数（重启 > 0 = 旧数据保持——
                // 强杀重启窗的持久性串口证据）。
                eneros_runtime::println!(
                    "[svc-telemetry] TSDB-DISK-OPEN base_block={} region_blocks_512={} points={}",
                    eneros_ota_ab::ab_partition::DATA_TSDB_BASE_BLOCK,
                    eneros_ota_ab::ab_partition::DATA_TSDB_BLOCKS,
                    reader.index_len()
                );
                service.set_history_sink_with_media(
                    alloc::boxed::Box::new(chain),
                    eneros_control_services::telemetry::HistoryMedia::Disk,
                );
                Some(reader)
            }
            None => {
                eneros_runtime::println!(
                    "[svc-telemetry] TSDB-DISK-UNAVAILABLE —— 内存环接替，TELH Unavailable"
                );
                service.set_history_sink(alloc::boxed::Box::new(
                    eneros_control_services::telemetry::HistoryBuffer::new(
                        eneros_control_services::telemetry::DEFAULT_HISTORY_CAP,
                    ),
                ));
                None
            }
        };
    // 蓝图 68 卷 68.4（WP-68.4.1/68.4.2）：OBS 事件面 + 快照面（三冻
    // 解冻：SS/SM，事件面含 SFS 同机制）。授予边复用 svc-storage 既有
    // 边（68.3 同槽）；两块通道（事件面/快照面各一）独立打开，失败显式
    // 降级（EVENT-STORE-UNAVAILABLE / SNAP-STORE-UNAVAILABLE 词形在
    // event_face 内一次性落串口）。构造期零 I/O——lazy-open 在首个 OBS
    // 批次（boot 预算不增，REG-BOOT-SLOW-FINAL 不再翻倍）。
    let mk_face_io = || {
        eneros_agent_sdk::eipc_resolve("svc-telemetry", "svc-storage")
            .and_then(|(_, slot)| eneros_agent_sdk::block_io::eipc_block_io(slot, 0, 5_000))
    };
    match mk_face_io().zip(mk_face_io()) {
        Some((io_event, io_snap)) => {
            service.set_event_face(alloc::boxed::Box::new(
                eneros_control_services::event_face::EventFace::new(
                    io_event,
                    io_snap,
                    eneros_ota_ab::ab_partition::DataRegion::EventFace,
                    eneros_ota_ab::ab_partition::DataRegion::SnapshotFace,
                ),
            ));
            eneros_runtime::println!(
                "[svc-telemetry] EVENT-FACE armed base_block={} snap_base_block={} (lazy open on first OBS)",
                eneros_ota_ab::ab_partition::DATA_EVENT_BASE_BLOCK,
                eneros_ota_ab::ab_partition::DATA_SNAP_BASE_BLOCK
            );
        }
        None => {
            eneros_runtime::println!("[svc-telemetry] EVENT-STORE-NOT-GRANTED —— 事件面未装配");
        }
    }
    // 蓝图 61：svc-config 客户端（授予边 eipc_grants 单源；槽解析无 I/O
    // ——真实取数惰性发生在首个 OBS 批次，此时 svc-config 必已服务）。
    let config_client: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    > = eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-telemetry", "svc-config");
    if config_client.is_some() {
        eneros_runtime::println!("[svc-telemetry] config EIPC bound (lazy load on first OBS)");
    } else {
        eneros_runtime::println!("[svc-telemetry] config EIPC NOT granted (shadow disabled)");
    }
    let mut handler = TelemetryHandler {
        svc: &mut service,
        history: history_reader,
        config: config_client,
    };
    eneros_control_services::control_service_main(
        ep,
        "telemetry",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct TelemetryHandler<'a> {
    svc: &'a mut eneros_control_services::telemetry::TelemetryService,
    /// SPEC-C（WP-C.2）：磁盘历史读面（None=降级态 → TELH 回不可用错误帧）。
    history: Option<eneros_control_services::telemetry_history::TsdbHistoryReader>,
    /// 蓝图 61：svc-config 客户端（None = 未授予——影子路径如实停用）。
    /// 蓝图 62 卷 62.2（WP-62.2.4）：端点单源 ChunkedEndpoint。
    config: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
}

#[cfg(target_os = "none")]
impl TelemetryHandler<'_> {
    /// SPEC-C/A：TELR 查询服务端分支（经分块通道的 Final 载荷进入；响应由
    /// control_service_main 分页回传客户端）。全部参数防线（钳制/未知点位/
    /// 页游标 more）由 history_wire::answer_history_query 单源承载（SPEC-A
    /// 收编）——本 bin 只剩一行装配，host 测试经同一函数直测真策略。
    fn handle_history(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>) {
        resp.extend_from_slice(&eneros_agent_sdk::history_wire::answer_history_query(
            self.history.as_ref(),
            req,
        ));
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for TelemetryHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        // SPEC-C：TELR 分块载荷在既有路由前拦截（识别单源 is_tellr_request）。
        if eneros_agent_sdk::history_wire::is_tellr_request(req) {
            self.handle_history(req, resp);
            return true;
        }
        // 蓝图 61：首个 OBS 批次 → 惰性装载来源身份（69.3 T2：单源 =
        // TelemetryService::ensure_source_identity——取数/解码/目录/评估器
        // 重建/词形全在库；一次成功后短路）。
        if req.first() == Some(&eneros_control_services::telemetry::TEL_INGEST_OBS) {
            let _ = self
                .svc
                .ensure_source_identity(self.config.as_mut(), now_ms);
        }
        // TEL_SNAPSHOT 不再拦截（69.3 T3：拒绝判定行随 SNAPSHOT 臂收编
        // handle_request 单源——词形逐字不变，语义零变更）。
        *resp = eneros_control_services::telemetry::handle_request(self.svc, req, now_ms);
        true
    }
}

eneros_runtime::svc_panic!("svc-telemetry");

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
