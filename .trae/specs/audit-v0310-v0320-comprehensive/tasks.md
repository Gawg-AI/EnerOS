# Tasks — 审计 v0.31.0 + v0.32.0 综合合规性检查

> **审计原则**：Karpathy 四原则 — 只修复真实问题，不重构、不优化、不"改进"无关代码
> **任务分波**：Wave 1 静态扫描 → Wave 2 国标 KAT → Wave 3 PKI 正确性 → Wave 4 安全性 → Wave 5 修复 → Wave 6 回归验证

## Wave 1: 静态扫描（并行执行）

- [x] **Task 1: no_std 合规性扫描**
  - 在 `crates/security/crypto/src/` 下搜索 `use std::`
  - 搜索 `panic!` / `todo!` / `unimplemented!` / `unreachable!`（排除 `#[cfg(test)]` 模块）
  - 验证仅 `lib.rs` 有 `#![cfg_attr(not(test), no_std)]`，子模块不重复
  - 检查 `extern crate alloc` 使用规范
  - **验证**：搜索结果为 0 违规，或记录违规清单

- [x] **Task 2: 版本标识一致性扫描**
  - 检查根 `Cargo.toml` version = "0.32.0"
  - 检查 `crates/security/crypto/Cargo.toml` version.workspace = true
  - 检查 `lib.rs` VERSION = "0.32.0"
  - 检查 `Makefile` VERSION := 0.32.0
  - 检查 `.github/workflows/ci.yml` Version: v0.32.0
  - 检查 `ci/src/gate.rs` 注释含 v0.32.0
  - **验证**：所有位置一致，无 0.31.0 残留（除历史注释）

- [x] **Task 3: 代码质量扫描**
  - 运行 `cargo fmt --all -- --check` 验证格式
  - 运行 `cargo clippy -p eneros-crypto --all-targets -- -D warnings` 验证无警告
  - 运行 `cargo deny check licenses bans sources` 验证合规
  - **验证**：全部 PASS，或记录警告清单

## Wave 2: 国标 KAT 验证（并行执行）

- [x] **Task 4: SM3 KAT 覆盖验证**
  - 读取 `sm3_kat` 测试模块
  - 验证至少包含 GB/T 32905-2016 附录 A 的 3 组标准向量：
    - 空串 ("")
    - "abc"
    - 64 字节消息（512 bit）
  - 验证哈希值与国标附录一致
  - **验证**：KAT 向量覆盖完整，或记录缺失向量

- [x] **Task 5: SM4 KAT 覆盖验证**
  - 读取 `sm4_kat` 测试模块
  - 验证 ECB 模式至少 1 组标准加解密向量
  - 验证 CBC 模式至少 1 组标准加解密向量
  - 验证 GCM 模式至少 1 组标准加解密向量
  - 验证密钥/明文/密文与国标附录一致
  - **验证**：KAT 向量覆盖完整，或记录缺失向量

- [x] **Task 6: SM2 KAT 覆盖验证**
  - 读取 `sm2_kat` 测试模块
  - 验证签名生成/验证 KAT 至少 1 组
  - 验证加密/解密 KAT 至少 1 组（若实现）
  - 验证曲线参数与 GB/T 32918.5-2017 推荐参数一致（p/a/b/Gx/Gy/n）
  - **验证**：KAT 向量覆盖完整，曲线参数正确

## Wave 3: PKI 正确性验证（并行执行）

- [x] **Task 7: ASN.1 DER 往返一致性验证**
  - 读取 `asn1.rs` 测试模块
  - 验证 INTEGER/SEQUENCE/SET/OID/UTCTime/GeneralizedTime 往返测试存在
  - 验证 BIT STRING/OCTET STRING/BOOLEAN/NULL 往返测试存在
  - 验证 Context-specific [0]..[3] EXPLICIT 往返测试存在
  - 验证长格式长度边界（≥ 128 字节）测试存在
  - **验证**：往返测试覆盖完整，或记录缺失场景

- [x] **Task 8: 证书有效期边界检查验证**
  - 读取 `verify.rs` 中 `verify` 方法的有效期检查逻辑
  - 验证 `now == not_before` → Ok(())（含等号）
  - 验证 `now == not_after` → Ok(())（含等号，RFC 5280 §4.1.2.5）
  - 验证 `now < not_before` → CertNotYetValid
  - 验证 `now > not_after` → CertExpired
  - 检查集成测试是否覆盖边界场景
  - **验证**：边界逻辑正确，或记录边界 bug

- [x] **Task 9: CRL 吊销检查正确性验证**
  - 读取 `crl.rs` 的 `is_revoked` 方法
  - 验证按 serial_number 精确匹配（长度不同不匹配）
  - 验证空 CRL（revoked = []）不触发吊销
  - 验证 `verify.rs` 中 CRL 检查逻辑正确调用 `is_revoked`
  - **验证**：CRL 逻辑正确，或记录 bug

- [x] **Task 10: 签名验证拒绝无效签名验证**
  - 读取 `sm2/sign.rs` 的 `verify` 方法
  - 验证签名长度 ≠ 64 字节时拒绝
  - 验证 r 或 s 不在 [1, n-1] 范围时拒绝
  - 验证错误公钥验证时返回 false
  - 验证错误消息验证时返回 false
  - 读取 `verify.rs` 的 `verify_signature` 函数
  - 验证签名长度检查 + SM2 verify 调用正确
  - **验证**：无效签名被正确拒绝，或记录 bug

## Wave 4: 安全性审计（并行执行）

- [x] **Task 11: 常数时间比较审计**
  - 读取 `constant_time.rs`
  - 验证 `ct_eq` 实现是常数时间（无提前返回/分支）
  - 搜索密码学比较场景是否使用 `ct_eq`（而非 `==`）
  - 重点检查：签名验证、MAC 比较、密钥比较
  - **验证**：常数时间比较正确使用，或记录违规

- [x] **Task 12: 私钥清零审计**
  - 读取 `sm2/keypair.rs` 中 `Sm2PrivateKey`
  - 验证实现 `Drop` trait 清零私钥
  - 搜索其他私钥类型（如 `Sm4Key`、`Sm2Signature` 的内部状态）
  - 验证私钥不在日志/错误信息中泄露
  - **验证**：私钥清零完整，或记录违规

- [x] **Task 13: RNG 熵源安全审计**
  - 读取 `rng.rs`
  - 验证 `CsRng` 实现（NIST SP 800-90A 风格）
  - 验证 `from_seed` 标记为测试专用（或文档说明生产环境需硬件 TRNG）
  - 验证 RNG 状态不被泄露（Debug impl 不暴露内部状态）
  - **验证**：RNG 安全性合规，或记录违规

## Wave 5: 修复发现的问题

- [x] **Task 14: 修复 Wave 1-4 发现的问题** ✅
  - 根据 Wave 1-4 审计结果，修复发现的真实问题
  - 遵循 Surgical Changes 原则：只修复发现的问题，不重构
  - 每个修复记录：问题 → 修复 → 验证
  - **依赖**：Task 1-13 全部完成
  - **验证**：修复后 `cargo test -p eneros-crypto` 全部通过
  - **修复记录**：
    1. **[HIGH] Sm2PrivateKey/Sm2KeyPair Debug 泄露私钥**
       - 问题：`#[derive(Debug)]` 导致 `format!("{:?}", sk)` 输出私钥标量 d 明文
       - 修复：移除 Debug derive，手工实现输出 `<Sm2PrivateKey: redacted>` / `<Sm2KeyPair: redacted>`（keypair.rs L225-229 / L294-298）
       - 验证：cargo test 402 PASS，Debug 输出已 redacted
    2. **[MEDIUM-HIGH] SM4 轮密钥未清零**
       - 问题：Sm4 结构体无 Drop trait，析构后 32 个轮密钥残留内存（等价于主密钥）
       - 修复：为 Sm4 实现 Drop（mod.rs L219-230）用 write_volatile 清零 rk；为 Sm4Gcm 实现 Drop（gcm.rs L163-175）清零 H
       - 验证：cargo test PASS，clippy 无警告
    3. **[MEDIUM-HIGH] CsRng::new() 在生产代码中使用固定种子**
       - 问题：CaIssuer::new 内部调用 CsRng::new()（硬编码固定种子），而非接受外部 RNG
       - 修复：修改 CaIssuer::new 签名接受 `rng: CsRng` 参数（ca.rs L77），子代理更新全部 19 处调用方
       - 验证：cargo build + cargo test --no-run 编译通过，cargo test 402 PASS
    4. **格式化修复**：cargo fmt --all 修复 ca.rs 中 3 处行宽超限（L74/L448/L474）
  - **已知偏差（不修复，需重构）**：
    - [HIGH] scalar_mult 时序侧信道（keypair.rs L148-156）—— 修复需重构 Montgomery ladder
    - [MEDIUM] CBC padding 时序差异（cbc.rs L112-118）—— padding oracle 向量
    - [MEDIUM] RNG 缺少自动 reseed（csrng.rs）—— NIST SP 800-90A 强制要求

## Wave 6: 回归验证

- [x] **Task 15: 全量回归验证** ✅
  - 运行 `cargo fmt --all -- --check` — **PASS**（exit 0）
  - 运行 `cargo clippy -p eneros-crypto --all-targets -- -D warnings` — **PASS**（0 warnings, 1.27s）
  - 运行 `cargo test -p eneros-crypto` — **PASS**（402 tests: 345 unit + 11 pki 集成 + 15 sm2_kat + 10 sm3_kat + 10 sm4_kat + 11 doctests）
  - 运行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` — **PASS**（eneros-time 117 + eneros-tsdb 104 + eneros-user-heap 9 + eneros-watchdog 22 + 全部 doctests）
  - 运行 `cargo run -p eneros-ci` — **PASS**（Overall: PASS, fmt 363ms + clippy 996ms + audit 1513ms + test 13378ms）
  - WSL2 运行 aarch64 交叉编译 — **PASS**（1.93s, Finished dev profile）
  - 运行 `cargo deny check licenses bans sources` — **PASS**（bans ok / licenses ok / sources ok）
  - **验证**：全部 PASS，无回归

## Task Dependencies

- Task 1-13: 可并行执行（Wave 1-4 独立审计维度）
- Task 14: 依赖 Task 1-13 全部完成（修复需基于完整审计结果）
- Task 15: 依赖 Task 14 完成（回归验证修复结果）
