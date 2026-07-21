//! EnerOS v0.108.0 IEC 62351 GOOSE/SV 安全层（P2-G 第 4 版）.
//!
//! 在 v0.31.0 国密 SM2/SM3/SM4（eneros-crypto）与 v0.107.0 GOOSE 事件通道基座上，
//! 实现 IEC 62351 GOOSE/SV 加密封装（SM4-GCM 机密性 + SM3-HMAC 完整性/认证）
//! 与会话密钥管理（多密钥存储、过期检测、密钥轮换），
//! 打通联邦安全通信的「采样 + 事件 + 加密」全链路，加密延迟 < 0.5ms。
//!
//! 为 v0.109.0 故障录波提供安全采样数据源。
//!
//! # 核心类型
//!
//! - [`key_mgmt::SessionKey`] — 会话密钥（key_id + SM4 密钥 + HMAC 密钥 + 过期时间，D9）
//! - [`key_mgmt::KeyMgmt`] — 密钥管理（多密钥存储 / 过期检测 / 密钥轮换，D6/D9）
//! - [`secure_goose::SecureGoose`] — GOOSE 加密封装（SM4-GCM + SM3-HMAC + IV 计数器）
//! - [`secure_sv::SecureSv`] — SV 加密封装（同构于 SecureGoose，独立类型防混用，D8）
//! - [`secure_goose::SecureFrame`] — 安全帧封装（key_id / iv / ciphertext / tag / hmac）
//! - [`SecError`] — 错误枚举（KeyExpired / HmacMismatch / DecryptFailed / EncryptFailed / InvalidKeyId，D10）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/iec61850_sv/` → `crates/protocols/iec61850-sv/`（eneros-iec61850-sv）；蓝图 `crates/iec62351/` → `crates/security/iec62351/`（eneros-iec62351） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；SV 属 protocols，IEC 62351 属 security |
//! | **D2** | 蓝图 `docs/phase2/sv_security.md` → `docs/protocols/iec61850-sv-design.md` + `docs/protocols/iec62351-design.md` | 记忆 §2.3.3 强制：文档按方向分类；两个 crate 独立文档 |
//! | **D3** | 蓝图 `tests/sv_secure.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.107.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 删除蓝图 §4.5 `extern "C"` raw socket FFI + unsafe；SV 侧复用 GOOSE 的 `L2Transport` trait + `MockL2`（置于 lib.rs）；真实 raw socket 接线在集成层 | aarch64-unknown-none 无 libc 可链接 extern "C"；项目零 unsafe/零 C FFI 惯例；与 v0.107.0 D4 同先例 |
//! | **D5** | `SvSubscriber<T: L2Transport>` 泛型化，transport 由 `new` 注入（蓝图内部建 socket 写死） | 可测试性 + 网卡选择属集成层决策（Karpathy Simplicity First） |
//! | **D6** | 蓝图 §4.1 `RingBuffer { buf: Box<[T]> }` → `Vec<T>` 固定容量（heapless 风格）；`Box` 在 no_std 需全局分配器，Vec 更通用 | no_std 下 `Box<[T]>` 需 `alloc::boxed::Box` 且初始化冗长；`Vec::with_capacity` 更直观（v0.107.0 MockL2 用 Vec 先例） |
//! | **D7** | 蓝图 §4.5 `Sm4Cipher`/`Sm3Hmac` 自封装 FFI → 直接复用 eneros-crypto 的 `Sm4Gcm`/`Sm3Hmac`（纯 Rust，零 unsafe） | v0.31.0 已落地纯 Rust 实现；蓝图 FFI 代码在 aarch64-unknown-none 无法链接（无 libc）；避免重复造轮子（记忆 §5.5） |
//! | **D8** | 蓝图 §4.1 `SecureGoose` 单类型 → `SecureGoose` + `SecureSv` 同构双类型（内部均委托公共 `SecureChannel` 私有结构） | GOOSE 与 SV 语义独立（事件 vs 采样），调用方不应混用；公共逻辑抽取私有结构避免重复（Simplicity First） |
//! | **D9** | 蓝图 §4.1 `KeyMgmt.rotate_keys()` 内部生成密钥 → `rotate_keys(now, new_key_data, new_mac_key)` 由调用方注入密钥材料 | no_std 无系统熵源（CsRng 固定种子仅测试用）；生产环境密钥应由硬件 TRNG/密钥管理系统注入；与 v0.31.0 CaIssuer 外部注入 rng 先例一致 |
//! | **D10** | 错误模型统一：`SvError` = TransportError / BerDecodeError / InvalidConfig / BufferOverflow（4 变体）；`SecError` = KeyExpired / HmacMismatch / DecryptFailed / EncryptFailed / InvalidKeyId（5 变体） | 蓝图 SocketCreateFailed/SendFailed 随 FFI 删除合并为 TransportError；变体覆盖各失败面（对齐 v0.107.0 D10 精简风格） |
//! | **D11** | 性能 < 0.5ms（加密延迟）落地为 cfg(test) Instant 断言（MockL2 回路，加密+解密口径，文档声明）；§6.2 真实 GOOSE 端到端加密为实验室硬件项，以 mock 替代 | 无真实网卡硬件（与 v0.107.0 D11 同口径） |
//! | **D12** | 接收侧 smpCnt 跳变检测以 `SampleStatus`（New/Duplicate/SmpJump）随样本返回；蓝图 §4.4 要求检测跳变但 §4.2 `receive -> Result<(), SvError>` 无承载 → `SvSample.status: SampleStatus` 字段 + `receive -> Result<bool, SvError>` | 蓝图自相矛盾（要求检测但接口无处上报）；接收方必须能区分新采样/重复/丢样 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，第三方依赖仅 `eneros-crypto`（path），
//! 零 unsafe，不调用 `panic!` / `todo!` / `unimplemented!`，
//! 可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod key_mgmt;
pub mod secure_goose;
pub mod secure_sv;

pub use key_mgmt::{KeyMgmt, SessionKey};
pub use secure_goose::{SecureFrame, SecureGoose};
pub use secure_sv::SecureSv;

/// IEC 62351 安全层错误（D10：统一错误模型，5 变体覆盖密钥/认证/加解密全部失败面）。
#[derive(Debug, Clone, PartialEq)]
pub enum SecError {
    /// 会话密钥已过期或不存在可用密钥。
    KeyExpired,
    /// SM3-HMAC 校验不匹配（报文被篡改或密钥错误，先于解密校验）。
    HmacMismatch,
    /// SM4-GCM 解密失败（tag 校验不通过）。
    DecryptFailed,
    /// SM4-GCM 加密失败。
    EncryptFailed,
    /// 按 key_id 查找密钥未命中。
    InvalidKeyId,
}
