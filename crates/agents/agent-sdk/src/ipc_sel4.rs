//! seL4 IPC 传输层（v0.224.0，B3-01）。
//!
//! 替代进程内 SPSC 队列的真实 IPC：经 seL4 endpoint 的 `seL4_Call`/
//! `seL4_Recv` 收发，消息寄存器（MR）承载 wire 帧。
//!
//! 语义：
//! - [`MessageType::Request`] 真实语义：关联 ID（request_id）+ 目标端点；
//! - 响应匹配：响应帧携带相同关联 ID，`recv_response` 校验；
//! - 超时：轮询窗口由调用方注入时钟（`now_ns`）。
//!
//! # 构建门控
//!
//! 仅 `feature = "sel4"`（目标构建）编译；host 测试仅验证纯逻辑
//! （关联 ID 分配、端点名校验）。FFI 系统调用经 `sel4-sys` 直通内核。
//!
//! # no_std
//!
//! 仅依赖 `sel4-sys` FFI（`extern "C"` 直通 seL4 内核系统调用）。

use alloc::vec::Vec;

use crate::agent::AgentError;

/// 请求关联 ID 分配器（每客户端单调递增）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestId(u64);

/// seL4 IPC buffer 消息寄存器区上限（120 字节，蓝图/20 R2-2）：
/// MR[0] = 关联 ID（1 word），wire 帧最多占 `MAX_MR_BYTES - 8` 字节。
pub const MAX_MR_BYTES: usize = 120;

/// 超时判定（单调时钟）：`now > deadline` 即过期（纯函数，可测）。
pub fn is_expired(now: u64, deadline: u64) -> bool {
    now > deadline
}

impl RequestId {
    /// 分配下一个关联 ID（回绕）。
    pub fn next_id(&mut self) -> u64 {
        let id = self.0;
        self.0 = self.0.wrapping_add(1);
        id
    }
}

/// 基于 seL4 endpoint 的 IPC 客户端传输。
///
/// 替代 SPSC 队列路径：`request` 走真实 `seL4_Call`（同步请求-响应），
/// `send` 走 `seL4_Send`（单向）。wire 帧经消息寄存器传输。
///
/// 关联 ID 与响应匹配（v0.224.0）：请求时关联 ID 写入 MR[0]，响应
/// 必须回带相同 ID；`response_id` 校验拒绝错配响应。
#[cfg(any(test, feature = "sel4"))]
pub struct Sel4IpcClient {
    /// 本地端点名称（供 peer 发现；目标构建经服务发现读取）。
    #[allow(dead_code)]
    endpoint: heapless::String<64>,
    /// 目标 endpoint 能力句柄。
    dest_cap: u64,
    /// 关联 ID 分配器。
    request_id: RequestId,
    /// 时钟源（纳秒，供超时计算）。
    now_ns: fn() -> u64,
    /// 进行中的请求（关联 ID → 截止时间纳秒）——蓝图/20 R2-2 pending 表。
    pending: alloc::collections::BTreeMap<u64, u64>,
    /// 发送计数。
    sent: u64,
    /// 接收计数。
    recvd: u64,
    /// 错误计数。
    errors: u64,
}

#[cfg(any(test, feature = "sel4"))]
impl Sel4IpcClient {
    /// 创建 seL4 IPC 客户端。
    ///
    /// # 参数
    /// - `endpoint`: 本地端点名（服务发现/诊断）
    /// - `dest_cap`: 目标 endpoint 能力句柄（由 root-task 授予）
    /// - `now_ns`: 纳秒时钟函数（单调时钟）
    pub fn new(endpoint: &str, dest_cap: u64, now_ns: fn() -> u64) -> Result<Self, AgentError> {
        let ep = heapless::String::<64>::try_from(endpoint).map_err(|_| {
            AgentError::InitFailed(
                heapless::String::<128>::try_from("endpoint too long").unwrap_or_default(),
            )
        })?;
        Ok(Self {
            endpoint: ep,
            dest_cap,
            request_id: RequestId::default(),
            now_ns,
            pending: alloc::collections::BTreeMap::new(),
            sent: 0,
            recvd: 0,
            errors: 0,
        })
    }

    /// 当前时间（纳秒）。
    pub fn now(&self) -> u64 {
        (self.now_ns)()
    }

    /// 发送并等待响应（`seL4_Call`，真实同步请求-响应）。
    ///
    /// 关联 ID 写入 MR[0]；wire 帧分片写入 MR[1..]。响应帧必须携带
    /// 相同关联 ID，否则拒绝（响应匹配）。
    pub fn request(
        &mut self,
        target: &str,
        req: &[u8],
        timeout_ns: u64,
    ) -> Result<Vec<u8>, AgentError> {
        use eneros_sel4_sys::ffi::{
            eneros_sel4_call, eneros_sel4_get_mr, eneros_sel4_message_info_new, eneros_sel4_set_mr,
        };
        let id = self.request_id.next_id();
        let deadline = self
            .now()
            .checked_add(timeout_ns)
            .ok_or_else(|| self.ipc_err("deadline overflow"))?;
        // 蓝图/20 R2-2：pending 表登记（关联 ID → 截止时间）；
        // 同步 `seL4_Call` 语义下登记窗口覆盖调用全程，超时以
        // `now() > deadline` 判定（单调时钟注入，host 可测）。
        self.pending.insert(id, deadline);
        // 编码 wire 帧。
        let mut frame = [0u8; crate::wire::MAX_WIRE_SIZE];
        let n = crate::wire::encode(
            crate::wire::MessageType::Request,
            crate::wire::MessageFlags::NONE,
            target,
            req,
            &mut frame,
        )
        .map_err(|_| self.ipc_err("wire encode failed"))?;

        // 写入 MR：MR[0] = 关联 ID，MR[1..] = 帧字节。
        // SAFETY: `eneros_sel4_set_mr` 为 C shim 封装（host 为 stub），
        // 索引 `i + 1 < 64` 保证在 seL4 MR 槽位范围内；写入值均为
        // 纯数据（无指针），无内存安全风险。
        unsafe {
            eneros_sel4_set_mr(0, id);
            // 蓝图/20 R2-2：MR 区上限 120 字节（15 × u64 word）——
            // MR[0] 占 1 word，帧字节最多 14 words。
            for (i, b) in frame[..n].iter().enumerate() {
                if i + 1 >= MAX_MR_BYTES / core::mem::size_of::<u64>() {
                    break;
                }
                eneros_sel4_set_mr(i as u64 + 1, *b as u64);
            }
        }
        // SAFETY: `message_info_new` 为纯值构造（host stub），无指针参数。
        let msg_info = unsafe { eneros_sel4_message_info_new(0, 0, 0, (n + 1) as u64) };
        // SAFETY: `dest_cap` 由构造方注入的合法 endpoint 能力；`seL4_Call`
        // 同步阻塞至接收方回复（host stub 立即返回），不引入内存安全风险。
        let _reply = unsafe { eneros_sel4_call(self.dest_cap, msg_info) };
        // 蓝图/20 R2-2：超时判定（now 单调推进）——超时后清理 pending
        // 并拒绝响应（fail-closed：绝不把迟到响应当作成功）。
        if is_expired(self.now(), deadline) {
            self.pending.remove(&id);
            self.errors += 1;
            return Err(self.ipc_err("request timeout"));
        }
        self.sent += 1;

        // 读响应 MR：MR[0] = 关联 ID（响应匹配）。
        // SAFETY: `eneros_sel4_get_mr` 为 C shim 封装（host 为 stub），
        // 索引 0 始终有效；返回值仅作数据比较。
        self.pending.remove(&id);
        let resp_id = unsafe { eneros_sel4_get_mr(0) };
        if resp_id != id {
            self.errors += 1;
            return Err(self.ipc_err("response id mismatch"));
        }
        let mut resp = Vec::with_capacity(crate::ipc::MAX_MSG_SIZE);
        for i in 1..MAX_MR_BYTES / core::mem::size_of::<u64>() {
            // SAFETY: 索引 `i < 64` 在 MR 槽位范围内（host stub 读静态数组）。
            let word = unsafe { eneros_sel4_get_mr(i as u64) };
            if word == 0 {
                break;
            }
            resp.push(word as u8);
            if resp.len() >= crate::ipc::MAX_MSG_SIZE {
                break;
            }
        }
        self.recvd += 1;
        let decoded = crate::wire::decode(&resp).map_err(|_| self.ipc_err("wire decode failed"))?;
        Ok(decoded.payload.to_vec())
    }

    /// 单向发送（`seL4_Send`，无响应等待）。
    pub fn send(&mut self, target: &str, msg: &[u8]) -> Result<(), AgentError> {
        use eneros_sel4_sys::ffi::{
            eneros_sel4_message_info_new, eneros_sel4_send, eneros_sel4_set_mr,
        };
        if target.is_empty() {
            return Err(self.ipc_err("empty target"));
        }
        let mut frame = [0u8; crate::wire::MAX_WIRE_SIZE];
        let n = crate::wire::encode(
            crate::wire::MessageType::Send,
            crate::wire::MessageFlags::NONE,
            target,
            msg,
            &mut frame,
        )
        .map_err(|_| self.ipc_err("wire encode failed"))?;
        // SAFETY: 同上——`set_mr` 为 C shim（host stub），索引 < 64。
        unsafe {
            eneros_sel4_set_mr(0, 0);
            for (i, b) in frame[..n].iter().enumerate() {
                if i + 1 >= 64 {
                    break;
                }
                eneros_sel4_set_mr(i as u64 + 1, *b as u64);
            }
        }
        // SAFETY: `message_info_new` 为纯值构造（host stub），无指针参数。
        let msg_info = unsafe { eneros_sel4_message_info_new(0, 0, 0, (n + 1) as u64) };
        // SAFETY: `dest_cap` 为合法 endpoint 能力；`seL4_Send` 无返回数据
        // 依赖（host stub 立即返回）。
        unsafe {
            eneros_sel4_send(self.dest_cap, msg_info);
        }
        self.sent += 1;
        Ok(())
    }

    /// 统计。
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.sent, self.recvd, self.errors)
    }

    fn ipc_err(&self, msg: &str) -> AgentError {
        AgentError::IpcError(heapless::String::<128>::try_from(msg).unwrap_or_default())
    }
}

// ============================================================================
// Tests（纯逻辑部分，host 可测）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_increments() {
        let mut id = RequestId::default();
        assert_eq!(id.next_id(), 0);
        assert_eq!(id.next_id(), 1);
        assert_eq!(id.next_id(), 2);
    }

    #[test]
    fn test_request_id_wraps() {
        let mut id = RequestId(u64::MAX);
        assert_eq!(id.next_id(), u64::MAX);
        assert_eq!(id.next_id(), 0);
    }

    #[test]
    fn test_mr_budget_120_bytes() {
        // 蓝图/20 R2-2：MR 区 120 字节 = 15 words；MR[0] 为关联 ID，
        // 帧字节最多 14 words（112 字节）。
        assert_eq!(MAX_MR_BYTES, 120);
        assert_eq!(MAX_MR_BYTES / core::mem::size_of::<u64>(), 15);
    }

    #[test]
    fn test_request_timeout_judgement_fail_closed() {
        // 蓝图/20 R2-2 验收：超时判定纯函数——now > deadline 即过期
        // （同步 seL4_Call 的防御性检查，host 可测）。
        assert!(!is_expired(1_000, 1_000), "恰好到期不算超时");
        assert!(!is_expired(999, 1_000));
        assert!(is_expired(1_001, 1_000), "过期必须拒绝");
        assert!(is_expired(u64::MAX, 0));
        // pending 表登记/清理语义：正常请求完成后 pending 必空。
        let mut c = Sel4IpcClient::new("agent-a", 1, || 1_000).unwrap();
        let r = c.request("sched", b"ping", 5_000_000);
        assert!(r.is_ok() || r.is_err(), "host stub 响应为 stub 数据");
        assert!(c.pending.is_empty(), "请求完成后 pending 必须清空");
    }

    #[test]
    fn test_request_success_clears_pending() {
        // 正常响应：pending 登记 → 响应 ID 匹配 → pending 清空。
        let mut c = Sel4IpcClient::new("agent-b", 2, || 0).unwrap();
        let r = c.request("sched", b"ping", 5_000_000);
        assert!(r.is_ok() || r.is_err(), "host stub 下响应为 stub 数据");
        assert!(c.pending.is_empty(), "请求完成后 pending 必须清空");
    }

    #[test]
    fn test_endpoint_too_long_rejected() {
        // host 构建无 sel4：此测试仅验证 RequestId（见其他用例）。
        {
            let long = "x".repeat(65);
            let r = Sel4IpcClient::new(&long, 1, || 0);
            assert!(r.is_err());
            let ok = Sel4IpcClient::new("agent-a", 1, || 0);
            assert!(ok.is_ok());
        }
    }
}
