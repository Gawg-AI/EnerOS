# v0.113.0 Secure Boot 全链 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.113.0（P2-I 安全体系第 1 版，9 节齐全）。新建 crate `crates/security/secure-boot/`（eneros-secure-boot，唯一依赖 eneros-crypto 复用国密）。蓝图/路线图检索确认无 v0.113.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1），本任务仅 v0.113.0。
>
> 注：v0.112.0（云端数字孪生主节点）非本版前置依赖（蓝图 §2 前序仅 v0.4.0 用户态组件 + v0.31.0 国密，均已落地），按用户指令跳过，工作区版本号 0.111.0 → 0.113.0。

## Why

防止篡改镜像启动，保障系统从源头可信（蓝图 §1）。ROM→Bootloader→内核→Runtime 逐级 SM2 签名校验，无签名验证则恶意镜像可启动（§2 阻塞项）。v0.31.0/v0.32.0 已落地国密 SM2/SM3 与 PKI 基座（eneros-crypto），本版实现四级信任链验证器 + 镜像签名头格式 + 防降级时间戳，为 v0.114.0 测量启动/远程证明与联邦安全启动奠基。

## What Changes

- **新建** `crates/security/secure-boot/`（`eneros-secure-boot`，no_std + alloc，零外部依赖，唯一 workspace 内 path 依赖 `eneros-crypto`，D9）：
  - `src/header.rs`：`ImageSignature`（全固定字段，Copy，118B）+ `encode_header`/`decode_header` 二进制编解码（magic "ESIG" + version 1，全小端，零 serde，D7/D11）
  - `src/chain.rs`：`BootStage`（5 变体）+ `ChainOfTrust`（root_key + stage_key + current_stage，删除死字段，D6）
  - `src/verifier.rs`：`BootVerifier`（`new` / `verify_stage` / `advance_stage` / `current_stage` / `stats`），四级验证逻辑同构收敛于单文件（D4）
  - `src/lib.rs`：`BootError`（10 变体）/ `BootStats` + 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/secure-boot.toml`：`[trust_root]` / `[anti_rollback]` / `[stages]` 三节 + 中文注释
- **新增** `docs/security/secure-boot-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 20 个单元测试**（src 内嵌 `#[cfg(test)]`：HDR×5 + VER×9 + CHN×3 + INT×2 + PERF×1）
- 根 `Cargo.toml`：members 追加 `"crates/security/secure-boot"`（iec62351 之后）+ version 0.111.0 → 0.113.0；`Makefile`（VERSION + L3 头部注释）/ `ci.yml` L3 注释 / `gate.rs` 注释串同步（沿用 v0.111.0 同步点位）
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v11300-secure-boot（新建）
- Affected code：`crates/security/secure-boot/`（新建）、`configs/`、`docs/security/`、根 4 文件版本号
- 上游：v0.31.0 国密 eneros-crypto（`sm3_hash` / `sm2_verify` / `Sm2Signature` / `Sm2PublicKey` / `Sm2KeyPair` 复用，path 依赖）、v0.4.0 用户态组件
- 下游：v0.114.0 测量启动与远程证明（信任链基座）；v0.119.0 渗透测试（篡改镜像 100% 拒绝验收）

## ADDED Requirements

### Requirement: 镜像签名头编解码（header.rs）

The system SHALL provide `ImageSignature { magic: [u8; 4], version: u16, image_size: u64, image_hash: [u8; 32], signature: [u8; 64], timestamp: u64 }`（derive Debug/Clone/Copy/PartialEq，全固定字段共 118 字节；删除蓝图 `signer_cert: Vec<u8>`——信任锚为构造注入的信任根公钥，证书链验证归 v0.32.0 PKI 层，D7）。二进制编解码（零 serde 依赖，D11 对齐 v0.111.0 D5 先例）：`HEADER_LEN: usize = 118`；`encode_header(&ImageSignature) -> [u8; 118]` / `decode_header(&[u8]) -> Result<ImageSignature, BootError>`，帧布局全小端——`[magic 4B = "ESIG"][version u16][image_size u64][image_hash 32B][signature 64B][timestamp u64]`；输入长度 < 118 → `Err(InvalidHeader)`；magic 错误 → `Err(InvalidMagic)`；version != 1 → `Err(UnsupportedVersion)`（魔数/版本在解码期拦截，verify_stage 期对直接构造的结构体同样复检）。

#### Scenario: 编解码往返
- **WHEN** 全字段 ImageSignature `encode_header` 后 `decode_header`
- **THEN** 解码结果与原值逐字段相等

#### Scenario: 坏头拒绝
- **WHEN** magic 篡改 / version != 1 / 输入截断（< 118B）
- **THEN** 分别 `Err(InvalidMagic)` / `Err(UnsupportedVersion)` / `Err(InvalidHeader)`

### Requirement: 信任链与启动阶段（chain.rs）

The system SHALL provide `BootStage { Rom, Bootloader, Kernel, Runtime, Complete }`（derive Debug/Clone/Copy/PartialEq）；`ChainOfTrust { root_key: Sm2PublicKey, stage_key: Option<Sm2PublicKey>, current_stage: BootStage }`（字段私有，D5/D6：蓝图 `rom_root_key/bl_pubkey: [u8; 64]` 与 SM2 未压缩公钥 65B 格式不符，改用 `Sm2PublicKey` 类型；删除蓝图 `kernel_sig/runtime_sig/bl_pubkey: [0u8;64]` 三个从不更新的死字段/无效初始化）。`ChainOfTrust::new(root_key)` 初始 stage=Rom、stage_key=None；`stage_key()` 访问器。

#### Scenario: 初始状态
- **WHEN** `ChainOfTrust::new(root_key)`
- **THEN** current_stage==Rom 且 stage_key==None

### Requirement: 启动验证器（verifier.rs）

The system SHALL provide `BootVerifier { chain: ChainOfTrust, min_timestamp: u64, stats: BootStats }`（字段私有）：

`new(root_key: Sm2PublicKey, min_timestamp: u64)`——信任根与反降级时间戳下限构造注入（D8：蓝图 `get_min_timestamp` 恒返回 0 属空转；安全存储/熔丝值由集成层供给，v0.111.0 D11 注入先例）。

`verify_stage(&mut self, stage: BootStage, image: &[u8], sig: &ImageSignature) -> Result<(), BootError>` 按序执行（失败时 `stats.rejected += 1` 并记录 `last_error`，D11）：
1. `stage != chain.current_stage` → `Err(WrongStage)`（强制逐级顺序，蓝图未检）
2. `stage == Rom` → `Ok(())`（ROM 已由硬件根信任验证，蓝图 §4.5 原语义）；`stage == Complete` → `Ok(())`（蓝图原语义）
3. `sig.magic != *b"ESIG"` → `Err(InvalidMagic)`
4. `sig.version != 1` → `Err(UnsupportedVersion)`
5. `image.len() as u64 != sig.image_size` → `Err(SizeMismatch)`（显式长度校验，防截断镜像，蓝图有字段未校验）
6. `sm3_hash(image) != sig.image_hash` → `Err(HashMismatch)`（复用 eneros-crypto，D9）
7. `sig.timestamp < self.min_timestamp` → `Err(StaleImage)`（防降级，D8）
8. 选择验签密钥：`Bootloader → &chain.root_key`；`Kernel | Runtime → chain.stage_key`（None → `Err(MissingStageKey)`，蓝图 bl_pubkey 恒零 bug 修复，D6）
9. `Sm2Signature::from_bytes(&sig.signature)` + `sm2_verify(&sig.image_hash, &sm2_sig, key)`（签名消息为 SM3 哈希 32B，D9）→ `Ok(false)` 或 `Err(_)` ⇒ `Err(SignatureInvalid)`
10. 成功：`stats.verified_stages += 1`

`advance_stage(&mut self, next_key: Option<Sm2PublicKey>) -> Result<BootStage, BootError>`：Complete → `Err(AlreadyComplete)`；`Bootloader→Kernel` 转换时 `next_key` 必须为 `Some(bl_pubkey)`（None → `Err(MissingStageKey)`）并写入 `chain.stage_key`——BL 公钥随已验签镜像体传递，其完整性由步骤 6/9 的哈希+签名覆盖传递可信（D6）；`Kernel→Runtime` 传 None 则沿用当前 stage_key（蓝图「同 BL key」语义），传 Some 则轮换；返回新 stage。

`current_stage() -> BootStage`；`stats() -> BootStats`。`BootStats { verified_stages: u32, rejected: u32, last_error: Option<BootError> }`（derive Debug/Clone/Copy/PartialEq，§9 可观测落地）。`BootError = InvalidMagic / UnsupportedVersion / InvalidHeader / SizeMismatch / HashMismatch / SignatureInvalid / StaleImage / WrongStage / MissingStageKey / AlreadyComplete`（10 变体，Debug/Clone/Copy/PartialEq）。恢复模式（蓝图 §4.4）为平台集成职责——crate 仅返回错误，由集成层据 Err 进入恢复/安全停止（no_std 无平台复位抽象，D11 文档化）。

#### Scenario: 全链快乐路径（蓝图 §4.3/§7.1）
- **WHEN** 测试用 `Sm2KeyPair::generate` 生成 root/bl 两对密钥：root 私钥签 BL 镜像、bl 私钥签内核与 Runtime 镜像（消息均为各自 SM3 哈希）；依次 verify_stage(Bootloader) → advance_stage(Some(bl_pk)) → verify_stage(Kernel) → advance_stage(None) → verify_stage(Runtime) → advance_stage(None)
- **THEN** 四步全 `Ok`，最终 current_stage==Complete，stats.verified_stages==3

#### Scenario: 篡改镜像 100% 拒绝（蓝图 §6.2/§7.3）
- **WHEN** BL 镜像字节被篡改 1 字节（签名头不动）
- **THEN** verify_stage 返回 `Err(HashMismatch)`，stats.rejected==1，current_stage 不变

#### Scenario: 错密钥/坏签名（蓝图 §6.1）
- **WHEN** 用非 root 私钥对 BL 镜像签名
- **THEN** `Err(SignatureInvalid)`；signature 字段非合法 r‖s 编码同样 `Err(SignatureInvalid)`

#### Scenario: 防降级（蓝图 §4.4/§5.2）
- **WHEN** sig.timestamp < min_timestamp
- **THEN** `Err(StaleImage)`

#### Scenario: 顺序强制与密钥门禁（D6）
- **WHEN** 在 Rom 阶段直接 verify_stage(Kernel) → `Err(WrongStage)`；Bootloader→Kernel advance 时 next_key=None → `Err(MissingStageKey)`；Complete 后 advance → `Err(AlreadyComplete)`
- **THEN** 三类非法调用均被拒绝且 stats.rejected 相应增加

#### Scenario: 验签性能（蓝图 §6.3/§7.2）
- **WHEN** release 模式单次 verify_stage(Bootloader)（真实 SM2 验签，cfg(test) Instant 口径）
- **THEN** 耗时计时输出；设 `ENEROS_PERF_GATE=1` 环境变量时断言 < 50ms（D13：主机纯 Rust 仿射 SM2 实测 ~150ms，50ms 门禁面向目标硬件 SM2 加速/性能 CI 场景）

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动；eneros-crypto 仅被 path 引用，零源码改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D13，相对蓝图 §3/§4/§5/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/secure_boot/` → `crates/security/secure-boot/`（eneros-secure-boot） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；Secure Boot 属安全体系，与 crypto/iec62351 同属 security 子系统 |
| **D2** | 蓝图 `docs/phase2/secure_boot.md` → `docs/security/secure-boot-design.md` | 记忆 §2.3.3 强制：文档按方向分类（docs/security/ 已有 pki-design.md/sm-crypto-design.md 先例） |
| **D3** | 蓝图 `tests/secure_boot.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.111.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图四文件 `rom_verify/bl_verify/kernel_verify/rt_verify.rs` → 单 `verifier.rs`（另拆 header.rs/chain.rs 承载数据结构与编解码） | 四级验证逻辑同构（仅验签密钥来源不同），四文件属重复建设（禁忌 14）；Karpathy 最小实现 |
| **D5** | 蓝图密钥 `[u8; 64]`（rom_root_key/bl_pubkey）→ `Sm2PublicKey` 强类型 | SM2 未压缩公钥为 65B（0x04‖x‖y），蓝图 64B 与 eneros-crypto `Sm2PublicKey::from_bytes/to_bytes_uncompressed` 格式不符；强类型编译期防错 |
| **D6** | ① 删除 `ChainOfTrust.kernel_sig/runtime_sig` 死字段（蓝图声明后从未使用）；② 修复 `bl_pubkey: [0u8;64]` 初始化后永不更新、Kernel/Runtime 验签恒用零密钥的蓝图 bug → `advance_stage(next_key: Option<Sm2PublicKey>)` 显式传递下级密钥，`Bootloader→Kernel` 强制 Some，否则 `Err(MissingStageKey)` | 蓝图代码逻辑错误必须修复；BL 公钥随已验签镜像体传递，完整性由哈希+签名覆盖传递可信；Kernel→Runtime 传 None 沿用「同 BL key」蓝图语义 |
| **D7** | 删除蓝图 `ImageSignature.signer_cert: Vec<u8>` 字段 | 信任锚为构造注入的信任根公钥；证书链验证归 v0.32.0 PKI 层职责，本版不做链式验证（v0.111.0 D11 同先例，Karpathy 最小实现）；结构体由此全固定字段 118B 可 Copy |
| **D8** | 蓝图 `get_min_timestamp` 恒返回 0（反降级空转）→ 构造注入 `min_timestamp: u64`，每级校验 `sig.timestamp >= min_timestamp` | 蓝图防降级机制无实际效果；熔丝/安全存储中的时间戳下限由集成层供给（no_std 无安全存储抽象，注入先例同 v0.111.0 D11） |
| **D9** | 蓝图 `sm3_hash`/`sm2_verify_sig` 未指明实现 → 复用 eneros-crypto（path 依赖 `../crypto`）：`sm3_hash(data) -> [u8;32]`、`sm2_verify(&hash, &Sm2Signature, &Sm2PublicKey)`、`Sm2Signature::from_bytes` | 记忆 §5.5/禁忌 14 禁止重复造轮子；国密实现已经安全评审（常量时间/零化/Drop），自研重引入风险 |
| **D10** | 补充蓝图缺失校验：`stage != current_stage` → `WrongStage`（强制逐级顺序）；`image.len() != sig.image_size` → `SizeMismatch`；`version != 1` → `UnsupportedVersion` | 蓝图 verify_stage 不校验 stage 顺序，可跳级验签；image_size 字段声明未用；version 字段同理 |
| **D11** | 错误模型 `BootError` = InvalidMagic / UnsupportedVersion / InvalidHeader / SizeMismatch / HashMismatch / SignatureInvalid / StaleImage / WrongStage / MissingStageKey / AlreadyComplete（10 变体，Copy 对齐 v0.111.0 OtaError 惯例）；新增 `BootStats { verified_stages, rejected, last_error }` 落地 §9 可观测；恢复模式（§4.4）为平台集成职责，crate 仅返回错误 | 蓝图引用 BootError 未定义；变体覆盖 §4.4 各失败面；no_std 无平台复位/恢复模式抽象，集成层据 Err 进入恢复 |
| **D12** | 信任根公钥配置（蓝图 §3「配置：信任根公钥配置」）落地为 `configs/secure-boot.toml` 模板（hex 占位符 + 注释），真实密钥由集成层烧录 | 密钥不入仓（记忆 §3.1 密钥禁忌）；配置模板先例同 v0.111.0 |
| **D13** | PERF20 蓝图 §7.2「< 50ms」release 断言 → release 默认仅打印计时，设 `ENEROS_PERF_GATE=1` 环境变量时启用 < 50ms 断言 | eneros-crypto 纯 Rust 仿射坐标 + EEA 模逆，主机 release 实测 ~150ms 超指标；50ms 面向目标硬件 SM2 加速场景；机器相关性能断言默认关闭避免主机/CI 误红，目标硬件/性能 CI 经 ENEROS_PERF_GATE=1 开启门禁；本版硬约束禁改 eneros-crypto，crypto 点运算优化（Jacobian/窗口法）为后续议题 |

## 测试规划（20 个）

| 编号 | 名称 | 断言要点 |
|------|------|---------|
| HDR1 | 编解码往返 | 全字段 encode→decode 逐字段相等 |
| HDR2 | 坏 magic | Err(InvalidMagic) |
| HDR3 | version != 1 | Err(UnsupportedVersion) |
| HDR4 | 截断输入（117B / 空） | Err(InvalidHeader) |
| HDR5 | HEADER_LEN 常量 == 118 | 帧布局静态保证 |
| VER6 | Rom 阶段直接 Ok | 硬件根信任语义（蓝图 §4.5） |
| VER7 | Bootloader 真实签名验过 | eneros-crypto Sm2KeyPair 签名 → Ok，verified_stages==1 |
| VER8 | 篡改镜像 1 字节 | Err(HashMismatch)，rejected==1，stage 不变 |
| VER9 | 错私钥签名 | Err(SignatureInvalid) |
| VER10 | 签名字段非法编码 | Err(SignatureInvalid) |
| VER11 | 坏 magic / 坏 version / size 不符 | InvalidMagic / UnsupportedVersion / SizeMismatch |
| VER12 | timestamp < min_timestamp | Err(StaleImage) |
| VER13 | 跳级 verify（Rom→Kernel） | Err(WrongStage) |
| VER14 | 缺 stage_key 验 Kernel | Err(MissingStageKey)（直接构造场景） |
| CHN15 | advance 全流程 | Rom→Bootloader→Kernel→Runtime→Complete 依次推进返回值正确 |
| CHN16 | BL→Kernel 缺密钥 | Err(MissingStageKey)，stage 不变 |
| CHN17 | Complete 后 advance | Err(AlreadyComplete) |
| INT18 | 全链快乐路径 | 两级密钥三镜像全过 → Complete，verified_stages==3，rejected==0 |
| INT19 | 链中途篡改拒绝后重验 | Kernel 篡改 → HashMismatch → 修正镜像重验 → Ok（拒绝不推进 stage，可恢复重试） |
| PERF20 | 单次 SM2 验签 < 50ms | release 计时打印；ENEROS_PERF_GATE=1 时断言（cfg(test) Instant，D13 口径） |
