//! 信封投递帧编解码（蓝图/24 §6.2/6.4：AI→device-agent 的 EIPC 载荷契约）。
//!
//! 投递帧 = 签名信封（172 字节，control-gateway envelope 布局）+ 功率载荷
//! （f64 LE，8 字节）= 180 字节定长。经 agent-sdk EIPC wire 传输（帧层
//! magic/CRC 归 agent-sdk `wire.rs` 管辖）；本模块负责语义载荷层的编解码，
//! 签发方（ai/runner 投递器）与复验方（device-agent ingress）同源消费。
//!
//! 功率载荷语义（ADR-029）：`ControlCommand` 为开关语义（Single On/Off），
//! `power_kw` 作为**执行载荷**传给 `write_authorized`——ControlChain 负责
//! 授权，适配器负责落写含功率。

use eneros_control_gateway::envelope::ENVELOPE_LEN;

/// 投递帧总长度（信封 172 + 功率 8）。
pub const DELIVERY_LEN: usize = ENVELOPE_LEN + 8;

/// 投递帧解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 载荷长度不为 [`DELIVERY_LEN`]。
    BadLength { got: usize },
}

/// 信封投递帧：签名信封字节 + 功率设定（kW）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeliveryFrame {
    /// 签名信封（172 字节，control-gateway 布局）。
    pub envelope: [u8; ENVELOPE_LEN],
    /// 功率设定值（kW，f64 LE 编码）。
    pub power_kw: f64,
}

impl DeliveryFrame {
    /// 编码为 180 字节定长载荷（EIPC payload）。
    pub fn encode(&self) -> [u8; DELIVERY_LEN] {
        let mut out = [0u8; DELIVERY_LEN];
        out[..ENVELOPE_LEN].copy_from_slice(&self.envelope);
        out[ENVELOPE_LEN..].copy_from_slice(&self.power_kw.to_le_bytes());
        out
    }

    /// 解码 180 字节定长载荷；长度不符即拒绝（default-deny，不喂给分配器）。
    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        if buf.len() != DELIVERY_LEN {
            return Err(FrameError::BadLength { got: buf.len() });
        }
        let mut envelope = [0u8; ENVELOPE_LEN];
        envelope.copy_from_slice(&buf[..ENVELOPE_LEN]);
        let mut power = [0u8; 8];
        power.copy_from_slice(&buf[ENVELOPE_LEN..]);
        Ok(Self {
            envelope,
            power_kw: f64::from_le_bytes(power),
        })
    }
}

/// 握手请求标记（EIPC 载荷单字节，与 180B 投递帧可区分；device-agent 侧
/// EIPC handler 依此路由到 [`crate::ingress::EnvelopeIngress::handshake_response`]）。
pub const HANDSHAKE_MARKER: [u8; 1] = [0x00];

// 蓝图 40 WP-40.11：结果帧统一收敛到 control-gateway 的 `wire` 模块
//（唯一线上编解码——dispatch 与 device-agent 同源消费）。旧本地定义
//（2 字节 code/class）由 `OutcomeFrame`/`OutcomeCode` 重导出兼容；
// 结果码扩展：4=AppliedReadbackUnavailable、5=AppliedReadbackMismatch、
// 6=Failed（分层成功语义）。
pub use eneros_control_gateway::wire::OutcomeFrame;

/// 兼容常量（旧 CODE_* 名称保留；新代码见 [`OutcomeCode`]）。
pub const CODE_APPLIED: u8 = 0;
pub const CODE_IDEMPOTENT_HIT: u8 = 1;
pub const CODE_DRY_RUN: u8 = 2;
pub const CODE_DENIED: u8 = 3;
pub const CODE_APPLIED_READBACK_UNAVAILABLE: u8 = 4;
pub const CODE_APPLIED_READBACK_MISMATCH: u8 = 5;
pub const CODE_FAILED: u8 = 6;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn delivery_roundtrip() {
        let frame = DeliveryFrame {
            envelope: [0xAB; ENVELOPE_LEN],
            power_kw: -12.5,
        };
        let buf = frame.encode();
        assert_eq!(buf.len(), DELIVERY_LEN);
        let back = DeliveryFrame::decode(&buf).expect("decode");
        assert_eq!(back, frame);
        assert_eq!(back.power_kw, -12.5);
    }

    #[test]
    fn delivery_rejects_bad_length() {
        assert_eq!(
            DeliveryFrame::decode(&[0u8; DELIVERY_LEN - 1]),
            Err(FrameError::BadLength {
                got: DELIVERY_LEN - 1
            })
        );
        assert_eq!(
            DeliveryFrame::decode(&[0u8; DELIVERY_LEN + 1]),
            Err(FrameError::BadLength {
                got: DELIVERY_LEN + 1
            })
        );
    }
}
