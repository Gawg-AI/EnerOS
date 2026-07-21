# Checklist — 审计 v0.31.0 + v0.32.0 综合合规性检查

> **验证清单**：所有检查项必须通过才能标记审计完成。
> **回归保护**：v0.31.0 的 249 tests + v0.32.0 的 402 tests 必须全部继续通过。

## 一、静态扫描通过

- [x] **C1 无 `use std::*` 违规**：`crates/security/crypto/src/` 下搜索 `use std::` 返回 0 匹配
- [x] **C2 无 panic 宏违规**：搜索 `panic!` / `todo!` / `unimplemented!` / `unreachable!` 在非测试代码中返回 0 匹配
- [x] **C3 no_std 继承正确**：仅 `lib.rs` 有 `#![cfg_attr(not(test), no_std)]`，子模块不重复
- [x] **C4 extern crate alloc 规范**：所有使用 `alloc` 的模块有 `extern crate alloc;`
- [x] **C5 版本标识一致**：所有版本标识位置显示 0.32.0，无 0.31.0 残留（除历史注释）
- [x] **C6 cargo fmt 通过**：`cargo fmt --all -- --check` 无差异
- [x] **C7 cargo clippy 通过**：`cargo clippy -p eneros-crypto --all-targets -- -D warnings` 无警告
- [x] **C8 cargo deny 通过**：licenses/bans/sources 全部 ok

## 二、国标 KAT 覆盖完整

- [x] **C9 SM3 KAT 覆盖**：至少包含 GB/T 32905-2016 附录 A 的 3 组向量（空串/abc/64字节）
- [x] **C10 SM3 KAT 哈希值正确**：每组哈希值与国标附录一致
- [x] **C11 SM4 ECB KAT 覆盖**：至少 1 组标准加解密向量
- [x] **C12 SM4 CBC KAT 覆盖**：至少 1 组标准加解密向量
- [x] **C13 SM4 GCM KAT 覆盖**：至少 1 组标准加解密向量
- [x] **C14 SM4 KAT 密文正确**：密钥/明文/密文与国标附录一致
- [x] **C15 SM2 签名 KAT 覆盖**：至少 1 组签名生成/验证向量
- [x] **C16 SM2 加密 KAT 覆盖**：至少 1 组加密/解密向量（若实现）
- [x] **C17 SM2 曲线参数正确**：p/a/b/Gx/Gy/n 与 GB/T 32918.5-2017 推荐参数一致

## 三、PKI 正确性验证

- [x] **C18 ASN.1 INTEGER 往返**：encode → decode 一致
- [x] **C19 ASN.1 SEQUENCE/SET 往返**：encode → decode 一致
- [x] **C20 ASN.1 OID 往返**：encode → decode 一致
- [x] **C21 ASN.1 UTCTime/GeneralizedTime 往返**：encode → decode 一致
- [x] **C22 ASN.1 BIT STRING/OCTET STRING 往返**：encode → decode 一致
- [x] **C23 ASN.1 BOOLEAN/NULL 往返**：encode → decode 一致
- [x] **C24 ASN.1 Context-specific [0]..[3] 往返**：encode → decode 一致
- [x] **C25 ASN.1 长格式长度边界**：≥ 128 字节长度正确编码
- [x] **C26 证书有效期 now == not_before 通过**：边界含等号
- [x] **C27 证书有效期 now == not_after 通过**：边界含等号（RFC 5280 §4.1.2.5）
- [x] **C28 证书有效期 now < not_before 拒绝**：返回 CertNotYetValid
- [x] **C29 证书有效期 now > not_after 拒绝**：返回 CertExpired
- [x] **C30 CRL 空 revoked 不吊销**：空 CRL 验证通过
- [x] **C31 CRL serial 精确匹配**：长度不同不匹配
- [x] **C32 签名长度 ≠ 64 字节拒绝**：返回 SignatureInvalid
- [x] **C33 错误公钥验证拒绝**：返回 false 或 Err
- [x] **C34 错误消息验证拒绝**：返回 false 或 Err
- [x] **C35 verify_signature 正确调用 Sm2Signer::verify**：TBS + 签名 + 公钥

## 四、安全性审计

- [x] **C36 ct_eq 常数时间实现**：无提前返回/分支
- [x] **C37 签名验证使用 ct_eq**：签名比较场景使用常数时间比较
- [x] **C38 Sm2PrivateKey 实现 Drop**：析构时清零私钥
- [x] **C39 私钥不在日志/错误泄露**：Debug impl 不暴露私钥内容
- [x] **C40 CsRng from_seed 标记测试专用**：文档说明生产环境需硬件 TRNG
- [x] **C41 CsRng Debug 不泄露状态**：Debug impl 不暴露内部状态
- [x] **C42 无 unsafe 滥用**：unsafe 块有充分注释说明

## 五、修复完成验证

- [x] **C43 Wave 1-4 发现的问题已修复**：每个问题有修复记录
- [x] **C44 修复遵循 Surgical Changes**：只修复发现的问题，不重构
- [x] **C45 修复未引入新依赖**：eneros-crypto 保持零外部依赖
- [x] **C46 修复未破坏 no_std 合规**：修复后 no_std 检查仍通过

## 六、回归验证通过

- [x] **C47 cargo test -p eneros-crypto 通过**：v0.31.0 + v0.32.0 全部测试通过
- [x] **C48 v0.31.0 249 tests 回归**：原有测试无回归
- [x] **C49 v0.32.0 402 tests 回归**：原有测试无回归
- [x] **C50 workspace 回归通过**：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全绿
- [x] **C51 eneros-ci Overall: PASS**：fmt/clippy/test 全部通过
- [x] **C52 aarch64 交叉编译通过**：WSL2 `cargo build -p eneros-crypto --target aarch64-unknown-none` 通过
- [x] **C53 cargo fmt 通过**：修复后格式检查通过
- [x] **C54 cargo clippy 通过**：修复后 clippy 无警告
- [x] **C55 cargo deny 通过**：修复后 deny 检查通过

## 七、审计记录完整性

- [x] **C56 审计报告记录完整**：每个 Task 的审计结果已记录
- [x] **C57 发现的问题清单完整**：所有发现的问题已记录到 tasks.md
- [x] **C58 修复记录完整**：每个修复有 问题 → 修复 → 验证 三段记录
- [x] **C59 无遗留问题**：所有发现的问题已修复或记录为已知偏差
