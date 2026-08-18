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

/// 43（L2 协议互操作面）：协议服务网络传输（virtio-net + smoltcp +
/// TransportFactory seam——modbus-tcp 真实 TCP 传输接入点）。
pub mod net_transport;

/// 服务进程全局分配器（复用 drv-svc 的实现）。
pub use eneros_drv_svc::allocator::HeapAllocator;
/// 服务实例 epoch（重启后变化；客户端旧句柄失效）。
pub use eneros_drv_svc::instance_epoch;
/// 单调毫秒。
pub use eneros_drv_svc::now_ms;

#[cfg(test)]
extern crate std;
