# Checklist — v0.114.0 测量启动与远程证明

## 功能正确性（蓝图 §7.1/§7.3）

- [x] `PcrBank` 24 个 PCR 全零初始，SM3-only 单 bank（无 HashAlgorithm 枚举）
- [x] `pcr_extend_value(current, digest) == sm3(current‖digest)`，SoftTpm/measure/replay 三方共用
- [x] `TpmBackend` trait 四方法（pcr_extend/pcr_read/quote/attestation_pubkey），sync 无 Send+Sync 要求
- [x] `SoftTpm`：new(CsRng) 生成 AK；inject_failure 故障注入；pcr_idx ≥ 24 → InvalidPcrIndex
- [x] `quote` 返回 (PcrQuote, [u8;64]) 二元组（修复蓝图签名永不填充 bug）；空 pcr_indices → EmptyPcrSelection
- [x] `PcrQuote` 内嵌 nonce [u8;20]（随 quote_digest 签名绑定防重放）
- [x] `quote_digest` 规范编码：pcr_count + idx 列表 + values + nonce + quote_time LE
- [x] `TcgEventLog::measure` = sm3(data) → pcr_extend → 追挂事件；错误显式传播无吞错
- [x] `replay` 从零值链式重放，与 SoftTpm bank 一致
- [x] `RemoteAttestation::generate` 组装 quote + 64B 签名 + 日志克隆
- [x] `AttestVerifier::verify` 四步顺序：NonceMismatch → SignatureInvalid → EventLogInconsistent → PcrMismatch → Verified
- [x] PcrMismatch 时 pcr_mismatches 记录全部不匹配索引
- [x] `AttestResult.reason` 为 AttestReason 枚举（非 String）；`AttestStats` 随 verify 更新
- [x] `AttestTransport` + `MockAttestTransport`（故障注入 + calls 计数）
- [x] 无 extern "C"/unsafe/NonNull（TPM FFI 移除，D4）

## 测试（22 个全过）

- [x] TPM1~TPM6 通过
- [x] LOG7~LOG9 通过
- [x] ATT10~ATT17 通过
- [x] MOCK18~MOCK19 通过
- [x] INT20~INT21 通过（含攻击场景 PcrMismatch 拒绝信任）
- [x] PERF22：release 打印耗时；ENEROS_PERF_GATE=1 断言 < 100ms（D12 口径）

## no_std 与依赖合规（记忆 §4.3/§5.5）

- [x] 无 `use std::*`（测试模块除外）；无 `panic!`/`todo!`/`unimplemented!`；子模块不重复 `#![no_std]`
- [x] 唯一依赖 `eneros-crypto = { path = "../crypto" }`；零外部 crates.io 依赖
- [x] 未自研 SM2/SM3（全部复用 eneros-crypto 公开 API）

## 目录结构（记忆 §2.4.1）

- [x] C1：crate 位于 `crates/security/attestation/`，未放根目录
- [x] C2：根 `Cargo.toml` members 已添加 `"crates/security/attestation"`（secure-boot 之后）
- [x] C3：`path = "../crypto"` 相对路径正确
- [x] C4：文档位于 `docs/security/attestation-design.md`，未平面化
- [x] C5：根目录无新 crate 文件夹

## 构建校验（记忆 §2.4.2）

- [x] C6：`cargo metadata` 成功
- [x] C7：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 零回归
- [x] C8：`cargo build -p eneros-attestation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9：`cargo fmt --all -- --check` 通过
- [x] C10：`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 零告警
- [x] C11：`cargo deny check advisories licenses bans sources` 通过

## 文档与规范（记忆 §2.4.3）

- [x] C12：`docs/security/attestation-design.md` 12 章节 + ≥2 Mermaid + D1~D12 偏差表
- [x] C13：`git status` 无 target/、*.elf、*.bin、IDE 缓存被追踪
- [x] C14：无新文件类型需补 .gitignore
- [x] C15：提交信息遵循 Conventional Commits（feat(security/attestation): v0.114.0 ...）
- [x] `configs/attestation.toml` 三节齐全 + 中文注释；AK 公钥仅占位符，密钥不入仓
- [x] 版本同步：根 Cargo.toml 0.114.0 + Makefile + ci.yml + gate.rs 四处一致
