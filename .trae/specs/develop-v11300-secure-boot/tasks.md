# Tasks — v0.113.0 Secure Boot 全链

> Spec：`.trae/specs/develop-v11300-secure-boot/spec.md`。蓝图：`蓝图/phase2.md` §v0.113.0。
> 全局约束：no_std（禁 `std::*`/`panic!`/`todo!`/`unimplemented!`，子模块不重复 `#![no_std]` 属性，lib.rs 统一声明）；代码注释中文；复用 eneros-crypto（path = "../crypto"）禁重复造轮子；`Sm2PublicKey` 不 derive Debug 时按 crypto 既有惯例处理（参考 iec62351 用法）。

- [x] Task 1：新建 crate 骨架 `crates/security/secure-boot/`（eneros-secure-boot）
  - [x] SubTask 1.1：`Cargo.toml`——name=eneros-secure-boot，version/edition/authors/license workspace 继承，description 含 "v0.113.0"；`[dependencies] eneros-crypto = { path = "../crypto" }`（对齐 iec62351 模板）
  - [x] SubTask 1.2：`src/lib.rs`——`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`（如需要）；`BootError`（10 变体：InvalidMagic/UnsupportedVersion/InvalidHeader/SizeMismatch/HashMismatch/SignatureInvalid/StaleImage/WrongStage/MissingStageKey/AlreadyComplete，derive Debug/Clone/Copy/PartialEq）；`BootStats`（verified_stages/rejected/last_error，derive Debug/Clone/Copy/PartialEq）；模块声明 + 重导出
  - [x] SubTask 1.3：根 `Cargo.toml` members 追加 `"crates/security/secure-boot"`（置于 `"crates/security/iec62351"` 之后）
  - 验证：`cargo metadata --format-version 1 > NUL` 成功
- [x] Task 2：实现 `src/header.rs`（ImageSignature + 编解码）
  - [x] SubTask 2.1：`ImageSignature` 结构体（magic [u8;4] / version u16 / image_size u64 / image_hash [u8;32] / signature [u8;64] / timestamp u64，derive Debug/Clone/Copy/PartialEq）+ `HEADER_LEN: usize = 118` 常量
  - [x] SubTask 2.2：`encode_header(&ImageSignature) -> [u8; HEADER_LEN]` / `decode_header(&[u8]) -> Result<ImageSignature, BootError>`（全小端；<118B → InvalidHeader；magic≠"ESIG" → InvalidMagic；version≠1 → UnsupportedVersion）
  - [x] SubTask 2.3：内嵌测试 HDR1~HDR5（往返/坏 magic/坏 version/截断/HEADER_LEN==118）
  - 验证：`cargo test -p eneros-secure-boot header` 5/5 通过
- [x] Task 3：实现 `src/chain.rs` + `src/verifier.rs`（信任链 + 四级验证器）
  - [x] SubTask 3.1：`BootStage`（5 变体，derive Debug/Clone/Copy/PartialEq）；`ChainOfTrust`（root_key/stage_key/current_stage 私有 + `new(root_key)` + 访问器）
  - [x] SubTask 3.2：`BootVerifier::new(root_key: Sm2PublicKey, min_timestamp: u64)`；`verify_stage` 按 spec 10 步顺序实现（WrongStage → Rom/Complete 直通 → InvalidMagic → UnsupportedVersion → SizeMismatch → HashMismatch（sm3_hash）→ StaleImage → 选钥（Bootloader=root_key，Kernel/Runtime=stage_key，None → MissingStageKey）→ Sm2Signature::from_bytes + sm2_verify → false/Err ⇒ SignatureInvalid）；失败路径 stats.rejected+=1 且记录 last_error，成功 verified_stages+=1
  - [x] SubTask 3.3：`advance_stage(next_key: Option<Sm2PublicKey>)`（Complete → AlreadyComplete；Bootloader→Kernel 强制 Some 否则 MissingStageKey；Kernel→Runtime None 沿用）；`current_stage()`；`stats()`
  - [x] SubTask 3.4：内嵌测试 VER6~VER14 + CHN15~CHN17 + INT18~INT19（真实 Sm2KeyPair::generate + sm2_sign 构造签名；篡改/错钥/坏编码/坏头/防降级/跳级/缺钥/全流程/中途拒绝重验）
  - 验证：`cargo test -p eneros-secure-boot` 19/19 通过
- [x] Task 4：性能测试 PERF20 + 配置与文档
  - [x] SubTask 4.1：PERF20——release 模式单次 verify_stage(Bootloader) 真实 SM2 验签计时（cfg(test) Instant 口径；debug 仅打印；release 默认打印，ENEROS_PERF_GATE=1 时断言 < 50ms，D13）
  - [x] SubTask 4.2：`configs/secure-boot.toml`——`[trust_root]`（root_pubkey_hex 65B 未压缩点 hex 占位符）/ `[anti_rollback]`（min_timestamp）/ `[stages]`（四级说明）+ 中文注释 ≥7 点
  - [x] SubTask 4.3：`docs/security/secure-boot-design.md`——12 章节（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/实现偏差 D1~D13/集成指引/参考资料）+ ≥2 Mermaid（信任链 flowchart + 验证流程 sequence）+ D1~D13 偏差表；明确「恢复模式为集成层职责」「密钥不入仓」
  - 验证：`cargo test -p eneros-secure-boot --release` 21/21 通过
- [x] Task 5：版本同步 + §2.4 全量校验
  - [x] SubTask 5.1：根 `Cargo.toml` version 0.111.0 → 0.113.0；`Makefile`（VERSION 变量 + L3 头部注释）/ `.github/workflows/ci.yml` L3 注释 / `ci/src/gate.rs` 注释串同步（先检索 0.111.0/0.110.0 字样定位沿用点位）
  - [x] SubTask 5.2：§2.4.2 构建校验全过——`cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（零回归）+ `cargo build -p eneros-secure-boot --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` + `cargo fmt --all -- --check` + `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` + `cargo deny check advisories licenses bans sources`
  - 验证：上述命令全部零告警通过；`git status` 无垃圾文件

# Task Dependencies

- Task 2、Task 3 依赖 Task 1（crate 骨架 + BootError 定义）
- Task 3 依赖 Task 2（header 类型被 verifier 引用）
- Task 4 依赖 Task 3（PERF 基于完整 verifier）
- Task 5 依赖 Task 1~4（全量校验最后执行）
- 无并行项（单 crate 内聚强，串行最简）
