//! EnerOS v0.113.0 Secure Boot 全链验证（P2-I 安全体系第 1 版）.
//!
//! 防止篡改镜像启动，保障系统从源头可信（蓝图 §1）。ROM → Bootloader → 内核 →
//! Runtime 逐级 SM2 签名校验，无签名验证则恶意镜像可启动（§2 阻塞项）。本 crate 在
//! v0.31.0/v0.32.0 国密 SM2/SM3 与 PKI 基座（eneros-crypto）之上，实现四级信任链
//! 验证器 + 镜像签名头格式 + 防降级时间戳，为 v0.114.0 测量启动/远程证明与联邦安全
//! 启动奠基。
//!
//! # 核心类型
//!
//! - [`ImageSignature`] / [`HEADER_LEN`] / [`encode_header`] / [`decode_header`] —
//!   镜像签名头（magic "ESIG" + version 1，全固定字段 118B，全小端二进制编解码，
//!   零 serde，D7/D11）
//! - [`BootStage`] / [`ChainOfTrust`] — 启动阶段（Rom/Bootloader/Kernel/Runtime/
//!   Complete）与信任链状态（root_key + stage_key + current_stage，D5/D6）
//! - [`BootVerifier`] — 四级验证器：`verify_stage`（顺序强制 → ROM/Complete 直通 →
//!   头校验 → 长度校验 → SM3 哈希 → 防降级 → 选钥 → SM2 验签）+ `advance_stage`
//!   （BL→Kernel 强制携带下级密钥），D4/D10
//! - [`BootError`] — 错误枚举（10 变体，D11）
//! - [`BootStats`] — 验证统计（verified_stages/rejected/last_error，§9 可观测，D11）
//!
//! # 偏差声明（D1~D13，相对蓝图 §3/§4/§5/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/secure_boot/` → `crates/security/secure-boot/`（eneros-secure-boot） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；Secure Boot 属安全体系，与 crypto/iec62351 同属 security 子系统 |
//! | **D2** | 蓝图 `docs/phase2/secure_boot.md` → `docs/security/secure-boot-design.md` | 记忆 §2.3.3 强制：文档按方向分类（docs/security/ 已有 pki-design.md/sm-crypto-design.md 先例） |
//! | **D3** | 蓝图 `tests/secure_boot.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.111.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图四文件 `rom_verify/bl_verify/kernel_verify/rt_verify.rs` → 单 `verifier.rs`（另拆 header.rs/chain.rs 承载数据结构与编解码） | 四级验证逻辑同构（仅验签密钥来源不同），四文件属重复建设（禁忌 14）；Karpathy 最小实现 |
//! | **D5** | 蓝图密钥 `[u8; 64]`（rom_root_key/bl_pubkey）→ `Sm2PublicKey` 强类型 | SM2 未压缩公钥为 65B（0x04‖x‖y），蓝图 64B 与 eneros-crypto `Sm2PublicKey::from_bytes/to_bytes_uncompressed` 格式不符；强类型编译期防错 |
//! | **D6** | ① 删除 `ChainOfTrust.kernel_sig/runtime_sig` 死字段（蓝图声明后从未使用）；② 修复 `bl_pubkey: [0u8;64]` 初始化后永不更新、Kernel/Runtime 验签恒用零密钥的蓝图 bug → `advance_stage(next_key: Option<Sm2PublicKey>)` 显式传递下级密钥，`Bootloader→Kernel` 强制 Some，否则 `Err(MissingStageKey)` | 蓝图代码逻辑错误必须修复；BL 公钥随已验签镜像体传递，完整性由哈希+签名覆盖传递可信；Kernel→Runtime 传 None 沿用「同 BL key」蓝图语义 |
//! | **D7** | 删除蓝图 `ImageSignature.signer_cert: Vec<u8>` 字段 | 信任锚为构造注入的信任根公钥；证书链验证归 v0.32.0 PKI 层职责，本版不做链式验证（v0.111.0 D11 同先例，Karpathy 最小实现）；结构体由此全固定字段 118B 可 Copy |
//! | **D8** | 蓝图 `get_min_timestamp` 恒返回 0（反降级空转）→ 构造注入 `min_timestamp: u64`，每级校验 `sig.timestamp >= min_timestamp` | 蓝图防降级机制无实际效果；熔丝/安全存储中的时间戳下限由集成层供给（no_std 无安全存储抽象，注入先例同 v0.111.0 D11） |
//! | **D9** | 蓝图 `sm3_hash`/`sm2_verify_sig` 未指明实现 → 复用 eneros-crypto（path 依赖 `../crypto`）：`sm3_hash(data) -> [u8;32]`、`sm2_verify(&hash, &Sm2Signature, &Sm2PublicKey)`、`Sm2Signature::from_bytes` | 记忆 §5.5/禁忌 14 禁止重复造轮子；国密实现已经安全评审（常量时间/零化/Drop），自研重引入风险 |
//! | **D10** | 补充蓝图缺失校验：`stage != current_stage` → `WrongStage`（强制逐级顺序）；`image.len() != sig.image_size` → `SizeMismatch`；`version != 1` → `UnsupportedVersion` | 蓝图 verify_stage 不校验 stage 顺序，可跳级验签；image_size 字段声明未用；version 字段同理 |
//! | **D11** | 错误模型 `BootError` = InvalidMagic / UnsupportedVersion / InvalidHeader / SizeMismatch / HashMismatch / SignatureInvalid / StaleImage / WrongStage / MissingStageKey / AlreadyComplete（10 变体，Copy 对齐 v0.111.0 OtaError 惯例）；新增 `BootStats { verified_stages, rejected, last_error }` 落地 §9 可观测；恢复模式（§4.4）为平台集成职责，crate 仅返回错误 | 蓝图引用 BootError 未定义；变体覆盖 §4.4 各失败面；no_std 无平台复位/恢复模式抽象，集成层据 Err 进入恢复 |
//! | **D12** | 信任根公钥配置（蓝图 §3「配置：信任根公钥配置」）落地为 `configs/secure-boot.toml` 模板（hex 占位符 + 注释），真实密钥由集成层烧录 | 密钥不入仓（记忆 §3.1 密钥禁忌）；配置模板先例同 v0.111.0 |
//! | **D13** | PERF20 蓝图 §7.2「< 50ms」release 断言 → release 默认仅打印计时，设 `ENEROS_PERF_GATE=1` 环境变量时启用 < 50ms 断言 | eneros-crypto 纯 Rust 仿射坐标 + EEA 模逆，主机 release 实测 ~150ms 超指标；50ms 面向目标硬件 SM2 加速场景；机器相关性能断言默认关闭避免主机/CI 误红，目标硬件/性能 CI 经 ENEROS_PERF_GATE=1 开启门禁；本版硬约束禁改 eneros-crypto，crypto 点运算优化（Jacobian/窗口法）为后续议题 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]`。
//! 仅使用 `core::*`，唯一依赖 eneros-crypto（workspace 内 path 依赖），
//! 零第三方依赖，零 unsafe，零 extern "C"，不调用 `panic!` / `todo!` /
//! `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

pub mod chain;
pub mod header;
pub mod verifier;

pub use chain::{BootStage, ChainOfTrust};
pub use header::{decode_header, encode_header, ImageSignature, HEADER_LEN};
pub use verifier::BootVerifier;

/// Secure Boot 错误（D11：10 变体覆盖蓝图 §4.4 各失败面，Copy 对齐 v0.111.0
/// OtaError 惯例；crate 仅返回错误，恢复模式为平台集成层职责）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootError {
    /// 签名头魔数错误（非 "ESIG"）.
    InvalidMagic,
    /// 签名头版本不支持（当前仅支持 version = 1）.
    UnsupportedVersion,
    /// 签名头无效（解码输入长度不足 [`HEADER_LEN`] 字节）.
    InvalidHeader,
    /// 镜像实际长度与签名头声明的 `image_size` 不符（防截断镜像）.
    SizeMismatch,
    /// 镜像 SM3 哈希与签名头声明的 `image_hash` 不符（镜像被篡改）.
    HashMismatch,
    /// SM2 验签失败（签名编码非法 / 验签结果为 false / 验签内部错误）.
    SignatureInvalid,
    /// 镜像时间戳低于防降级下限 `min_timestamp`（拒绝降级启动）.
    StaleImage,
    /// 启动阶段顺序错误（`verify_stage` 的 stage 必须与当前阶段一致，强制逐级）.
    WrongStage,
    /// 缺少下级验签密钥（Kernel/Runtime 验签时 stage_key 未安装，或
    /// Bootloader→Kernel 推进时未携带 BL 公钥）.
    MissingStageKey,
    /// 信任链已推进至 Complete，无法再次推进.
    AlreadyComplete,
}

/// Secure Boot 验证统计（§9 可观测落地，D11）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootStats {
    /// 累计验签通过的启动阶段数（Rom/Complete 直通不计入）.
    pub verified_stages: u32,
    /// 累计拒绝次数（`verify_stage` / `advance_stage` 全部失败路径）.
    pub rejected: u32,
    /// 最近一次拒绝的错误（无拒绝时为 None）.
    pub last_error: Option<BootError>,
}
