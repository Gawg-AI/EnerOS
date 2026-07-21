# Tasks — v0.114.0 测量启动与远程证明

> Spec：`.trae/specs/develop-v11400-attestation/spec.md`。蓝图：`蓝图/phase2.md` §v0.114.0。
> 全局约束：no_std（禁 `std::*`/`panic!`/`todo!`/`unimplemented!`，子模块不重复 `#![no_std]` 属性，lib.rs 统一声明）；代码注释中文；复用 eneros-crypto（path = "../crypto"）禁重复造轮子；测试模块可用 std。

- [x] Task 1：新建 crate 骨架 `crates/security/attestation/`（eneros-attestation）
  - [x] SubTask 1.1：`Cargo.toml`——name=eneros-attestation，version/edition/authors/license workspace 继承，description 含 "v0.114.0"；`[dependencies] eneros-crypto = { path = "../crypto" }`（对齐 secure-boot 模板）
  - [x] SubTask 1.2：`src/lib.rs`——`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；`TpmError`（5 变体）/ `AttestError`（6 变体 + `From<TpmError>`）/ `AttestStats` / `AttestTransport` trait + `MockAttestTransport`；模块声明 + 重导出
  - [x] SubTask 1.3：根 `Cargo.toml` members 追加 `"crates/security/attestation"`（置于 `"crates/security/secure-boot"` 之后）
  - 验证：`cargo metadata --format-version 1` 成功
- [x] Task 2：实现 `src/tpm.rs`（PcrBank + TpmBackend + SoftTpm）
  - [x] SubTask 2.1：`PcrBank`（[[u8;32];24] 全零初始）+ `pcr_extend_value(current, digest) = sm3(current‖digest)` 共享函数
  - [x] SubTask 2.2：`TpmBackend` trait（pcr_extend/pcr_read/quote/attestation_pubkey）
  - [x] SubTask 2.3：`SoftTpm`（new/inject_failure/三方法实现 + fail_remaining 故障注入 + 越界 InvalidPcrIndex + 空选择 EmptyPcrSelection + quote SM2 签名）+ `quote_digest` 规范编码函数
  - [x] SubTask 2.4：内嵌测试 TPM1~TPM6
  - 验证：`cargo test -p eneros-attestation tpm` 6/6 通过
- [x] Task 3：实现 `src/event_log.rs` + `src/attest.rs`
  - [x] SubTask 3.1：`TcgEvent` / `TcgEventLog`（new/measure/replay/events/len/is_empty）
  - [x] SubTask 3.2：`PcrQuote` / `RemoteAttestation::generate` / `AttestVerifier::verify`（nonce → 验签 → 自一致性 → 期望值比对 四步）/ `AttestResult` / `AttestReason`
  - [x] SubTask 3.3：内嵌测试 LOG7~LOG9 + ATT10~ATT17 + MOCK18~MOCK19
  - 验证：`cargo test -p eneros-attestation` 21/21 通过
- [x] Task 4：集成测试 + 性能 + 配置与文档
  - [x] SubTask 4.1：INT20~INT21（端到端三级度量证明 + 攻击场景拒绝）+ PERF22（release 打印，ENEROS_PERF_GATE=1 断言 < 100ms）
  - [x] SubTask 4.2：`configs/attestation.toml`——`[tpm]`（backend="soft"，注明真实 TPM2 适配器属集成层）/ `[quote]`（pcr_indices=[0,1,2,3,4,5,6,7]）/ `[verifier]`（ak_pubkey_hex 65B 占位符，密钥不入仓）+ 中文注释 ≥7 点
  - [x] SubTask 4.3：`docs/security/attestation-design.md`——12 章节 + ≥2 Mermaid（蓝图 §4.3 sequenceDiagram 证明时序 + verify 四步 flowchart）+ D1~D12 偏差表 + 源码相对链接 `../../crates/security/attestation/src/...`
  - 验证：`cargo test -p eneros-attestation --release` 22/22 通过
- [x] Task 5：版本同步 + §2.4 全量校验
  - [x] SubTask 5.1：根 `Cargo.toml` version 0.113.0 → 0.114.0；`Makefile`（VERSION + L3 头部注释）/ `.github/workflows/ci.yml` L3 注释 / `ci/src/gate.rs` 注释串同步（沿用 v0.113.0 点位：gate.rs L144+L233 附近）
  - [x] SubTask 5.2：§2.4.2 构建校验全过——workspace 测试零回归 + aarch64-unknown-none 交叉编译 + fmt --check + clippy -D warnings + cargo deny check
  - 验证：上述命令全部零告警通过；`git status` 无垃圾文件（cargo deny advisories 因 GitHub 网络不可达跳过拉取，licenses/bans/sources 通过）
- [x] Task 6（验证后修复）：PERF 门禁判定加固——`var_os().is_some()` 对空串变量误激活，改为 `var(...).as_deref() == Ok("1")`（attest.rs PERF22 + verifier.rs PERF20 两处，后者为 v0.113.0 同源隐患一并修复）；复跑 release 22/22 + 21/21 通过、fmt 通过

# Task Dependencies

- Task 2、Task 3 依赖 Task 1（crate 骨架 + 错误类型）
- Task 3 依赖 Task 2（TpmBackend/SoftTpm 被 event_log/attest 引用）
- Task 4 依赖 Task 3（INT/PERF 基于完整实现）
- Task 5 依赖 Task 1~4（全量校验最后执行）
- 无并行项（单 crate 内聚强，串行最简）
