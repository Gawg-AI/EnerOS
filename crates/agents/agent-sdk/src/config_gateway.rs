//! ConfigGet 调用侧管道内核（SPEC-SERIES-2026-08-29/S1 WP-S1.2）。
//!
//! 历史病灶（第五轮体检 C2）：`get_entry → decode_response_body → 领域
//! decode → 呈现` 的调用侧管道在 5 处逐字手抄（protocol-services
//! `service_cfg` ×2 / control-services `cfg_bindings` ×2 /
//! `dispatch_assembly::fetch_control_write` ×1），诊断措辞三档不齐。
//! 本模块把**管道与失败分类**收编一处（[`config_get`] + [`ConfigStep`] +
//! [`response_body`]）；呈现（各自串口词形与 fail-closed 策略）仍归调用方
//! ——与 [`crate::config_client`] 的分工：wire 帧知识（请求构造/分块/
//! 4B 前缀解析）在 config_client，调用侧管道在 config_gateway。
//!
//! R2 红线：本模块**不打印**任何串口行、不定义任何词形——调用方按
//! [`ConfigStep`] 呈现各自既有措辞（各串口判定行逐字节保持）。
//! host 无条件可用（不依赖 `sel4` feature；69.3 WP-69.3.4 起
//! [`crate::endpoint::ChunkedEndpoint`] 的 impl 为无条件 blanket
//! `C: RawIpc`，原 `Sel4IpcClient` 具体实现及其 `any(test, sel4)` 门
//! 随泛型化撤销）。

use alloc::vec::Vec;

use crate::config_client::{self, ConfigError};

/// ConfigGet 调用侧取数 seam：节 id → 原始响应字节（含 4B 长度前缀）。
///
/// `None` = 未授予 / 传输失败（两态对调用方呈现等价）。target 实现挂在
/// [`crate::endpoint::ChunkedEndpoint`]（蓝图 67 WP-67.2.2）；host 测试由
/// 各消费方持内存假件（`FakeConfigServer` 先例，[`crate::config_client`]
/// 同法）。
pub trait ConfigEntrySource {
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>>;

    /// active bundle generation（蓝图 61 观测映射世代；不可得 → None——
    /// 调用方按 fail-closed 降级：世代未知的事件不满足控制资格）。
    fn config_generation(&mut self) -> Option<u64> {
        None
    }
}

/// 调用侧管道失败分类（单点）。[`ConfigError`] 映射：
/// `Transport → Unavailable`、`TooShort → TooShort`、`LengthPrefix →
/// BadPrefix`；领域解码失败由 decode 闭包直接给出 `DecodeFailed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStep {
    /// 未授予 / 传输失败（fetch 不得字节）。
    Unavailable,
    /// 响应不足 4B 长度前缀。
    TooShort,
    /// 4B 长度前缀越出响应体。
    BadPrefix,
    /// 领域解码失败（decode 闭包返回 Err）。
    DecodeFailed,
}

impl From<ConfigError> for ConfigStep {
    fn from(e: ConfigError) -> Self {
        match e {
            ConfigError::Transport => ConfigStep::Unavailable,
            ConfigError::TooShort => ConfigStep::TooShort,
            ConfigError::LengthPrefix => ConfigStep::BadPrefix,
        }
    }
}

/// 响应前缀剥离单点（`4B LE 长度前缀 + canonical payload`）。
///
/// 供需要**先检视原始载荷**再剥离的调用方使用（如 svc-ai-worker 的
/// `ERR:` 结构化拒绝透传——原始响应检查必须先于前缀剥离）。
/// 知识本体仍 = [`config_client::decode_response_body`]（本函数是管道
/// 侧入口，不复制实现）。
pub fn response_body(payload: &[u8]) -> Result<&[u8], ConfigStep> {
    config_client::decode_response_body(payload).map_err(ConfigStep::from)
}

/// 取一节配置并解码——调用侧管道单点。
///
/// `decode` 必须是 **HRTB 闭包**，不得是 `fn` 指针（F-32 实测）：领域
/// 解码常需额外实参（如 `decode_points(body, &ConfigId)`）——闭包捕获
/// 之，`fn` 指针不允许捕获（E0308）。decode 在前缀剥离的**借切片有效期
/// 内**完成（F-29：`decode_response_body` 返回借切片）。
pub fn config_get<T>(
    io: &mut dyn ConfigEntrySource,
    id: &str,
    decode: impl for<'a> FnOnce(&'a [u8]) -> Result<T, ConfigStep>,
) -> Result<T, ConfigStep> {
    let payload = io.config_entry(id).ok_or(ConfigStep::Unavailable)?;
    let body = response_body(&payload)?;
    decode(body)
}

/// 蓝图 67 WP-67.2.2（[01] §2.1a 执行期订正）：transport impl 挂
/// [`ChunkedEndpoint`]（epoch 单点纪律直达 trait 面）——原
/// `Sel4IpcClient` impl 每次往返恒 epoch=0（类二违纪面）随 impl 迁移
/// 消除；消费方持 endpoint（svc-telemetry/svc-ai-worker/svc-device-agent/
/// dispatch_assembly catalog 路径/protocol_ladder Sel4ServiceIo）。
///
/// 69.3 WP-69.3.4：具体 impl 泛化为无条件 blanket（`C: RawIpc`）——任意
/// 传输适配器经同一桥进 ConfigGet 管道（`fetch_control_chain_key` 泛型化
/// + host RawIpc 假件的使能面）。
///
/// 原 `any(test, feature="sel4")` 门随泛型化失去意义：impl 体零 sel4 专有
/// 调用（get_entry/get_generation 本就泛型于 RawIpc），且不再提及
/// Sel4IpcClient（其类型门留在 ipc_sel4）；wire/epoch 语义零变更。
impl<C: crate::chunked::RawIpc> ConfigEntrySource for crate::endpoint::ChunkedEndpoint<C> {
    /// ConfigGet 一次往返（超时 = 常量默认 5_000ms，与原实参一致；
    /// epoch 由 endpoint 单点递增）。
    fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
        config_client::get_entry(self, id).ok()
    }

    /// GET_GENERATION 一次往返（蓝图 61 v0.314.27）。
    fn config_generation(&mut self) -> Option<u64> {
        config_client::get_generation(self).ok()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    /// 内存对端：按节 id 返回预置原始响应（含 4B 前缀）/ None（未授予）。
    struct FakeSource {
        entries: alloc::vec::Vec<(&'static str, Option<Vec<u8>>)>,
    }

    impl FakeSource {
        fn with(id: &'static str, raw: Option<Vec<u8>>) -> Self {
            Self {
                entries: alloc::vec![(id, raw)],
            }
        }
    }

    impl ConfigEntrySource for FakeSource {
        fn config_entry(&mut self, id: &str) -> Option<Vec<u8>> {
            self.entries
                .iter()
                .find(|(k, _)| *k == id)
                .and_then(|(_, v)| v.clone())
        }
    }

    /// 原始响应帧：4B LE 长度前缀 + payload。
    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// protocol-* canonical KV 负载（`encode_protocol` 同格式：count +
    /// [klen + key + vlen:u16 + val]——agent-sdk 不依赖 config-bundle，
    /// 测试按格式构造、以真 `decode_protocol` 验证往返）。
    fn proto_payload(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut out = alloc::vec![entries.len() as u8];
        for (k, v) in entries {
            out.push(k.len() as u8);
            out.extend_from_slice(k.as_bytes());
            out.extend_from_slice(&(v.len() as u16).to_le_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        out
    }

    /// C3 臂 1：未授予 / 传输失败 → `Unavailable`（decode 不得被触达）。
    #[test]
    fn unavailable_when_entry_missing() {
        let mut io = FakeSource::with("protocol-x", None);
        let r: Result<alloc::string::String, ConfigStep> =
            config_get(&mut io, "protocol-x", |_| {
                unreachable!("fetch 失败不得进入 decode")
            });
        assert_eq!(r, Err(ConfigStep::Unavailable));
    }

    /// C3 臂 2：响应不足 4B → `TooShort`。
    #[test]
    fn too_short_when_response_under_4b() {
        let mut io = FakeSource::with("protocol-x", Some(vec![1, 2, 3]));
        let r: Result<alloc::string::String, ConfigStep> =
            config_get(&mut io, "protocol-x", |_| {
                unreachable!("前缀失败不得进入 decode")
            });
        assert_eq!(r, Err(ConfigStep::TooShort));
    }

    /// C3 臂 3：4B 前缀越出响应体 → `BadPrefix`。
    #[test]
    fn bad_prefix_when_length_exceeds_body() {
        let mut raw = alloc::vec![0u8];
        raw.extend_from_slice(&100u32.to_le_bytes());
        raw.extend_from_slice(b"abc");
        let mut io = FakeSource::with("protocol-x", Some(raw));
        let r: Result<alloc::string::String, ConfigStep> =
            config_get(&mut io, "protocol-x", |_| {
                unreachable!("前缀失败不得进入 decode")
            });
        assert_eq!(r, Err(ConfigStep::BadPrefix));
    }

    /// C3 臂 4：真实节 id（protocol-*）经完整管道解码成功，且 decode
    /// 收到的是**剥离前缀后**的 body（F-29 借切片在有效期内）。
    #[test]
    fn real_protocol_section_roundtrip() {
        let payload = proto_payload(&[("peer", "10.0.0.9:1502"), ("unit_id", "1")]);
        let mut io = FakeSource::with("protocol-modbus-tcp", Some(framed(&payload)));
        let got = config_get(&mut io, "protocol-modbus-tcp", |body| {
            assert_eq!(body, &payload, "decode 收到剥离 4B 前缀后的 canonical body");
            eneros_config_bundle::protocol::decode_protocol(body)
                .map_err(|_| ConfigStep::DecodeFailed)
        })
        .unwrap();
        assert_eq!(got.get("peer"), Some("10.0.0.9:1502"));
        assert_eq!(got.get("unit_id"), Some("1"));
    }

    /// 领域解码失败 → `DecodeFailed`（分类透传，不吞）。
    #[test]
    fn decode_failed_propagates() {
        let mut io = FakeSource::with("protocol-x", Some(framed(b"junk")));
        let r: Result<(), ConfigStep> =
            config_get(&mut io, "protocol-x", |_| Err(ConfigStep::DecodeFailed));
        assert_eq!(r, Err(ConfigStep::DecodeFailed));
    }

    /// `response_body` 前缀剥离单点：正例 + TooShort/BadPrefix 分类与
    /// `config_get` 一致（svc-ai-worker 先检视原始载荷再剥离的管道）。
    #[test]
    fn response_body_strip_matches_pipeline() {
        assert_eq!(response_body(&framed(b"kv")), Ok(&b"kv"[..]));
        assert_eq!(response_body(&[1, 2, 3]), Err(ConfigStep::TooShort));
        let mut raw = alloc::vec![0u8];
        raw.extend_from_slice(&9u32.to_le_bytes());
        raw.extend_from_slice(b"ab");
        assert_eq!(response_body(&raw), Err(ConfigStep::BadPrefix));
    }
}
