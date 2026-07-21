# Tasks — v0.115.0 mTLS 通信安全

> Spec：`.trae/specs/develop-v11500-mtls/spec.md`。蓝图：`蓝图/phase2.md` §v0.115.0。
> 全局约束：no_std（禁 `std::*`/`panic!`/`todo!`/`unimplemented!`，子模块不重复 `#![no_std]` 属性，lib.rs 统一声明）；代码注释中文；复用 eneros-crypto（path = "../crypto"）禁重复造轮子；测试模块可用 std。

- [x] Task 1：新建 crate 骨架 `crates/security/mtls/`（eneros-mtls）
  - [x] SubTask 1.1：`Cargo.toml`——name=eneros-mtls，version/edition/authors/license workspace 继承，description 含 "v0.115.0"；`[dependencies] eneros-crypto = { path = "../crypto" }`（对齐 attestation 模板）
  - [x] SubTask 1.2：`src/lib.rs`——`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；`TlsError`（8 变体：NoCommonCipherSuite/HandshakeFailed/CertInvalid/DecryptFailed/ReplayDetected/TransportError/InvalidMessage/InternalError）/ `CertError`（Expired/NotYetValid/Revoked/SignatureInvalid/ChainBroken）+ `From<CertError> for TlsError` / `TlsStats` / `MtlsTransport` trait（send/recv 同步）+ `MockMtlsTransport`（故障注入 + calls 计数）；模块声明 + 重导出
  - [x] SubTask 1.3：根 `Cargo.toml` members 追加 `"crates/security/mtls"`（置于 `"crates/security/attestation"` 之后）
  - 验证：`cargo metadata --format-version 1` 成功 ✅
- [x] Task 2：实现 `src/cipher_suite.rs` + `src/cert_mgr.rs`
  - [x] SubTask 2.1：`KeyExchange`(Sm2Dhe/EcdheSm2) / `Cipher`(Sm4Gcm/Sm4Cbc) / `MacAlgorithm`(Sm3Hmac/None) / `SmCipherSuite` + `negotiate(client: &[SmCipherSuite], server: &[SmCipherSuite]) -> Result<SmCipherSuite, TlsError>`（服务端优先顺序选首个交集）
  - [x] SubTask 2.2：`CertManager`——`new(trusted_roots: Vec<X509Certificate>)` / `verify_cert(cert, now)`（链式验签 → 有效期 → CRL，顺序固定）/ `check_revocation(cert)`（SM3 指纹比对 CRL）/ `load_crl(crl: Crl)`（复用 eneros-crypto `Crl`，不自研解析）
  - [x] SubTask 2.3：内嵌测试 SUITE1~SUITE3（协商成功/无交集/服务端优先）+ 套件编解码往返 + CERT4~CERT8（有效通过/过期/未生效/吊销/坏签名）
  - 验证：`cargo test -p eneros-mtls` 9/9 通过 ✅（CERT8 修复：他 CA 改用不同 DN，避免同名 DN 被 issuer 查找命中退化为 SignatureInvalid）
- [x] Task 3：实现 `src/handshake.rs` + `src/record.rs`
  - [x] SubTask 3.1：`MtlsContext`（local_cert/local_key/cert_mgr/verify_peer/cipher_suites）+ `handshake_client`/`handshake_server` 双向状态机（ClientHello → ServerHello+Cert+CertRequest → 互验证书 → SM2 密钥交换 → SM3-HMAC Finished → 派生会话密钥）；`HandshakeOutcome`（session_key + suite + peer_cert_fingerprint）
  - [x] SubTask 3.2：`MtlsRecord`——`seal(plaintext)`（SM4-GCM + 序列号 nonce + AAD 绑序列号）/ `open(ciphertext)`（tag 校验 + 防重放窗口）；密钥派生用 SM3 散列（master_secret ‖ "enc" / ‖ "mac" 标签分离）
  - [x] SubTask 3.3：内嵌测试 HS9~HS13（双向成功/服务端过期拒绝/客户端吊销拒绝/单向模式/HMAC 不匹配中止）+ REC14~REC17（加密往返/篡改拒绝/重放拒绝/序列号单调）
  - 验证：`cargo test -p eneros-mtls` 18/18 通过 ✅（HS10 修复：drop(transport) 释放 c2s_tx，防客户端先拒绝时服务端 recv 永久阻塞死锁）
- [x] Task 4：集成测试 + 性能 + 配置与文档
  - [x] SubTask 4.1：INT18~INT19（端到端双向 mTLS 加密通信 + 中间人篡改全线拒绝）+ PERF20（release 打印单次握手耗时，ENEROS_PERF_GATE=1 断言 < 200ms，门禁判定 `var(...).as_deref() == Ok("1")` 口径；release 实测 ≈1.47s，主机纯软 SM2 超目标硬件指标属预期，同 v0.113.0/v0.114.0 先例）
  - [x] SubTask 4.2：`configs/mtls.toml`——`[context]`（cert/key/ca 路径占位 + verify_peer=true）/ `[cipher]`（suites 列表）/ `[crl]`（路径占位 + 刷新说明）+ 中文注释 7 点；真实密钥/证书不入仓
  - [x] SubTask 4.3：`docs/security/mtls-design.md`——12 章节 + 2 Mermaid（§4.3 sequenceDiagram 握手时序 + §4.4 verify_cert 三步 flowchart）+ D1~D9 偏差表（FFI 移除/纯 Rust 实现/TcpStream→MtlsTransport 等逐条登记）+ 源码相对链接 `../../crates/security/mtls/src/...`
  - 验证：`cargo test -p eneros-mtls --release` 21/21 通过 ✅；debug 21/21 通过 ✅
- [x] Task 5：版本同步 + §2.4 全量校验
  - [x] SubTask 5.1：根 `Cargo.toml` version 0.114.0 → 0.115.0；`Makefile`（VERSION + L3 头部注释）/ `.github/workflows/ci.yml` L3 注释 / `ci/src/gate.rs` L144+L233 注释串同步追加 v0.115.0 条目
  - [x] SubTask 5.2：§2.4.2 构建校验全过——workspace 测试零回归 + aarch64-unknown-none 交叉编译 + fmt --check + clippy -D warnings + cargo deny check
  - 验证：上述命令全部零告警通过 ✅；`git status` 无垃圾文件 ✅

# Task Dependencies

- Task 2、Task 3 依赖 Task 1（crate 骨架 + 错误类型）
- Task 3 依赖 Task 2（SmCipherSuite/CertManager 被 handshake 引用）
- Task 4 依赖 Task 3（INT/PERF 基于完整实现）
- Task 5 依赖 Task 1~4（全量校验最后执行）
- 无并行项（单 crate 内聚强，串行最简）
