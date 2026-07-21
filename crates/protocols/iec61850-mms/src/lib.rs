//! EnerOS v0.106.0 IEC 61850 MMS 协议栈（P2-G 第 2 版：MMS 服务层）.
//!
//! 在 v0.105.0 信息模型（LD/LN/DO/DA）类型基座上实现 BER 编解码 + ACSE 关联
//! + COTP 握手 + MMS Read/Write 服务，打通联邦多机 IEC 61850 通信的服务层。
//!
//! 为 v0.107.0 GOOSE、v0.108.0 SV + IEC 62351 奠基。
//!
//! # 核心类型
//!
//! - [`ber_encode::BerEncoder`] — BER 编码器（tag + 长度占位 + 内容 + 回填）
//! - [`ber_decode::decode_read_response`] / [`ber_decode::decode_write_response`] / [`ber_decode::read_tag_length`] — BER 解码
//! - [`acse::encode_aarq`] / [`acse::decode_aare`] / [`acse::encode_cotp_cr`] / [`acse::decode_cotp_cc`] — ACSE 关联 + COTP 握手
//! - [`mms_client::MmsClient`] — MMS 客户端（泛型传输，connect 重试 ≤3 次 / read / write / disconnect）
//! - [`mms_client::MmsTransport`] — 传输层抽象 trait（connect/send/recv）
//! - [`mms_client::MockTransport`] — 脚本化 mock 传输（测试/集成占位）
//! - [`mms_client::MmsConnection`] / [`mms_client::ConnState`] — 连接状态（Idle/Connecting/Connected/Error）
//! - [`mms_client::MmsRequest`] / [`mms_client::MmsResponse`] — 请求/响应
//! - [`mms_client::VarAccessSpec`] — 变量访问规格（domain/item）
//! - [`mms_client::MmsReadResult`] / [`mms_client::MmsWriteResult`] — 读写结果
//! - [`mms_client::MmsErrorCode`] — MMS 错误码（Timeout/Refused/NotFound/TypeMismatch/Unknown）
//! - [`MmsError`] — MMS 错误（Timeout/ConnRefused/NotConnected/BerDecodeError/TransportError/IedError）
//!
//! # 偏差声明（D1~D12）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/iec61850_mms/` → `crates/protocols/iec61850-mms/`（eneros-iec61850-mms） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 modbus/iec104/iec61850-model 同 protocols 子系统 |
//! | **D2** | 蓝图 `docs/phase2/mms_protocol.md` → `docs/protocols/iec61850-mms-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/mms_client.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.105.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 新增 `MmsTransport` trait（connect/send/recv）+ `MockTransport`（置于 mms_client.rs，不新增文件）；`MmsClient<T: MmsTransport>` 泛型化；v0.29.0 Socket 真实接线在集成层 | 蓝图 §4.3 时序需要传输层但 §4.1/§4.5 无抽象；mqtt/iec104/agent-bus-dds 同先例（crate 内 trait+Mock，无真实网络 I/O）；no_std 主机可测 |
//! | **D5** | 蓝图 §4.1 `model: Arc<Iec61850Model>` 字段删除 | 蓝图 §4.5 全部代码从未使用该字段（read/write 仅以字符串 VarAccessSpec 操作），死字段（Karpathy Simplicity First）；DaValue/Quality 等类型经 eneros-iec61850-model crate 依赖保留；GetVariableAccessAttributes 的模型消费在后续版本接入 |
//! | **D6** | 蓝图 bug 修复①：BER 编码长度回填（`write_tag` 后无占位字节即写内容，`backfill_length` 会覆盖后续 tag；listOfVariable 用 `vars.len()` 元素个数冒充字节长度）→ tag+0x00 占位+内容+回填，长度恒为内容字节数 | 蓝图代码直接运行产出畸形 BER（Karpathy：不带着疑问照抄）；BER 长度语义为字节数（X.690） |
//! | **D7** | 蓝图 bug 修复②：浮点解码 `bytes[..copy_len]` 左对齐致 4 字节浮点错位 → 按 val_len 右对齐，4→`Float32`、8→`Float64`（蓝图一律 Float64） | IEC 61850 测量值可为 32 位浮点；左对齐解码数值错误 |
//! | **D8** | std `String`/`Vec`/`Arc` → `alloc::*`；trait/struct 无 Send+Sync bound | 蓝图 §43.1 + 记忆 §4.3 全项目 no_std；与 v0.64.0/v0.105.0 去 bound 惯例一致 |
//! | **D9** | COTP CR/CC 辅助（定长简化结构）放入 acse.rs（蓝图文件清单无 cotp.rs）；COTP 数据 TPDU 头在 mms_client 内联 | §4.3 时序含 COTP 握手但 §3 交付物无对应文件；acse.rs 同属关联建立层，不新增文件（Simplicity First）；真实 COTP 选项协商在集成层 |
//! | **D10** | 错误模型统一：`MmsError` = Timeout/ConnRefused/NotConnected/BerDecodeError/TransportError/IedError(MmsErrorCode)；§4.4"BER 解码失败→MmsErrorCode::Unknown"与 §4.5 代码 `MmsError::BerDecodeError` 矛盾 → 采用代码侧 | 蓝图自相矛盾（Karpathy：surface inconsistencies）；BerDecodeError 可区分本地解码失败与对端拒绝 |
//! | **D11** | 连接重试：§4.4"超时重试 3 次" → connect 至多 3 次尝试，第 3 次失败返回 Timeout；无 sleep（传输层内部决定超时语义），重试计数经 MockTransport 断言 | no_std 无计时器（v0.64.0 D1 时间注入先例）；重试次数上限语义与蓝图一致 |
//! | **D12** | 性能 100 点 < 50ms 落地为 cfg(test) Instant 断言（mock 回路，编码+解码口径，文档声明）；§6.2"与认证 IED 通信"集成测试为实验室硬件项，以 MockTransport 脚本化响应替代 | 无真实 IED 硬件（与 v0.105.0 D13 同口径）；v0.104.0 D12 测试计时先例 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，第三方依赖仅 `eneros-iec61850-model`（path），
//! 零 unsafe，不调用 `panic!` / `todo!` / `unimplemented!`，
//! 可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod acse;
pub mod ber_decode;
pub mod ber_encode;
pub mod mms_client;

pub use ber_decode::{decode_read_response, decode_write_response, read_tag_length};
pub use ber_encode::BerEncoder;
pub use mms_client::{
    ConnState, MmsClient, MmsConnection, MmsErrorCode, MmsReadResult, MmsRequest, MmsResponse,
    MmsTransport, MmsWriteResult, MockTransport, VarAccessSpec,
};

/// MMS 协议错误（D10：统一错误模型，区分本地解码失败与对端拒绝）。
#[derive(Debug, Clone, PartialEq)]
pub enum MmsError {
    /// 连接/接收超时（重试至多 3 次后报出，D11）。
    Timeout,
    /// 对端拒绝连接。
    ConnRefused,
    /// 未建立连接即调用 read/write。
    NotConnected,
    /// BER 解码失败（本地报文畸形/截断）。
    BerDecodeError,
    /// 传输层错误（发送/接收失败）。
    TransportError,
    /// IED 返回错误码（关联拒绝、服务错误等）。
    IedError(MmsErrorCode),
}
