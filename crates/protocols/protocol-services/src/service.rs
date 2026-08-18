//! 协议服务共享入口（蓝图 39 WP-39.12）。
//!
//! 每个 bin 的 `_start`：设置 IPC buffer TLS → 初始化分配器 → 构造
//! 对应 [`ProtocolPlugin`]（descriptor 报告）→ 进入通用 IPC loop。
//! 设备/传输 capability 未接通前（40 移交）→ `EXECUTE` 显式
//! `ST_UNAVAILABLE`；readiness 打印携带插件 descriptor（证据行）。

use eneros_drv_svc::runtime::{DriverRequest, DriverResponse, DriverService};

/// 未分配协议服务（capability None → EXECUTE 显式 ST_UNAVAILABLE——
/// 不伪造协议 I/O；设备句柄移交属蓝图 40）。
pub struct UnassignedProtocolService;

impl DriverService for UnassignedProtocolService {
    fn capability(&self) -> Option<&'static str> {
        None
    }
    fn handle(&mut self, _req: &DriverRequest, _resp: &mut DriverResponse) {}
}

#[cfg(test)]
mod tests {
    use eneros_drv_svc::runtime::{
        process_request, wire, BoundedQueue, DriverRequest, DriverResponse, OP_EXECUTE, OP_PING,
        ST_OK, ST_UNAVAILABLE,
    };

    use super::*;

    /// 39 收尾复验：未分配协议服务 EXECUTE → ST_UNAVAILABLE（不伪造
    /// 协议 I/O；设备句柄移交属蓝图 40），PING 正常。
    #[test]
    fn unassigned_execute_unavailable_ping_ok() {
        let mut svc = UnassignedProtocolService;
        assert!(svc.capability().is_none());
        let mut queue = BoundedQueue::new();

        // EXECUTE → ST_UNAVAILABLE。
        let req = DriverRequest {
            correlation: 7,
            opcode: OP_EXECUTE,
            deadline_ms: 0,
            payload: [0u8; eneros_drv_svc::runtime::PAYLOAD_CAP],
            payload_len: 0,
        };
        let raw = wire::encode_request(&req);
        let mut out = DriverResponse::new(0, 255, 9);
        assert!(process_request(
            &mut queue, &mut svc, 9, 100, &raw, &mut out
        ));
        assert_eq!(out.status, ST_UNAVAILABLE);
        assert_eq!(out.correlation, 7);

        // PING → ST_OK（服务进程存活与设备无关）。
        let ping = DriverRequest {
            correlation: 8,
            opcode: OP_PING,
            deadline_ms: 0,
            payload: [0u8; eneros_drv_svc::runtime::PAYLOAD_CAP],
            payload_len: 0,
        };
        let raw = wire::encode_request(&ping);
        let mut out = DriverResponse::new(0, 255, 9);
        assert!(process_request(
            &mut queue, &mut svc, 9, 100, &raw, &mut out
        ));
        assert_eq!(out.status, ST_OK);
    }
}
