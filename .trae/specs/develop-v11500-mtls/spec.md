# v0.115.0 mTLS 通信安全 Spec

> 蓝图：`蓝图/phase2.md` §v0.115.0（L9679~L9966）。前序：v0.98.0 跨域通信通道、v0.32.0 PKI 证书（eneros-crypto `pki` 模块）、v0.31.0 国密（SM2/SM3/SM4）。
> 定位：P2-I 第 3 版，联邦安全通信基石，下游 v0.116.0 模型签名。

## Why

联邦跨域通信当前缺乏双向身份认证与国密加密通道，存在中间人攻击与窃听风险。本版本实现 mTLS 双向认证 + SM2/SM3/SM4 全国密通信加密，满足信创合规与"抓包全加密"出口判定。

## What Changes

- 新建 crate `crates/security/mtls/`（eneros-mtls），no_std，唯一依赖 `eneros-crypto = { path = "../crypto" }`
- 实现 SM 密码套件协商（SM2-DHE 密钥交换 + SM4-GCM/SM4-CBC + SM3-HMAC）
- 实现证书管理（链式验签 + 有效期 + CRL 吊销检查，复用 eneros-crypto `pki`）
- 实现 mTLS 握手状态机（双向认证 → 会话密钥派生）与记录层加密封装
- 新增 `configs/mtls.toml` 与 `docs/security/mtls-design.md`
- 版本同步 0.114.0 → 0.115.0（根 Cargo.toml / Makefile / ci.yml / gate.rs）

**蓝图偏差说明（沿用既有版本惯例）**：蓝图 §4.5 为 GmSSL C FFI（extern "C" + NonNull + TcpStream std 类型）。按 §4.3 no_std 硬性要求与 v0.113.0/v0.114.0 先例，FFI 移除，改为纯 Rust 实现 + `MtlsTransport` 同步 trait 抽象，主机可测；真实 GmSSL/Tongsuo 适配器归属集成层。偏差在设计文档 D 表中逐条登记。

## Impact

- Affected specs：本 spec（新增）
- Affected code：
  - 新增 `crates/security/mtls/`（src/{lib,cipher_suite,cert_mgr,handshake,record}.rs）
  - 新增 `configs/mtls.toml`、`docs/security/mtls-design.md`
  - 修改根 `Cargo.toml`（members + version）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`

## ADDED Requirements

### Requirement: SM 密码套件协商
系统 SHALL 提供 `SmCipherSuite`（key_exchange: KeyExchange / cipher: Cipher / mac: MacAlgorithm），客户端与服务端 hello 交换套件列表后按服务端优先顺序选出首个共同套件；无共同套件 SHALL 返回 `TlsError::NoCommonCipherSuite`。

#### Scenario: 协商成功
- **WHEN** 客户端提供 [Sm4Gcm+Sm3Hmac, Sm4Cbc+Sm3Hmac]，服务端支持 [Sm4Cbc+Sm3Hmac]
- **THEN** 协商结果为 Sm4Cbc+Sm3Hmac

#### Scenario: 无共同套件
- **WHEN** 双方套件列表无交集
- **THEN** 握手失败并返回 NoCommonCipherSuite

### Requirement: 证书管理
系统 SHALL 提供 `CertManager`：`verify_cert` 依次执行链式验签（复用 eneros-crypto `verify_signature`/`CertVerifier`）→ 有效期检查（now 越界 → `CertError::Expired`/`NotYetValid`）→ CRL 吊销检查（命中 → `CertError::Revoked`）；`load_crl` 解析吊销列表入库；验证顺序固定为签名先于有效期先于吊销，错误显式传播不吞错。

#### Scenario: 过期证书拒绝
- **WHEN** 验证 expiry < now 的证书
- **THEN** 返回 CertError::Expired

#### Scenario: 吊销证书拒绝
- **WHEN** 证书指纹在已加载 CRL 中
- **THEN** 返回 CertError::Revoked

### Requirement: mTLS 双向认证握手
系统 SHALL 提供 `MtlsContext::handshake`：ClientHello（套件+随机数）→ ServerHello+ServerCert+CertRequest → 客户端验服务端证 → ClientCert+SM2 密钥交换 → 服务端验客户端证 → 双方 Finished（SM3-HMAC 校验握手摘要）→ 派生会话密钥。任一步验证失败 SHALL 中止握手并更新 `TlsStats`。`verify_peer=false` 时跳过客户端证书验证（单向模式，默认 true 双向）。

#### Scenario: 双向认证成功
- **WHEN** 双方证书均有效且 HMAC 摘要匹配
- **THEN** 握手完成，双方得到相同会话密钥（SM4-GCM key + nonce 基）

#### Scenario: 对端证书无效
- **WHEN** 服务端证书过期/吊销/验签失败
- **THEN** 客户端中止握手，stats.rejected + 1，reason 记录对应 CertError

### Requirement: 记录层加密通道
系统 SHALL 提供 `MtlsRecord`：send 以 SM4-GCM（或协商套件）加密 + 单调递增序列号构造 nonce，防重放；recv 解密并校验 GCM tag/序列号窗口；解密失败 SHALL 返回 `TlsError::DecryptFailed` 并计入 stats。

#### Scenario: 加密往返
- **WHEN** 一端 seal 消息后另一端 open
- **THEN** 明文一致，且线上字节与明文完全不同（抓包全加密）

#### Scenario: 篡改密文
- **WHEN** 密文任一字节被翻转
- **THEN** open 返回 DecryptFailed

### Requirement: 可观测与性能
系统 SHALL 提供 `TlsStats`（handshakes/rejected/records_sent/records_recv/last_error）；release 模式 PERF 测试打印单次握手耗时，`ENEROS_PERF_GATE=1` 时断言 < 200ms（蓝图 §6.3/§7.2）；门禁判定沿用 v0.114.0 修复口径（`var(...).as_deref() == Ok("1")`）。

#### Scenario: 性能门禁
- **WHEN** release 模式运行 PERF 测试且 ENEROS_PERF_GATE=1
- **THEN** 单次握手（含双向验签 + SM2 密钥交换 + HMAC Finished）< 200ms

## MODIFIED Requirements

### Requirement: 版本同步
根 `Cargo.toml` workspace version 0.114.0 → 0.115.0；members 追加 `"crates/security/mtls"`（置于 `"crates/security/attestation"` 之后）；`Makefile` VERSION + L3 注释、`.github/workflows/ci.yml` L3 注释、`ci/src/gate.rs` L144+L233 注释串同步追加 v0.115.0 条目。

## REMOVED Requirements

无。
