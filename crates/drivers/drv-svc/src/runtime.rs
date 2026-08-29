//! 38（WP-38.9）：驱动服务通用 IPC runtime——替代自旋占位。
//!
//! # 设计（蓝图 38 WP-38.9）
//! - versioned `DriverRequest/DriverResponse` envelope：correlation、opcode、
//!   deadline_ms、payload（有界）；
//! - bounded queue + backpressure（队满 → `ST_BUSY`，不无限积压）；
//! - cancellation/timeout：deadline 过期 → `ST_TIMEOUT`；迟到 completion
//!   不误归因——每个响应携带自己的 correlation + 服务 epoch；
//! - panic/restart 后 epoch 改变（服务启动时刻单调时间），客户端旧句柄
//!   经 [`is_stale_epoch`] 判定失效（`ST_STALE_HANDLE` 语义）；
//! - 服务只持自己设备 cap/DMA 域/IRQ（38：设备未分配 → capability None
//!   → `ST_UNAVAILABLE`，不伪造设备 I/O）；
//! - target service main 执行真实 IPC loop（[`driver_service_main`]），
//!   不再 `spin_loop()` 占位；
//! - no_std 无 alloc：定容结构 + 固定大小队列。

#![allow(clippy::empty_loop)]

/// 信封版本（wire ABI；不匹配 → `ST_INVALID`）。
pub const ENVELOPE_VERSION: u32 = 1;

/// 请求队列容量（bounded；队满 → 背压 `ST_BUSY`）。
pub const QUEUE_CAPACITY: usize = 8;

/// 操作码。
pub const OP_PING: u8 = 0x01;
pub const OP_GET_CAPABILITY: u8 = 0x02;
/// 设备操作（blk/net/storage 等具体命令由 39 定义；38 无设备 → 未分配）。
pub const OP_EXECUTE: u8 = 0x10;

/// 响应状态。
pub const ST_OK: u8 = 0;
/// 队列背压（bounded queue 满）。
pub const ST_BUSY: u8 = 1;
/// deadline 过期（取消；响应不执行）。
pub const ST_TIMEOUT: u8 = 2;
/// 客户端句柄 epoch 过期（服务已重启）。
pub const ST_STALE_HANDLE: u8 = 3;
/// 能力未分配（无设备/cap——显式降级，不伪造成功）。
pub const ST_UNAVAILABLE: u8 = 4;
/// 坏信封（版本/长度/未知 opcode）。
pub const ST_INVALID: u8 = 5;

/// 载荷容量（有界；设备命令 39 扩展）。
pub const PAYLOAD_CAP: usize = 64;

/// 驱动请求信封（wire 布局：version u32 | correlation u32 | opcode u8 |
/// deadline_ms u64 | payload_len u16 | payload）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverRequest {
    pub correlation: u32,
    pub opcode: u8,
    /// 单调毫秒 deadline（0 = 无 deadline——仍按有界处理）。
    pub deadline_ms: u64,
    pub payload: [u8; PAYLOAD_CAP],
    pub payload_len: u16,
}

/// 驱动响应信封（wire 布局：correlation u32 | status u8 | epoch u64 |
/// payload_len u16 | payload）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverResponse {
    pub correlation: u32,
    pub status: u8,
    /// 服务实例 epoch（重启后变化；客户端据此识别陈旧句柄）。
    pub epoch: u64,
    pub payload: [u8; PAYLOAD_CAP],
    pub payload_len: u16,
}

impl DriverRequest {
    pub const fn new(correlation: u32, opcode: u8) -> Self {
        Self {
            correlation,
            opcode,
            deadline_ms: 0,
            payload: [0; PAYLOAD_CAP],
            payload_len: 0,
        }
    }
}

impl DriverResponse {
    pub const fn new(correlation: u32, status: u8, epoch: u64) -> Self {
        Self {
            correlation,
            status,
            epoch,
            payload: [0; PAYLOAD_CAP],
            payload_len: 0,
        }
    }
}

/// 信封编解码（固定布局；长度越界/版本不符 → None）。
pub mod wire {
    use super::*;

    /// 信封线长（版本 1）。
    pub const REQUEST_LEN: usize = 4 + 4 + 1 + 8 + 2 + PAYLOAD_CAP;
    pub const RESPONSE_LEN: usize = 4 + 1 + 8 + 2 + PAYLOAD_CAP;

    pub fn encode_request(req: &DriverRequest) -> [u8; REQUEST_LEN] {
        let mut out = [0u8; REQUEST_LEN];
        let mut o = 0;
        for b in ENVELOPE_VERSION.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        for b in req.correlation.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        out[o] = req.opcode;
        o += 1;
        for b in req.deadline_ms.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        for b in req.payload_len.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        let n = core::cmp::min(req.payload_len as usize, PAYLOAD_CAP);
        out[o..o + n].copy_from_slice(&req.payload[..n]);
        out
    }

    pub fn decode_request(bytes: &[u8]) -> Option<DriverRequest> {
        if bytes.len() != REQUEST_LEN {
            return None;
        }
        let mut o = 0;
        let version = u32::from_le_bytes(bytes[o..o + 4].try_into().ok()?);
        o += 4;
        if version != ENVELOPE_VERSION {
            return None;
        }
        let correlation = u32::from_le_bytes(bytes[o..o + 4].try_into().ok()?);
        o += 4;
        let opcode = bytes[o];
        o += 1;
        let deadline_ms = u64::from_le_bytes(bytes[o..o + 8].try_into().ok()?);
        o += 8;
        let payload_len = u16::from_le_bytes(bytes[o..o + 2].try_into().ok()?);
        o += 2;
        let mut payload = [0u8; PAYLOAD_CAP];
        let n = core::cmp::min(payload_len as usize, PAYLOAD_CAP);
        payload[..n].copy_from_slice(&bytes[o..o + n]);
        Some(DriverRequest {
            correlation,
            opcode,
            deadline_ms,
            payload,
            payload_len,
        })
    }

    pub fn encode_response(resp: &DriverResponse) -> [u8; RESPONSE_LEN] {
        let mut out = [0u8; RESPONSE_LEN];
        let mut o = 0;
        for b in resp.correlation.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        out[o] = resp.status;
        o += 1;
        for b in resp.epoch.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        for b in resp.payload_len.to_le_bytes() {
            out[o] = b;
            o += 1;
        }
        let n = core::cmp::min(resp.payload_len as usize, PAYLOAD_CAP);
        out[o..o + n].copy_from_slice(&resp.payload[..n]);
        out
    }

    pub fn decode_response(bytes: &[u8]) -> Option<DriverResponse> {
        if bytes.len() != RESPONSE_LEN {
            return None;
        }
        let mut o = 0;
        let correlation = u32::from_le_bytes(bytes[o..o + 4].try_into().ok()?);
        o += 4;
        let status = bytes[o];
        o += 1;
        let epoch = u64::from_le_bytes(bytes[o..o + 8].try_into().ok()?);
        o += 8;
        let payload_len = u16::from_le_bytes(bytes[o..o + 2].try_into().ok()?);
        o += 2;
        let mut payload = [0u8; PAYLOAD_CAP];
        let n = core::cmp::min(payload_len as usize, PAYLOAD_CAP);
        payload[..n].copy_from_slice(&bytes[o..o + n]);
        Some(DriverResponse {
            correlation,
            status,
            epoch,
            payload,
            payload_len,
        })
    }

    /// QEMU 实测容差解码（单一事实源，SPEC-B WP-B.2）：drv-svc 服务端按
    /// 4B/word 回 MR，`RESPONSE_LEN=79` 会以 20 words（80B）返回；
    /// [`decode_response`] 要求精确 79B，直接解码误判 bad-envelope。
    /// 本函数接受尾部 padding（≥`RESPONSE_LEN`），只解前 `RESPONSE_LEN`
    /// 字节——历史病灶：agent-sdk block_io 与 svc-control-dispatch 通道
    /// 两处各自 inline（2026-08-28 体检），收编至此。
    pub fn decode_response_padded(bytes: &[u8]) -> Option<DriverResponse> {
        if bytes.len() < RESPONSE_LEN {
            return None;
        }
        decode_response(&bytes[..RESPONSE_LEN])
    }
}

/// 有界请求队列（固定容量；队满 → `ST_BUSY` 背压）。
pub struct BoundedQueue {
    entries: [Option<DriverRequest>; QUEUE_CAPACITY],
    head: usize,
    len: usize,
}

impl BoundedQueue {
    pub const fn new() -> Self {
        Self {
            entries: [None; QUEUE_CAPACITY],
            head: 0,
            len: 0,
        }
    }
    /// 入队；满 → Err（背压，调用方回 `ST_BUSY`）。
    pub fn push(&mut self, req: DriverRequest) -> Result<(), DriverRequest> {
        if self.len == QUEUE_CAPACITY {
            return Err(req);
        }
        let slot = (self.head + self.len) % QUEUE_CAPACITY;
        self.entries[slot] = Some(req);
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<DriverRequest> {
        if self.len == 0 {
            return None;
        }
        let item = self.entries[self.head].take();
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        self.len -= 1;
        item
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for BoundedQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 驱动服务（每服务实现；38 提供 console/rng 模板 + Unassigned）。
pub trait DriverService {
    /// 能力名（无设备 → None → 设备操作回 `ST_UNAVAILABLE`）。
    fn capability(&self) -> Option<&'static str>;
    /// 处理请求（`handle` 只写本请求的响应——迟到 completion 不误归因）。
    fn handle(&mut self, req: &DriverRequest, resp: &mut DriverResponse);
}

/// 请求处理入口（bounded queue + deadline + 状态归因；host 可测）。
///
/// 语义：
/// - 版本/长度坏 → `ST_INVALID`；
/// - 队列满 → `ST_BUSY`（背压，不无限积压）；
/// - deadline 过期（`now >= deadline`）→ `ST_TIMEOUT`（不执行）；
/// - capability 缺失且 opcode 为设备操作 → `ST_UNAVAILABLE`；
/// - 其余交给 [`DriverService::handle`]。
pub fn process_request(
    queue: &mut BoundedQueue,
    service: &mut dyn DriverService,
    epoch: u64,
    now: u64,
    raw: &[u8],
    out: &mut DriverResponse,
) -> bool {
    // 解码（坏信封 → INVALID）。
    let Some(req) = wire::decode_request(raw) else {
        *out = DriverResponse::new(0, ST_INVALID, epoch);
        return true;
    };
    out.correlation = req.correlation;
    out.epoch = epoch;
    // 队列背压。
    if queue.push(req).is_err() {
        *out = DriverResponse::new(out.correlation, ST_BUSY, epoch);
        return true;
    }
    let req = queue.pop().expect("push 成功后必有元素");
    // deadline 过期 → 取消（不执行）。
    if req.deadline_ms != 0 && now >= req.deadline_ms {
        *out = DriverResponse::new(out.correlation, ST_TIMEOUT, epoch);
        return true;
    }
    match req.opcode {
        OP_PING => {
            *out = DriverResponse::new(out.correlation, ST_OK, epoch);
        }
        OP_GET_CAPABILITY => {
            *out = DriverResponse::new(out.correlation, ST_OK, epoch);
            match service.capability() {
                Some(cap) => {
                    let n = core::cmp::min(cap.len(), PAYLOAD_CAP);
                    out.payload[..n].copy_from_slice(cap.as_bytes());
                    out.payload_len = n as u16;
                }
                None => {
                    out.payload_len = 0;
                }
            }
        }
        OP_EXECUTE => match service.capability() {
            // 设备未分配 → 显式 CapabilityUnavailable（不伪造 I/O）。
            None => {
                *out = DriverResponse::new(out.correlation, ST_UNAVAILABLE, epoch);
            }
            Some(_) => {
                service.handle(&req, out);
            }
        },
        _ => {
            *out = DriverResponse::new(out.correlation, ST_INVALID, epoch);
        }
    }
    true
}

/// 客户端陈旧句柄判定：服务重启后 epoch 变化——旧句柄必须失效。
pub fn is_stale_epoch(resp_epoch: u64, client_epoch: u64) -> bool {
    resp_epoch != client_epoch
}

/// 未分配服务（38：无设备 cap 的服务模板；blk/net/storage/fault 39 接入）。
pub struct UnassignedService;

impl DriverService for UnassignedService {
    fn capability(&self) -> Option<&'static str> {
        None
    }
    fn handle(&mut self, _req: &DriverRequest, _resp: &mut DriverResponse) {}
}

// ============================================================================
// target：真实 IPC loop（替代 spin_loop 占位；仿 services::svc_ipc_loop）
// ============================================================================

/// 服务 fail-closed 挂起。
#[cfg(target_os = "none")]
fn svc_fail_closed(step: &str, e: impl core::fmt::Debug) -> ! {
    eneros_runtime::println!(
        "[drv-svc] IPC loop FAILED at {}: {:?} — 挂起（fail-closed）",
        step,
        e
    );
    loop {
        core::hint::spin_loop();
    }
}

/// 驱动服务 IPC 主循环（WP-38.9：真实 IPC loop，非自旋占位）。
///
/// 循环：`ipc_recv` 收请求 → `process_request`（bounded queue + deadline +
/// epoch + capability 归因）→ 编码响应信封入 MR → `ipc_reply_recv` 回复。
/// 任何 IPC/解码错误 → fail-closed 挂起（红线 4 精神）。
///
/// `now`：单调毫秒时钟（服务侧由 `eneros_time` 提供；测试注入）。
/// `epoch`：服务实例启动时刻（重启后变化——旧客户端句柄失效）。
#[cfg(target_os = "none")]
pub fn driver_service_main(
    ep: u64,
    name: &'static str,
    ready_ntfn: u64,
    now: fn() -> u64,
    epoch: u64,
    service: &mut dyn DriverService,
) -> ! {
    use eneros_sel4_sys::safe::{get_mr, ipc_recv, ipc_reply_recv, message_info_new, set_mr};
    use eneros_sel4_sys::types::message_info_length;

    let mut queue = BoundedQueue::new();
    let mut sender = 0u64;
    let mut reply_info = 0u64;
    eneros_runtime::println!(
        "[drv-svc-{}] ipc loop entered ep={} epoch={} cap={:?}",
        name,
        ep,
        epoch,
        service.capability()
    );
    if let Err(e) = eneros_sel4_sys::safe::ipc_signal(ready_ntfn) {
        svc_fail_closed("ready signal", e);
    }
    eneros_runtime::println!("[drv-svc-{}] READY-SIGNALED ntfn_slot={}", name, ready_ntfn);

    loop {
        let mi = {
            let r = if reply_info == 0 {
                ipc_recv(ep, Some(&mut sender))
            } else {
                ipc_reply_recv(ep, reply_info, Some(&mut sender))
            };
            match r {
                Ok(m) => m,
                Err(e) => svc_fail_closed("recv", e),
            }
        };
        // 帧载荷 MR → 信封字节（有界：MR 数受 seL4 消息上限约束）。
        // 41 存储句柄移交修复：REQUEST_LEN=83 非 4 倍数——`/4` 截断到
        // 20 words=80B，decode_request 要求 83B 必失败（QEMU 实弹
        // status=5）。改 div_ceil（21 words=84B ≥ 83）。
        let words = message_info_length(mi);
        let mut raw = [0u8; wire::REQUEST_LEN];
        let mut filled = 0usize;
        for i in 0..core::cmp::min(words as usize, wire::REQUEST_LEN.div_ceil(4)) {
            match get_mr(i as u64) {
                Ok(w) => {
                    // 越界保护：最后一 word 可能超出 REQUEST_LEN（83 非
                    // 4 倍数）——只写剩余字节，不越界（raw 数组边界）。
                    let remaining = raw.len().saturating_sub(filled);
                    if remaining == 0 {
                        break;
                    }
                    let n = core::cmp::min(4, remaining);
                    for k in 0..n {
                        raw[filled + k] = ((w >> (8 * k)) & 0xff) as u8;
                    }
                    filled += n;
                }
                Err(e) => svc_fail_closed("get_mr", e),
            }
        }
        // 处理（bounded queue + deadline + epoch + capability）。
        let mut resp = DriverResponse::new(0, ST_INVALID, epoch);
        process_request(&mut queue, service, epoch, now(), &raw[..filled], &mut resp);
        eneros_runtime::println!(
            "[drv-svc-{}] IPC: corr={} op={} status={}",
            name,
            resp.correlation,
            raw[8],
            resp.status
        );
        // 编码响应 → MR → reply。
        let enc = wire::encode_response(&resp);
        let mut mrs: [u64; (wire::RESPONSE_LEN + 3) / 4] = [0; (wire::RESPONSE_LEN + 3) / 4];
        for (i, chunk) in enc.chunks_exact(4).enumerate() {
            let w = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            mrs[i] = u64::from(w);
        }
        for (i, w) in mrs.iter().enumerate() {
            if let Err(e) = set_mr(i as u64, *w) {
                svc_fail_closed("set_mr", e);
            }
        }
        reply_info = match message_info_new(0, 0, 0, mrs.len() as u64) {
            Ok(m) => m,
            Err(e) => svc_fail_closed("message_info_new", e),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    fn req_bytes(req: &DriverRequest) -> Vec<u8> {
        wire::encode_request(req).to_vec()
    }

    #[test]
    fn envelope_roundtrip() {
        let mut req = DriverRequest::new(0x1234, OP_EXECUTE);
        req.deadline_ms = 999;
        req.payload[..3].copy_from_slice(b"abc");
        req.payload_len = 3;
        let bytes = wire::encode_request(&req);
        let dec = wire::decode_request(&bytes).unwrap();
        assert_eq!(dec.correlation, 0x1234);
        assert_eq!(dec.opcode, OP_EXECUTE);
        assert_eq!(dec.deadline_ms, 999);
        assert_eq!(&dec.payload[..3], b"abc");
        // 响应 roundtrip
        let mut resp = DriverResponse::new(0x1234, ST_OK, 42);
        resp.payload[..2].copy_from_slice(b"ok");
        resp.payload_len = 2;
        let rb = wire::encode_response(&resp);
        let rdec = wire::decode_response(&rb).unwrap();
        assert_eq!(rdec.status, ST_OK);
        assert_eq!(rdec.epoch, 42);
        assert_eq!(&rdec.payload[..2], b"ok");
    }

    #[test]
    fn bad_version_and_len_rejected() {
        let mut bytes = wire::encode_request(&DriverRequest::new(1, OP_PING)).to_vec();
        bytes[0] = 99; // 版本坏
        assert!(wire::decode_request(&bytes).is_none());
        assert!(wire::decode_request(&bytes[..10]).is_none(), "长度不足拒绝");
    }

    #[test]
    fn bounded_queue_backpressure() {
        let mut q = BoundedQueue::new();
        for i in 0..QUEUE_CAPACITY {
            assert!(q.push(DriverRequest::new(i as u32, OP_PING)).is_ok());
        }
        // 队满 → 背压（Err 返回原请求）。
        let overflow = DriverRequest::new(99, OP_PING);
        assert_eq!(q.push(overflow), Err(overflow));
        // 出队恢复。
        let first = q.pop().unwrap();
        assert_eq!(first.correlation, 0);
        assert!(q.push(DriverRequest::new(100, OP_PING)).is_ok());
    }

    #[test]
    fn process_ok_and_busy() {
        let mut q = BoundedQueue::new();
        let mut service = UnassignedService;
        let mut out = DriverResponse::new(0, 0, 0);
        // PING → OK
        process_request(
            &mut q,
            &mut service,
            7,
            100,
            &req_bytes(&DriverRequest::new(1, OP_PING)),
            &mut out,
        );
        assert_eq!((out.status, out.epoch, out.correlation), (ST_OK, 7, 1));
        // GET_CAPABILITY（无设备）→ OK + 空载荷
        process_request(
            &mut q,
            &mut service,
            7,
            100,
            &req_bytes(&DriverRequest::new(2, OP_GET_CAPABILITY)),
            &mut out,
        );
        assert_eq!(out.status, ST_OK);
        assert_eq!(out.payload_len, 0);
        // EXECUTE 无设备 → UNAVAILABLE（不伪造）
        process_request(
            &mut q,
            &mut service,
            7,
            100,
            &req_bytes(&DriverRequest::new(3, OP_EXECUTE)),
            &mut out,
        );
        assert_eq!(out.status, ST_UNAVAILABLE);
    }

    #[test]
    fn deadline_expiry_cancels() {
        let mut q = BoundedQueue::new();
        let mut service = UnassignedService;
        let mut out = DriverResponse::new(0, 0, 0);
        let mut req = DriverRequest::new(4, OP_PING);
        req.deadline_ms = 50;
        // now=100 >= deadline=50 → TIMEOUT（不执行）。
        process_request(&mut q, &mut service, 1, 100, &req_bytes(&req), &mut out);
        assert_eq!(out.status, ST_TIMEOUT);
        // now=10 < deadline=50 → 执行。
        process_request(&mut q, &mut service, 1, 10, &req_bytes(&req), &mut out);
        assert_eq!(out.status, ST_OK);
    }

    #[test]
    fn stale_epoch_detection() {
        // 服务重启：epoch 7 → 9；客户端旧句柄必须失效。
        assert!(is_stale_epoch(9, 7));
        assert!(!is_stale_epoch(7, 7));
    }

    #[test]
    fn unknown_opcode_invalid() {
        let mut q = BoundedQueue::new();
        let mut service = UnassignedService;
        let mut out = DriverResponse::new(0, 0, 0);
        process_request(
            &mut q,
            &mut service,
            1,
            10,
            &req_bytes(&DriverRequest::new(5, 0x7f)),
            &mut out,
        );
        assert_eq!(out.status, ST_INVALID);
    }

    /// console 模板服务（38：capability 声明 console；设备在 39 分配——
    /// 无设备时 EXECUTE → UNAVAILABLE 由 process_request 保证）。
    struct ConsoleService;
    impl DriverService for ConsoleService {
        fn capability(&self) -> Option<&'static str> {
            Some("console")
        }
        fn handle(&mut self, req: &DriverRequest, resp: &mut DriverResponse) {
            // 回声模板：把请求载荷原样回写（38 模板语义；真机 console
            // 收发在 39 设备分配后接线）。
            let n = core::cmp::min(req.payload_len as usize, PAYLOAD_CAP);
            resp.payload[..n].copy_from_slice(&req.payload[..n]);
            resp.payload_len = n as u16;
            resp.status = ST_OK;
        }
    }

    #[test]
    fn console_service_template() {
        let mut q = BoundedQueue::new();
        let mut svc = ConsoleService;
        let mut out = DriverResponse::new(0, 0, 0);
        // 能力查询。
        process_request(
            &mut q,
            &mut svc,
            3,
            10,
            &req_bytes(&DriverRequest::new(1, OP_GET_CAPABILITY)),
            &mut out,
        );
        assert_eq!(out.status, ST_OK);
        assert_eq!(&out.payload[..out.payload_len as usize], b"console");
        // EXECUTE（有 capability 模板）→ handle 回声。
        let mut req = DriverRequest::new(2, OP_EXECUTE);
        req.payload[..4].copy_from_slice(b"echo");
        req.payload_len = 4;
        process_request(&mut q, &mut svc, 3, 10, &req_bytes(&req), &mut out);
        assert_eq!(out.status, ST_OK);
        assert_eq!(&out.payload[..4], b"echo");
    }
}
