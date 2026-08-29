//! EnerOS 协议 EL0 服务（蓝图 39 WP-39.12）。
//!
//! 每个 ProductManifest 声明的协议以独立 EL0 service 入 bundle：
//! - `svc-modbus-rtu` / `svc-modbus-tcp` / `svc-iec104` / `svc-mqtt`；
//! - 复用 drv-svc 通用 IPC runtime（envelope/bounded queue/deadline/
//!   epoch——38 WP-38.9）；
//! - 服务装配时构造对应 [`ProtocolPlugin`]（descriptor 一并报告）；
//!   传输/设备 capability 随蓝图 40 设备句柄移交接通——未接通前
//!   `EXECUTE` 显式 `ST_UNAVAILABLE`（不伪造协议 I/O）。

#![no_std]

extern crate alloc;

/// 协议服务共享入口（`_start` 模板：IPC buffer + allocator + loop）。
pub mod service;

/// 46（WP-46.3）：协议配置拉取（svc-config GET_ENTRY → decode_protocol）。
/// EL0 target 构建启用 `sel4` feature（agent-sdk ipc_sel4）；host 不启用。
#[cfg(feature = "sel4")]
pub mod service_cfg;

/// 43（L2 协议互操作面）：协议服务网络传输（virtio-net + smoltcp +
/// TransportFactory seam——modbus-tcp 真实 TCP 传输接入点）。
pub mod net_transport;

/// 服务进程全局分配器（复用 drv-svc 的实现）。
pub use eneros_drv_svc::allocator::HeapAllocator;
/// 服务实例 epoch（重启后变化；客户端旧句柄失效）。
pub use eneros_drv_svc::instance_epoch;
/// 单调毫秒。
pub use eneros_drv_svc::now_ms;

/// SPEC-B WP-B.1：服务启动期墙钟注入（root 经 X6 下发 time_gate 闸门后的
/// Unix epoch 秒）。0 = 无可信时间——显式打印 `TIME-EPOCH-MISSING` 降级行
/// （绝不静默冒充可信时间）；非 0 → 写入 eneros_time 偏移面，此后服务内
/// `eneros_time::get_time()` 为真墙钟。
///
/// 必须在 `time_init(clock(), 0)` **之后**调用（time_init 会把偏移清零）；
/// 单调时钟面（`now_ms`/`now_ns`，退避与保活语义）不受影响——墙钟只经
/// `set_wall_epoch_secs` 显式注入，两条时间轴不混流。
#[cfg_attr(not(target_os = "none"), allow(unused_variables))]
pub fn apply_wall_epoch(service: &str, wall_epoch: u64) {
    #[cfg(target_os = "none")]
    match wall_epoch {
        0 => {
            eneros_runtime::println!("[{service}] TIME-EPOCH-MISSING");
        }
        secs => {
            eneros_time::set_wall_epoch_secs(secs);
            eneros_runtime::println!("[{service}] TIME-EPOCH={secs}");
        }
    }
    #[cfg(not(target_os = "none"))]
    {
        let _ = (service, wall_epoch);
    }
}

#[cfg(test)]
extern crate std;
