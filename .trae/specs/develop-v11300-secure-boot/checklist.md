# Checklist — v0.113.0 Secure Boot 全链

## 功能正确性（蓝图 §7.1/§7.3）

- [x] `ImageSignature` 全固定字段（magic/version/image_size/image_hash/signature/timestamp），118B，derive Debug/Clone/Copy/PartialEq
- [x] `encode_header`/`decode_header` 全小端；截断 → InvalidHeader；坏 magic → InvalidMagic；version≠1 → UnsupportedVersion
- [x] `BootStage` 5 变体（Rom/Bootloader/Kernel/Runtime/Complete）
- [x] `ChainOfTrust` 无 kernel_sig/runtime_sig 死字段；无 [0u8;64] 零密钥初始化
- [x] `BootVerifier::new(root_key: Sm2PublicKey, min_timestamp: u64)` 构造注入
- [x] `verify_stage` 顺序校验：stage 不符 → WrongStage；Rom/Complete 直通 Ok
- [x] image.len() ≠ sig.image_size → SizeMismatch
- [x] sm3_hash(image) ≠ sig.image_hash → HashMismatch（复用 eneros-crypto）
- [x] sig.timestamp < min_timestamp → StaleImage（防降级生效，非空转）
- [x] Bootloader 用 root_key 验签；Kernel/Runtime 用 stage_key 验签；stage_key None → MissingStageKey
- [x] Sm2Signature::from_bytes + sm2_verify 失败 → SignatureInvalid
- [x] `advance_stage`：Complete → AlreadyComplete；BL→Kernel 缺钥 → MissingStageKey；Kernel→Runtime None 沿用当前钥
- [x] `BootStats`（verified_stages/rejected/last_error）随成功/失败正确更新
- [x] 全链快乐路径：root 签 BL → bl 签内核/Runtime → 四步推进至 Complete
- [x] 篡改镜像 100% 拒绝（HashMismatch/SignatureInvalid），拒绝后 stage 不推进可重验

## 测试（20 个全过）

- [x] HDR1~HDR5 通过
- [x] VER6~VER14 通过
- [x] CHN15~CHN17 通过
- [x] INT18~INT19 通过
- [x] PERF20：release 模式单次 SM2 验签计时打印；ENEROS_PERF_GATE=1 时断言 < 50ms（蓝图 §6.3/§7.2，D13）

## no_std 与依赖合规（记忆 §4.3/§5.5）

- [x] 无 `use std::*`；无 `panic!`/`todo!`/`unimplemented!`；子模块不重复 `#![no_std]`
- [x] 唯一依赖 `eneros-crypto = { path = "../crypto" }`；零外部 crates.io 依赖
- [x] 未自研 SM2/SM3（全部复用 eneros-crypto 公开 API）

## 目录结构（记忆 §2.4.1）

- [x] C1：crate 位于 `crates/security/secure-boot/`，未放根目录
- [x] C2：根 `Cargo.toml` members 已添加 `"crates/security/secure-boot"`（iec62351 之后）
- [x] C3：`path = "../crypto"` 相对路径正确
- [x] C4：文档位于 `docs/security/secure-boot-design.md`，未平面化
- [x] C5：根目录无新 crate 文件夹

## 构建校验（记忆 §2.4.2）

- [x] C6：`cargo metadata` 成功
- [x] C7：`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 零回归
- [x] C8：`cargo build -p eneros-secure-boot --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9：`cargo fmt --all -- --check` 通过
- [x] C10：`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 零告警
- [x] C11：`cargo deny check advisories licenses bans sources` 通过

## 文档与规范（记忆 §2.4.3）

- [x] C12：`docs/security/secure-boot-design.md` 12 章节 + ≥2 Mermaid + D1~D13 偏差表
- [x] C13：`git status` 无 target/、*.elf、*.bin、IDE 缓存被追踪
- [x] C14：无新文件类型需补 .gitignore
- [x] C15：提交信息遵循 Conventional Commits（feat(security/secure-boot): v0.113.0 ...）
- [x] `configs/secure-boot.toml` 三节齐全 + 中文注释；真实密钥不入仓（占位符）
- [x] 版本同步：根 Cargo.toml 0.113.0 + Makefile + ci.yml + gate.rs 四处一致
