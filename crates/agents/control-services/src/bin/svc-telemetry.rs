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
    // 覆盖声明（与部署配置一致：电压/电流/频率/SOC/电网/过流/过温）。
    let config = eneros_safety_validator::EvaluatorConfig {
        coverage: eneros_safety_validator::RequiredPointCoverage {
            points: alloc::vec![
                eneros_safety_validator::RequiredPoint {
                    point: 1,
                    min: 0.0,
                    max: 1000.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Volt
                },
                eneros_safety_validator::RequiredPoint {
                    point: 2,
                    min: -1000.0,
                    max: 1000.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ampere
                },
                eneros_safety_validator::RequiredPoint {
                    point: 3,
                    min: 45.0,
                    max: 55.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Hertz
                },
                eneros_safety_validator::RequiredPoint {
                    point: 4,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Ratio
                },
                eneros_safety_validator::RequiredPoint {
                    point: 5,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Other
                },
                eneros_safety_validator::RequiredPoint {
                    point: 6,
                    min: 0.0,
                    max: 1.0,
                    max_age_ms: 5000,
                    unit: eneros_safety_validator::PointUnit::Other
                },
            ],
        },
        bindings: eneros_safety_validator::StateBindings {
            voltage: 1,
            current: 2,
            frequency: 3,
            soc: 4,
        },
        mapping_version: 1,
        max_skew_ms: 1000,
        allowed_sources: alloc::vec![1],
        require_integrity: true,
        control_horizon_ms: 30_000,
    };
    let mut service = eneros_control_services::telemetry::TelemetryService::new(config);
    // SPEC-GAP-CLOSURE WP-2：批次边界历史环缺省在位（内存前置，磁盘适配器
    // 待 D-TSDB-2 裁决）。容量审计见 docs/delivery/wp2-memory-audit-d-tsdb2.md
    // ——缺省 512 条 ×24B ≈12KiB，扩容 MUST 先出静态内存审计表（R2）。
    service.set_history_sink(alloc::boxed::Box::new(
        eneros_control_services::telemetry::HistoryBuffer::new(
            eneros_control_services::telemetry::DEFAULT_HISTORY_CAP,
        ),
    ));
    let mut handler = TelemetryHandler { svc: &mut service };
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
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for TelemetryHandler<'_> {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
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
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
