# Checklist — v0.115.0 mTLS 通信安全

## 功能正确性（蓝图 §7.1/§7.3）

- [x] `SmCipherSuite` 三字段（key_exchange/cipher/mac）+ `negotiate` 服务端优先选首个交集；无交集 → NoCommonCipherSuite
- [x] `CertManager::verify_cert` 顺序固定：链式验签 → 有效期 → CRL 吊销；错误显式传播无吞错
- [x] 过期 → CertError::Expired；未生效 → NotYetValid；CRL 命中 → Revoked；坏签名 → SignatureInvalid
- [x] `load_crl` 复用 eneros-crypto `Crl`，未自研 CRL 解析
- [x] `MtlsContext::handshake` 双向状态机完整（Hello → 证书交换 → 互验 → 密钥交换 → Finished HMAC → 会话密钥）
- [x] 双方握手后派生相同会话密钥；`verify_peer=false` 单向模式可用
- [x] Finished SM3-HMAC 不匹配 → 握手中止 + stats.rejected + last_error 记录
- [x] `MtlsRecord` SM4-GCM 加密：序列号 nonce + AAD 绑序列号；open 校验 tag + 防重放
- [x] 篡改密文 → DecryptFailed；重放帧 → 拒绝
- [x] `MtlsTransport` + `MockMtlsTransport`（故障注入 + calls 计数）
- [x] 无 extern "C"/unsafe/NonNull（GmSSL FFI 移除，偏差表登记）；无 std::net::TcpStream
- [x] 中间人场景：转发中篡改任一握手/记录字节 → 全线拒绝

## 测试（21 个全过）

- [x] SUITE1~SUITE3 + 套件编解码往返 通过
- [x] CERT4~CERT8 通过
- [x] HS9~HS13 通过
- [x] REC14~REC17 通过
- [x] INT18~INT19 通过（含中间人攻击场景）
- [x] PERF20：release 打印握手耗时（实测 ≈1.47s，主机纯软 SM2 属预期）；ENEROS_PERF_GATE=1 断言 < 200ms（蓝图 §6.3/§7.2）

## no_std 与依赖合规（记忆 §4.3/§5.5）

- [x] 无 `use std::*`（测试模块除外）；无 `panic!`/`todo!`/`unimplemented!`；子模块不重复 `#![no_std]`
- [x] 唯一依赖 `eneros-crypto = { path = "../crypto" }`；零外部 crates.io 依赖
- [x] 未自研 SM2/SM3/SM4/PKI/CRL（全部复用 eneros-crypto 公开 API）

## 目录结构（记忆 §2.4.1）

- [x] C1：crate 位于 `crates/security/mtls/`，未放根目录
- [x] C2：根 `Cargo.toml` members 已添加 `"crates/security/mtls"`（attestation 之后）
- [x] C3：`path = "../crypto"` 相对路径正确
- [x] C4：文档位于 `docs/security/mtls-design.md`，未平面化
- [x] C5：根目录无新 crate 文件夹

## 构建校验（记忆 §2.4.2）

- [x] C6：`cargo metadata` 成功
- [x] C7：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 零回归
- [x] C8：`cargo build -p eneros-mtls --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9：`cargo fmt --all -- --check` 通过
- [x] C10：`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 零告警
- [x] C11：`cargo deny check advisories licenses bans sources` 通过

## 文档与规范（记忆 §2.4.3）

- [x] C12：`docs/security/mtls-design.md` 12 章节 + 2 Mermaid + D1~D9 偏差表
- [x] C13：`git status` 无 target/、*.elf、*.bin、IDE 缓存被追踪
- [x] C14：无新文件类型需补 .gitignore
- [x] C15：提交信息遵循 Conventional Commits（feat(security/mtls): v0.115.0 ...）
- [x] `configs/mtls.toml` 三节齐全 + 中文注释；证书/密钥仅占位符，不入仓
- [x] 版本同步：根 Cargo.toml 0.115.0 + Makefile + ci.yml + gate.rs 四处一致
