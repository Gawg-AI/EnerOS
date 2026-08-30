//! svc-telemetry — 蓝图 40 WP-40.2：可信采集服务（EL0）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

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
    // 独立过温告警（peer 0x0106）入覆盖表，6/6 别名解除（见该常量文档）。
    let config = eneros_safety_validator::EvaluatorConfig {
        coverage: eneros_control_dispatch::feedback::safety_coverage(),
        bindings: eneros_safety_validator::StateBindings {
            voltage: 1,
            current: 2,
            frequency: 3,
            soc: 4,
        },
        mapping_version: 1,
        max_skew_ms: 1000,
        // 兼容轨初值：1 = root 合成设施（qemu-test-synth feature；qemu-prod
        // 闭包不含）。蓝图 61：配置装载后（首个 OBS 批次）以 [[source]]
        // 注册来源集合覆盖（apply_source_identity）——生产身份零硬编码。
        allowed_sources: alloc::vec![1],
        require_integrity: true,
        control_horizon_ms: 30_000,
    };
    let mut service = eneros_control_services::telemetry::TelemetryService::new(config);
    // SPEC-GAP-CLOSURE WP-2：批次边界历史环缺省在位（内存前置）。
    // SPEC-C（D-TSDB-2 方案 A）：磁盘 TSDB 接缝（EIPC 授予边 → LFS →
    // TimeSeriesDB）**target 侧显式停用**（E-DEF-01，2026-08-28 win4 实测）：
    // littlefs2 每操作 mount_and_then × BlockIo 52B 分片叠加，单个 LFS 操作
    // 2k-13k 次块命令往返（串口行 6k-40k/操作）——单 checkpoint 分钟级，
    // TEL_INGEST 5s deadline 物理不可满足，且启动期 format+mount 拖住整编队
    // readiness。修复方向（burst 块协议 / PAYLOAD_CAP 扩容+内存重审 /
    // eneros-fs 持久 mount）归 C 卷 owner 裁决；本接缝代码与
    // telemetry_history 读面保留（host FakeStorage 全链测试在守），修复后
    // 恢复下方注释的授予边装配即可。TELH 查询回 HIST_UNAVAILABLE（既有
    // 降级词形），采集线走内存环（TSDB-COMMIT 基线词形）——不伪造持久化。
    let history_reader: Option<eneros_control_services::telemetry_history::TsdbHistoryReader> =
        None;
    eneros_runtime::println!(
        "[svc-telemetry] TSDB-DISK DISABLED (E-DEF-01) —— 内存环接替，TELH Unavailable"
    );
    service.set_history_sink(alloc::boxed::Box::new(
        eneros_control_services::telemetry::HistoryBuffer::new(
            eneros_control_services::telemetry::DEFAULT_HISTORY_CAP,
        ),
    ));
    // 蓝图 61：svc-config 客户端（授予边 eipc_grants 单源；槽解析无 I/O
    // ——真实取数惰性发生在首个 OBS 批次，此时 svc-config 必已服务）。
    let config_client: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient> =
        eneros_agent_sdk::eipc_resolve("svc-telemetry", "svc-config").and_then(|(_, slot)| {
            eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
                "telemetry-config",
                slot,
                eneros_drv_svc::now_ns,
            )
            .ok()
        });
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
    config: Option<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
}

#[cfg(target_os = "none")]
impl TelemetryHandler<'_> {
    /// 蓝图 61 v0.314.27：惰性来源身份装载（首个 OBS 批次触发；一次成功
    /// 后短路）。取数失败 → false（下一轮 OBS 重试；期间 legacy 兼容轨
    /// 继续以装配参数运行）。
    fn ensure_source_identity(&mut self) -> bool {
        if self.svc.obs.is_some() {
            return true;
        }
        let Some(client) = self.config.as_mut() else {
            return false;
        };
        let cfg_id = eneros_config_bundle::ConfigId::from_static("points");
        // generation（GET_GENERATION；失败 = 0 → 世代未定，观测不满足控制
        // 资格——fail-closed）。
        let generation = eneros_agent_sdk::config_client::get_generation(client, 0, 5_000)
            .ok()
            .unwrap_or(0);
        let Some(payload) = eneros_agent_sdk::config_client::get_entry(client, "points", 0, 5_000)
            .ok()
            .and_then(|resp| {
                eneros_agent_sdk::config_client::decode_response_body(&resp)
                    .ok()
                    .map(|b| b.to_vec())
            })
        else {
            eneros_runtime::println!(
                "[svc-telemetry] OBS-CFG points fetch FAIL (retry next round)"
            );
            return false;
        };
        let Ok(table) = eneros_config_bundle::points::decode_points(&payload, &cfg_id) else {
            eneros_runtime::println!("[svc-telemetry] OBS-CFG points decode FAIL");
            return false;
        };
        if table.sources.is_empty() {
            eneros_runtime::println!("[svc-telemetry] OBS-CFG no [[source]] rows (legacy bundle)");
            return false;
        }
        let descriptors = eneros_control_services::obs_shadow::descriptors_from_config(
            &table.sources,
            &table.points,
        );
        // 生产评估器重建：allowed_sources / mapping_version 换为配置事实
        // （apply_source_identity 以注册来源集合**覆盖**——configs/points.toml
        // 的 source 1 = root 合成设施，刻意不注册，合成样本只走旧兼容轨；
        // mapping_version = bundle generation，零硬编码）。
        let eval_config = eneros_safety_validator::EvaluatorConfig {
            coverage: eneros_control_dispatch::feedback::safety_coverage(),
            bindings: eneros_safety_validator::StateBindings {
                voltage: 1,
                current: 2,
                frequency: 3,
                soc: 4,
            },
            mapping_version: generation,
            max_skew_ms: 1000,
            allowed_sources: alloc::vec![],
            require_integrity: true,
            control_horizon_ms: 30_000,
        };
        self.svc.apply_source_identity(descriptors, eval_config);
        eneros_runtime::println!(
            "[svc-telemetry] OBS-CFG loaded sources={} generation={}",
            table.sources.len(),
            generation
        );
        true
    }
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
        // 蓝图 61：首个 OBS 批次 → 惰性装载来源身份（一次成功后短路）。
        if req.first() == Some(&eneros_control_services::telemetry::TEL_INGEST_OBS) {
            let _ = self.ensure_source_identity();
        }
        if req.first() == Some(&eneros_control_services::telemetry::TEL_SNAPSHOT) {
            match self.svc.snapshot(now_ms) {
                Ok(t) => {
                    resp.push(eneros_control_services::telemetry::SNAPSHOT_OK);
                    resp.extend_from_slice(&eneros_control_services::telemetry::encode_trusted(&t));
                }
                Err(e) => {
                    let age0 = self.svc.first_sample_age_ms(now_ms).unwrap_or(u64::MAX);
                    eneros_runtime::println!(
                        "[svc-telemetry] SNAPSHOT rejected: {:?} (ingested={} now={} first_age={})",
                        e,
                        self.svc.ingested,
                        now_ms,
                        age0
                    );
                    resp.push(eneros_control_services::telemetry::SNAPSHOT_REJECTED);
                }
            }
            true
        } else {
            *resp = eneros_control_services::telemetry::handle_request(self.svc, req, now_ms);
            true
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // SPEC-E 续作：静默自旋曾把 target 侧栈溢出（win1-t2 首写挂死）藏了
    // 整个排障周期——死亡必须可见（svc-device-agent 先例同词形）。
    eneros_runtime::println!("[svc-telemetry] PANIC: {info}");
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
