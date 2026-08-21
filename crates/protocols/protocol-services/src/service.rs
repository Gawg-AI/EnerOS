//! 协议服务共享入口（蓝图 39 WP-39.12）。
//!
//! 每个 bin 的 `_start`：设置 IPC buffer TLS → 初始化分配器 → 构造
//! 对应 [`ProtocolPlugin`]（descriptor 报告）→ 进入通用 IPC loop。
//! 设备/传输 capability 未接通前（40 移交）→ `EXECUTE` 显式
//! `ST_UNAVAILABLE`；readiness 打印携带插件 descriptor（证据行）。

use eneros_drv_svc::runtime::{DriverRequest, DriverResponse, DriverService};
use eneros_protocol_abstract::plugin::{ProtocolPlugin, WorkBudget};

/// 未分配协议服务（capability None → EXECUTE 显式 ST_UNAVAILABLE——
/// 不伪造协议 I/O；设备句柄移交属蓝图 40）。
pub struct UnassignedProtocolService;

impl DriverService for UnassignedProtocolService {
    fn capability(&self) -> Option<&'static str> {
        None
    }
    fn handle(&mut self, _req: &DriverRequest, _resp: &mut DriverResponse) {}
}

/// 46（WP-46.2）：通用协议 I/O 服务包装——把装配成功的 [`ProtocolPlugin`]
/// 暴露为 [`DriverService`]：`EXECUTE` → `plugin.poll(now, WorkBudget)` 驱动
/// 真实协议 I/O（传输轮询/收发）；poll 失败 → `ST_UNAVAILABLE`（fail-closed，
/// 不伪造协议数据）。capability 形如 `net-<protocol>`（与 svc-modbus-tcp 的
/// `net-modbus-tcp` 同约定）。
pub struct PluginIoService<P: ProtocolPlugin> {
    plugin: P,
    capability: &'static str,
    /// 连续 poll 失败计数（观测）。
    pub err_count: u64,
}

impl<P: ProtocolPlugin> PluginIoService<P> {
    pub fn new(plugin: P, capability: &'static str) -> Self {
        Self {
            plugin,
            capability,
            err_count: 0,
        }
    }

    /// 暴露插件（诊断/测试）。
    pub fn plugin(&self) -> &P {
        &self.plugin
    }

    /// 47（WP-47.1 协议入站写）：导出插件已解析的写意图（服务层
    /// 转发给 svc-control-dispatch 前调用；缓冲清空）。
    pub fn drain_write_intents(
        &mut self,
        out: &mut alloc::vec::Vec<eneros_protocol_abstract::write::PointWriteIntent>,
    ) {
        self.plugin.drain_write_intents(out);
    }

    /// 46.5（L2 证据/「配置驱动运行」收口）：启动期自检轮询——对齐 45
    /// svc-modbus-tcp 的 `collect_and_report` 先例，进入 request 循环前
    /// **有界**驱动真实协议 I/O 多轮，使 post-connect 交换（104
    /// STARTDT_ACT/总召唤、mqtt CONNECT/SUB、raw 订阅 RX）在最早期完成。
    /// 异步状态机：本地短超时（max_work_ms 50），不阻塞等待对端；轮次
    /// 由调用方（svc bin）按插件语义自定。
    pub fn self_check_poll(&mut self, rounds: u32) {
        let budget = WorkBudget {
            max_work_ms: 50,
            max_polls: 8,
        };
        for i in 0..rounds {
            match self.plugin.poll(crate::now_ms(), budget) {
                Ok(_) => {}
                Err(_) => self.err_count = self.err_count.saturating_add(1),
            }
            // 46.5 诊断（QEMU 实弹排查，svc-iec104 自检窗口观测）：周期打印
            // 插件健康（状态机/错误），区分 Connected vs StartDtPending。
            if (i % 25) == 0 {
                let h = self.plugin.health();
                let dbg = self
                    .plugin
                    .debug_line()
                    .map(|s| alloc::format!(" dbg={s}"))
                    .unwrap_or_default();
                eneros_runtime::println!(
                    "[self-check] {} i={} now={} mono={} state={:?} progress={:?} errs={} last_err={:?}{}",
                    self.capability,
                    i,
                    crate::now_ms(),
                    eneros_time::get_monotonic_ns() / 1_000_000,
                    h.state,
                    h.last_progress,
                    h.error_count,
                    h.last_error,
                    dbg
                );
            }
            // 轮间有界自旋：QEMU TCG 下让异步栈时钟/对端 RTT 推进
            //（smoltcp TCP 重传计时、slirp 响应），~20ms×24 ≈ 0.5s。
            let t = eneros_time::get_monotonic_ns();
            while eneros_time::get_monotonic_ns().wrapping_sub(t) < 20_000_000 {
                core::hint::spin_loop();
            }
        }
    }
}

impl<P: ProtocolPlugin> DriverService for PluginIoService<P> {
    fn capability(&self) -> Option<&'static str> {
        Some(self.capability)
    }
    fn handle(&mut self, req: &DriverRequest, resp: &mut DriverResponse) {
        use eneros_drv_svc::runtime::{OP_EXECUTE, ST_INVALID, ST_OK, ST_UNAVAILABLE};
        if req.opcode == OP_EXECUTE {
            let budget = WorkBudget {
                max_work_ms: 50,
                max_polls: 8,
            };
            match self.plugin.poll(crate::now_ms(), budget) {
                Ok(_) => resp.status = ST_OK,
                Err(_) => {
                    self.err_count = self.err_count.saturating_add(1);
                    resp.status = ST_UNAVAILABLE;
                }
            }
        } else {
            resp.status = ST_INVALID;
        }
    }
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
