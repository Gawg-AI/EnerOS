//! EnerOS 蓝图 41 业务/运维 EL0 服务。
//!
//! 每个服务 = 真实领域逻辑模块（no_std，host 可测）+ `bin/svc-*` 薄壳
//! （`_start` → `control_service_main` 分块 EIPC 循环，40 模式）。
//!
//! 诚实性纪律（蓝图 41 §3）：
//! - 闭包返回 Ok / Vec push / 计数增加不再是 “real”；
//! - 外部副作用（持久介质/独立 peer/虚拟设备）未就绪时显式
//!   `Unavailable/Degraded`，不伪造成功；
//! - durable 状态经 [`eneros_durable_state`] 语义层（掉电恢复在 host
//!   负例中验证；target 存储句柄移交前显式 Unavailable）。

#![cfg_attr(not(test), no_std)]
#![allow(clippy::result_large_err)]

extern crate alloc;

pub mod agent_bus;
/// 41（WP-41.3 落盘）：durable-state PersistIo 的 EIPC 块客户端
/// （业务服务经 svc-storage 真落盘；host 可测）。
pub mod block_io;
pub mod cloud_sync;
pub mod config_service;
pub mod ledger;
pub mod market;
/// R14-E（2026-09-24）：控制服务 bin 族信封单源（magic‖cmd‖payload +
/// ERR 词形；bin 族手抄收编，wire 字节零变化）。
pub mod svc_wire;

/// 蓝图 61 v0.314.31（[07] §3.4）：市场事实接入 adapter——经认证信封
/// （SM3 摘要 + SM2 验签）走同一 IngestionModule 合同；修订/过期纪律。
pub mod external_facts;
// 蓝图 68 卷 68.4（WP-68.4.1）：外部事实持久化面（SFS 解冻——svc-market
// 装配；Q0 写透 RPO=0）。
pub mod fact_face;
pub mod market_ingest;
/// 蓝图 61 v0.314.32（[07] §4.6.3）：外部事实持久化 sink——认证事实经
/// `StoreFactEventSink` 写分区事件日志（RPO=0、可恢复重放）。
pub mod store_fact_sink;
// SPEC-C（WP-C.2）：history-query 分页执行器内核（RawIpc 泛型，host 可测）。
pub mod mgmt_history;
/// 70.5（WP-70.5.6）：签名管理 principal 绑定（MGT1 v2——station 端
/// 签发 token + ECDSA P-256 验签；wire 角色字节验签后才进授权）。
pub mod mgmt_ops;
pub mod mgmt_principal;
pub mod mgmt_rpc;
pub mod observability;
pub mod ota_service;
// WP-54.5：附接核心 host 可测（无 cfg 门）；target_durable 保持 target 门。
pub mod outbox_attach;
// target_durable 引用 agent-sdk 的 `ipc_sel4`/`block_io::eipc_block_io`
// （二者均在 `sel4` feature 下才存在）——门必须与 block_io.rs 的
// eipc_block_io 同限 `all(target_os="none", feature="sel4")`，否则
// 无 sel4 feature 的 target 构建会引用被配置掉的符号而编译失败。
#[cfg(all(target_os = "none", feature = "sel4"))]
pub mod target_durable;

pub use config_service::ConfigService;
pub use ledger::Ledger;
pub use market::MarketAdapter;
pub use ota_service::OtaService;

include!(concat!(env!("OUT_DIR"), "/config_bundle.rs"));

/// 服务侧共享：HeapAllocator（与 drv-svc 同源）。
pub use eneros_drv_svc::allocator::HeapAllocator;
/// 服务实例 epoch。
pub use eneros_drv_svc::instance_epoch;
/// 单调毫秒。
pub use eneros_drv_svc::now_ms;

/// WP-75.2.3（R16-D）：business-services bin 族 panic handler 单源宏——
/// 声化（println 死因）+ 自旋。词形 `[<svc>] PANIC: {info}` 与 svc-ota
/// 先例（REG-70.7 排查：静默 spin 掩盖死因整排障周期）逐字一致；host
/// 测试构建展开为零（cfg 门）。八 bin 逐一 `svc_panic!("svc-x");` 收编，
/// 静默 handler 回流由 `bins_panic_handler_vocalized_single_source` 守卫。
#[macro_export]
macro_rules! svc_panic {
    ($name:expr) => {
        #[cfg(target_os = "none")]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            eneros_runtime::println!("[{}] PANIC: {}", $name, info);
            loop {
                core::hint::spin_loop();
            }
        }
    };
}

#[cfg(test)]
mod panic_guard_tests {
    /// WP-75.2.3 守卫（红测先行）：八 bin panic 声化单源——
    /// ① 每个 bin 须以 `svc_panic!("<自身名>")` 收编 handler；
    /// ② 自有 `#[panic_handler]` 定义零残留（单源宏内）。
    /// 现状（七 bin 静默）下红；宏收编后绿；任一 bin 改回静默即红。
    #[test]
    fn bins_panic_handler_vocalized_single_source() {
        const BINS: &[(&str, &str)] = &[
            ("svc-market", include_str!("bin/svc-market.rs")),
            ("svc-ledger", include_str!("bin/svc-ledger.rs")),
            ("svc-agent-bus", include_str!("bin/svc-agent-bus.rs")),
            ("svc-cloud-sync", include_str!("bin/svc-cloud-sync.rs")),
            ("svc-config", include_str!("bin/svc-config.rs")),
            ("svc-mgmt", include_str!("bin/svc-mgmt.rs")),
            ("svc-observability", include_str!("bin/svc-observability.rs")),
            ("svc-ota", include_str!("bin/svc-ota.rs")),
        ];
        let macro_open = concat!("svc_panic", "!(\"");
        let handler_attr = concat!("#", "[panic_handler]");
        for (name, src) in BINS {
            let want = format!("{macro_open}{name}\")");
            assert!(
                src.contains(&want),
                "{name} 未以 svc_panic!(\"{name}\") 收编 panic handler"
            );
            assert!(
                !src.contains(handler_attr),
                "{name} 残留自有 panic handler（WP-75.2.3 已单源入宏）"
            );
        }
    }
}
