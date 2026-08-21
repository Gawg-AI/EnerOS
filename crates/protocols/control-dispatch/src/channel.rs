//! 蓝图 40 WP-40.8：生产 `DeliveryChannel`。
//!
//! 投递通道抽象（唯一投递 seam）：`ControlDispatch` 经
//! [`DeliveryChannel::deliver`] 把签名 `ControlEnvelope` 送到 device-agent
//! 服务并取回结构化回执。
//!
//! - 生产 impl：[`Sel4EipcChannel`]（seL4 EIPC，基于 agent-sdk
//!   `Sel4IpcClient` + 分块传输 [`eneros_agent_sdk::ChunkedClient`]——
//!   `ControlEnvelope` 270B 超出 seL4 MR 120B 预算，必须分块）；
//! - 测试 impl 仅 `#[cfg(test)]`（绝不进入 release resolved graph）。

#[cfg(feature = "sel4")]
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 投递通道错误（fail-closed：任何异常都是明确错误，不猜成功）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// 目标服务不可达（endpoint stale/未装配）。
    Unreachable,
    /// 超时（deadline 内无回执）。
    Timeout,
    /// 响应畸形（长度/格式）。
    BadResponse,
    /// 传输协议违例。
    Protocol(String),
    /// 服务端拒绝（ack 不符）。
    Rejected,
    /// 客户端被取消。
    Cancelled,
}

/// 投递通道 trait（唯一投递 seam；蓝图 40 WP-40.8）。
///
/// `target` = 服务端点标识（EIPC：服务名经装配映射 cap）；`payload` =
/// 编码后的 `ControlEnvelope`；`timeout_ms` = 端到端超时。返回服务端
/// 结构化回执字节（`OutcomeFrame`）。
pub trait DeliveryChannel {
    /// 投递并等待回执。
    fn deliver(
        &mut self,
        target: &str,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, ChannelError>;
}

/// seL4 EIPC 生产通道（WP-40.8 的"真实 seL4 EIPC impl"）。
///
/// 基于 agent-sdk `Sel4IpcClient`（correlation/deadline 检查）与分块
/// 传输协议；持有最小 endpoint cap（装配期注入 `dest_cap`）。
#[cfg(feature = "sel4")]
pub struct Sel4EipcChannel {
    client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
    /// 发送方 epoch（重启计数；防迟到消息）。
    epoch: u64,
    /// 单次投递超时（ms）。
    timeout_ms: u64,
}

#[cfg(feature = "sel4")]
impl Sel4EipcChannel {
    /// 构造（`dest_cap` = device-agent endpoint cap；`now_ns` = 单调时钟）。
    pub fn new(
        endpoint: &str,
        dest_cap: u64,
        epoch: u64,
        timeout_ms: u64,
        now_ns: fn() -> u64,
    ) -> Result<Self, eneros_agent_sdk::AgentError> {
        Ok(Self {
            client: eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(endpoint, dest_cap, now_ns)?,
            epoch,
            timeout_ms,
        })
    }
}

#[cfg(feature = "sel4")]
impl DeliveryChannel for Sel4EipcChannel {
    fn deliver(
        &mut self,
        target: &str,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, ChannelError> {
        let timeout = if timeout_ms == 0 {
            self.timeout_ms
        } else {
            timeout_ms
        };
        let mut transfer =
            eneros_agent_sdk::ChunkedClient::new(&mut self.client, self.epoch, timeout);
        self.epoch = self.epoch.saturating_add(1);
        transfer
            .transfer(target, payload)
            .map_err(|e| ChannelError::Protocol(format!("chunked: {e:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试通道：内存回执（仅测试；release resolved graph 不可达）。
    pub struct EchoChannel {
        pub outcome: Vec<u8>,
        pub received: Vec<Vec<u8>>,
        pub fail: Option<ChannelError>,
    }

    impl EchoChannel {
        pub fn new(outcome: Vec<u8>) -> Self {
            Self {
                outcome,
                received: Vec::new(),
                fail: None,
            }
        }
    }

    impl DeliveryChannel for EchoChannel {
        fn deliver(
            &mut self,
            _target: &str,
            payload: &[u8],
            _timeout_ms: u64,
        ) -> Result<Vec<u8>, ChannelError> {
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.received.push(payload.to_vec());
            Ok(self.outcome.clone())
        }
    }

    #[test]
    fn echo_channel_roundtrips_payload() {
        let mut ch = EchoChannel::new(vec![0, 0]);
        let out = ch
            .deliver("svc-device-agent", &[1, 2, 3], 1000)
            .expect("deliver");
        assert_eq!(out, vec![0, 0]);
        assert_eq!(ch.received.len(), 1);
        assert_eq!(ch.received[0], vec![1, 2, 3]);
    }

    #[test]
    fn echo_channel_propagates_failure() {
        let mut ch = EchoChannel::new(vec![]);
        ch.fail = Some(ChannelError::Timeout);
        assert_eq!(ch.deliver("t", &[1], 10), Err(ChannelError::Timeout));
    }
}
