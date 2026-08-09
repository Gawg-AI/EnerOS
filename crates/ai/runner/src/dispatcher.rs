//! 信封投递器（蓝图/24 §6.3/6.4：`DeviceExecutor` 的生产实现）。
//!
//! [`EnvelopeDispatcher`] 把 [`AuthorizedCommand`] + 签名信封 + 功率载荷经
//! EIPC 通道投递给 device-agent，等待结构化结果：
//!
//! ```text
//! 成功（Applied/IdempotentHit/DryRun）        → Ok(())
//! 复验拒绝（Denied{class}）                    → Err(Rejected(DeviceReject))
//! 握手失败（有界重试 ≤5 后仍不可用）            → Err(Offline)（fail-closed）
//! 通道/设备不可达                             → Err(Comm)
//! ```
//!
//! 降级行为原则（§6.3）：握手不可用 = 写不可用。任何"绕过握手"的降级
//! 都是对本规格的违反——`write` 在 token 缺失时经握手获取，失败即
//! `Offline`，绝不无 token 直投。

use eneros_control_gateway::gateway::AuthorizedCommand;
use eneros_device_agent::{
    verify_handshake_response, DeliveryFrame, OutcomeFrame, HANDSHAKE_MARKER,
};
use eneros_dual_brain::chain_sink::{DeviceExecError, DeviceExecutor, DeviceReject};

/// EIPC 投递通道抽象（候选 3 落地时迁移点单一——R1：投递器封装为 adapter）。
///
/// 生产实现对接 agent-sdk EIPC（`wire.rs` 帧编解码 + 通道收发）；测试用
/// 进程内通道直连 device-agent 收口点。
pub trait DeliveryChannel {
    /// 发送请求载荷并等待响应。`target` 为目标服务名（EIPC target）。
    /// `Err` = 通道/设备不可达或响应异常。
    fn request(&mut self, target: &str, payload: &[u8]) -> Result<Vec<u8>, ChannelError>;
}

/// 通道错误（不可达/超时/响应异常）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// 通道或目标服务不可达。
    Unreachable,
    /// 请求超时。
    Timeout,
    /// 响应异常（长度/格式不符）。
    BadResponse,
}

/// 握手客户端（§6.3）：有界重试（指数退避 ≤5 次），失败 fail-closed。
pub struct EnvelopeDispatcher {
    channel: Box<dyn DeliveryChannel>,
    /// 握手响应验证密钥（与 device-agent 同源，装配注入）。
    key: Vec<u8>,
    /// 本地租约 ID（两侧同源）。
    lease_id: [u8; 16],
    /// 已获取的 fencing token（首次写经握手获取）。
    token: Option<u64>,
    /// 握手有界重试上限（≤5，§6.3）。
    handshake_retries: u32,
    /// 退避函数（毫秒；测试注入 0 延迟）。
    backoff: fn(u32) -> u64,
}

impl EnvelopeDispatcher {
    /// 构造投递器。`key`/`lease_id` 与 device-agent 侧同源（装配注入，§2.6）。
    pub fn new(
        channel: Box<dyn DeliveryChannel>,
        key: &[u8],
        lease_id: [u8; 16],
        handshake_retries: u32,
    ) -> Self {
        Self {
            channel,
            key: key.to_vec(),
            lease_id,
            token: None,
            handshake_retries: handshake_retries.min(5),
            backoff: default_backoff,
        }
    }

    /// 注入退避函数（测试用）。
    pub fn with_backoff(mut self, f: fn(u32) -> u64) -> Self {
        self.backoff = f;
        self
    }

    /// 启动握手（§6.3 时序）：经 EIPC 发握手请求 → 验证 HMAC 响应 →
    /// 取 `{lease_id, fencing_token}`。有界重试（指数退避 ≤5 次）；
    /// 仍失败 → `Offline`（fail-closed，禁止绕过握手的降级）。
    pub fn handshake(&mut self) -> Result<u64, DeviceExecError> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match self.channel.request("device-agent", &HANDSHAKE_MARKER) {
                Ok(resp) => {
                    let (lease_id, token) = verify_handshake_response(&self.key, &resp).ok_or(
                        DeviceExecError::Comm(String::from("handshake hmac verify failed")),
                    )?;
                    if lease_id != self.lease_id {
                        return Err(DeviceExecError::Comm(String::from(
                            "handshake lease_id mismatch",
                        )));
                    }
                    self.token = Some(token);
                    return Ok(token);
                }
                Err(e) => {
                    if attempt >= self.handshake_retries {
                        // 握手不可用 = 写不可用（§6.3 降级行为原则）。
                        return Err(DeviceExecError::Offline);
                    }
                    let delay = (self.backoff)(attempt);
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                    let _ = e;
                }
            }
        }
    }

    /// 当前 fencing token（`None` = 尚未握手）。
    pub fn token(&self) -> Option<u64> {
        self.token
    }
}

impl DeviceExecutor for EnvelopeDispatcher {
    fn write(
        &mut self,
        _cmd: &AuthorizedCommand,
        envelope: &[u8],
        power_kw: f64,
    ) -> Result<(), DeviceExecError> {
        // 1. 首次写前完成握手（有界重试 ≤5；失败 fail-closed → Offline）。
        if self.token.is_none() {
            self.handshake()?;
        }
        // 2. 构造投递帧（签名信封 + 功率载荷，180B 定长）并投递。
        let mut env_bytes = [0u8; 172];
        if envelope.len() != env_bytes.len() {
            return Err(DeviceExecError::Comm(String::from("bad envelope length")));
        }
        env_bytes.copy_from_slice(envelope);
        let payload = DeliveryFrame {
            envelope: env_bytes,
            power_kw,
        }
        .encode();
        let resp = self
            .channel
            .request("device-agent", &payload)
            .map_err(|e| DeviceExecError::Comm(format!("delivery channel: {e:?}")))?;
        // 3. 结果映射（§6.4）：成功/幂等/干跑 → Ok；复验拒绝 → 结构化原因。
        let frame = OutcomeFrame::decode(&resp)
            .ok_or(DeviceExecError::Comm(String::from("bad outcome frame")))?;
        match frame.code {
            OutcomeFrame::CODE_APPLIED
            | OutcomeFrame::CODE_IDEMPOTENT_HIT
            | OutcomeFrame::CODE_DRY_RUN => Ok(()),
            OutcomeFrame::CODE_DENIED => Err(DeviceExecError::Rejected(DeviceReject::from_code(
                frame.class,
            ))),
            _ => Err(DeviceExecError::Comm(format!(
                "unknown outcome code {}",
                frame.code
            ))),
        }
    }
}

/// 默认指数退避（毫秒）：1s、2s、4s、8s（attempt=1 起）。
fn default_backoff(attempt: u32) -> u64 {
    (1u64 << (attempt.saturating_sub(1).min(3))) * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 通道错误注入（测试；Rc 计数）。
    struct FailChannel {
        calls: std::rc::Rc<std::cell::Cell<u32>>,
    }
    impl DeliveryChannel for FailChannel {
        fn request(&mut self, _target: &str, _payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
            self.calls.set(self.calls.get() + 1);
            Err(ChannelError::Unreachable)
        }
    }

    /// 有界重试 ≤5 且失败 fail-closed 报 Offline（§6.3 降级行为原则）。
    #[test]
    fn handshake_bounded_retries_then_offline() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let ch = FailChannel {
            calls: calls.clone(),
        };
        let mut d = EnvelopeDispatcher::new(Box::new(ch), b"k", [0u8; 16], 5).with_backoff(|_| 0);
        let r = d.handshake();
        assert_eq!(r, Err(DeviceExecError::Offline));
        assert_eq!(calls.get(), 5, "必须恰好重试 5 次（有界）");
    }

    /// 禁止绕过握手：token 缺失时 write 必须先握手，失败 → Offline 而非直投。
    #[test]
    fn write_fails_closed_without_handshake() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let ch = FailChannel {
            calls: calls.clone(),
        };
        let mut d = EnvelopeDispatcher::new(Box::new(ch), b"k", [0u8; 16], 5).with_backoff(|_| 0);
        let cmd = test_cmd();
        let r = d.write(&cmd, &[0u8; 172], 50.0);
        // 握手不可用 = 写不可用：禁止无 token 直投（§6.3）。
        assert_eq!(r, Err(DeviceExecError::Offline));
        assert_eq!(calls.get(), 5, "全部重试都花在握手，未发生直投");
    }

    fn test_cmd() -> AuthorizedCommand {
        use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
        AuthorizedCommand {
            device_id: 1,
            point_id: 10,
            command: ControlCommand::Single(SingleCommand::On),
            actor_kind: eneros_telemetry_model::command::ActorKind::Ai,
            issuer: [0u8; 16],
            correlation_id: [0u8; 16],
            sequence: 1,
            lease_id: [0u8; 16],
            fencing_token: 1,
            policy_version: 1,
            idempotency_key: [0u8; 16],
        }
    }
}
