# v0.114.0 测量启动与远程证明 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.114.0（P2-I 安全体系第 2 版，9 节齐全）。新建 crate `crates/security/attestation/`（eneros-attestation，唯一依赖 eneros-crypto 复用国密）。蓝图/路线图检索确认无 v0.114.x 刚性子版本，本任务仅 v0.114.0。

## Why

远程可验证系统完整性，建立联邦信任基础（蓝图 §1）。TPM PCR 度量值 + Quote + Nonce 使验证方可远程确认边缘节点启动链未被篡改；无 TPM 则无法度量（§2 阻塞项），故蓝图 §4.4/§5.1 要求软件度量降级。v0.113.0 已落地 Secure Boot 信任链（启动时「验签」），本版落地测量启动（启动时「度量存证」）+ 远程证明（「远程可验证」），为 v0.115.0 mTLS 与联邦可信验证奠基。

## What Changes

- **新建** `crates/security/attestation/`（`eneros-attestation`，no_std + alloc，零外部依赖，唯一 workspace 内 path 依赖 `eneros-crypto`，D9）：
  - `src/tpm.rs`：`PcrBank`（SM3-only，D9）+ `TpmBackend` sync trait + `SoftTpm`（软件 TPM，落地 §4.4 降级方案 + 主机可测，D4）+ `pcr_extend_value` 共享 extend 函数（D7）
  - `src/event_log.rs`：`TcgEvent` / `TcgEventLog`（`measure` = PCR extend + 事件追挂；`replay` 重放重算 PCR，D7/D8）
  - `src/attest.rs`：`PcrQuote`（nonce 内嵌，D10）/ `RemoteAttestation`（`generate`）/ `AttestVerifier`（本地验证器：nonce + SM2 验签 + 期望值重放比对 + 自一致性，D6）/ `AttestResult` + `AttestReason`
  - `src/lib.rs`：`TpmError`（5 变体）/ `AttestError`（6 变体）/ `AttestTransport` trait + `MockAttestTransport`（D5）/ `AttestStats` + 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/attestation.toml`：`[tpm]` / `[quote]` / `[verifier]` 三节 + 中文注释
- **新增** `docs/security/attestation-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 22 个单元测试**（src 内嵌 `#[cfg(test)]`：TPM×6 + LOG×3 + ATT×8 + MOCK×2 + INT×2 + PERF×1）
- 根 `Cargo.toml`：members 追加 `"crates/security/attestation"`（secure-boot 之后）+ version 0.113.0 → 0.114.0；`Makefile`（VERSION + L3 头部注释）/ `ci.yml` L3 注释 / `gate.rs` 注释串同步（沿用 v0.113.0 同步点位）
- **无 BREAKING**：纯新增 crate，既有 crate 零改动（不依赖 eneros-secure-boot，仅概念上游）

## Impact

- Affected specs：develop-v11400-attestation（新建）
- Affected code：`crates/security/attestation/`（新建）、`configs/`、`docs/security/`、根 4 文件版本号
- 上游：v0.113.0 Secure Boot（概念上游：被度量的各级镜像即其验签对象）、v0.31.0 国密 eneros-crypto（`sm3_hash` / `sm2_sign` / `sm2_verify` / `Sm2KeyPair` / `CsRng` 复用，path 依赖）
- 下游：v0.115.0 mTLS（证明结果作为通道建立前置）；联邦可信验证（v0.97.0~v0.101.0 跨域场景）

## ADDED Requirements

### Requirement: TPM 抽象与软件 TPM（tpm.rs）

The system SHALL provide `PcrBank { pcr_values: [[u8; 32]; 24] }`（SM3-only 单 bank，D9；derive Debug/Clone/Copy/PartialEq，初始全零）；`pcr_extend_value(current: &[u8; 32], digest: &[u8; 32]) -> [u8; 32]` 共享函数 = `sm3(current ‖ digest)`（TCG extend 语义，D7，measure/replay/SoftTpm 三方共用防分叉）。

`TpmBackend` trait（sync，no_std 单线程惯例，D4）：`pcr_extend(&mut self, pcr_idx: u8, digest: &[u8; 32]) -> Result<(), TpmError>`；`pcr_read(&self, pcr_idx: u8) -> Result<[u8; 32], TpmError>`；`quote(&mut self, pcr_indices: &[u8], nonce: &[u8; 20], now: u64) -> Result<(PcrQuote, [u8; 64]), TpmError>`（返回 quote + 签名二元组，修复蓝图签名永不填充 bug，D6）；`attestation_pubkey(&self) -> Sm2PublicKey`。pcr_idx ≥ 24 → `Err(InvalidPcrIndex)`。

`SoftTpm { bank: PcrBank, ak: Sm2KeyPair, fail_remaining: u32 }`（字段私有，D4 软件降级 + 故障注入）：`new(rng: &mut CsRng)`（生成 AK）；`inject_failure(count: u32)`；所有方法在 fail_remaining > 0 时递减并返回 `Err(TpmUnavailable)`；`pcr_extend` 用 `pcr_extend_value` 更新本地 bank；`quote`：pcr_indices 为空 → `Err(EmptyPcrSelection)`；逐个 pcr_read 收集值（错误显式传播，禁蓝图 `unwrap_or([0u8;32])` 吞错，D10）；构造 `PcrQuote { pcr_select: pcr_indices.to_vec(), pcr_values, nonce: *nonce, quote_time: now }`；计算 `quote_digest(&quote)` 并用 AK 私钥 `sm2_sign` 签名返回 64B 签名。PCR 仅随新实例复位（§8.5 坑点文档化）。

`quote_digest(quote: &PcrQuote) -> [u8; 32]`：SM3 over 规范编码（`pcr_count u8 ‖ 每 pcr_idx u8 ‖ 每 pcr_value 32B ‖ nonce 20B ‖ quote_time u64 LE`），签名绑定 nonce 防重放（D6/D10）。

#### Scenario: extend 确定性（蓝图 §6.1）
- **WHEN** 新 SoftTpm 对 PCR0 extend digest D
- **THEN** pcr_read(0) == sm3([0u8;32] ‖ D)；再 extend D 一次 == sm3(sm3(0‖D) ‖ D)（链式非幂等）

#### Scenario: 越界与故障注入（蓝图 §6.5）
- **WHEN** pcr_idx=24 → `Err(InvalidPcrIndex)`；inject_failure(1) 后任意操作
- **THEN** 返回 `Err(TpmUnavailable)` 且后续恢复正常

### Requirement: TCG 事件日志（event_log.rs）

The system SHALL provide `TcgEvent { pcr_index: u8, event_type: u32, digest: [u8; 32], event_data: Vec<u8> }`（derive Debug/Clone/PartialEq）；`TcgEventLog { events: Vec<TcgEvent> }`（字段私有）：`new()`；`measure<T: TpmBackend>(&mut self, tpm, pcr_index, event_type, data)`：`digest = sm3_hash(data)` → `tpm.pcr_extend(pcr_index, &digest)?`（错误显式传播）→ 追挂 `TcgEvent { pcr_index, event_type, digest, event_data: data.to_vec() }`；`replay(&self) -> [[u8; 32]; 24]`：从零值起对每个事件按 `pcr_extend_value` 链式重放（D7）；`events()` 访问器；`len()` / `is_empty()`。

#### Scenario: 度量即存证（蓝图 §5.2）
- **WHEN** measure(tpm, 0, EV_BL, bl_image) 后 measure(tpm, 1, EV_KERNEL, kernel_image)
- **THEN** 日志 2 事件，且 `log.replay()[0..2]` 与 `tpm.pcr_read(0)/(1)` 逐值相等

### Requirement: 远程证明生成与验证（attest.rs）

The system SHALL provide `RemoteAttestation { quote: PcrQuote, signature: [u8; 64], event_log: Vec<TcgEvent> }`（derive Debug/Clone/PartialEq；蓝图 `signature: Vec<u8>` 永不填充修复为定长 64B，D6）：`RemoteAttestation::generate<T: TpmBackend>(tpm, pcr_indices: &[u8], nonce: &[u8; 20], now: u64, log: &TcgEventLog) -> Result<Self, AttestError>` = `tpm.quote(...)` + 克隆日志事件。

`AttestVerifier { ak_pubkey: Sm2PublicKey, stats: AttestStats }`（字段私有）：`new(ak_pubkey)`；`verify(&mut self, attest: &RemoteAttestation, expected_log: &TcgEventLog, nonce: &[u8; 20]) -> AttestResult` 按序：
1. `attest.quote.nonce != *nonce` → reason=NonceMismatch，untrusted
2. `Sm2Signature::from_bytes(&attest.signature)` + `sm2_verify(&quote_digest(&attest.quote), &sig, &self.ak_pubkey)` 非 true → reason=SignatureInvalid，untrusted
3. 自一致性：用 attest.event_log 重放，选中 PCR 索引的重放值 ≠ quote.pcr_values → reason=EventLogInconsistent，untrusted（D11）
4. 期望值比对：expected_log.replay() 的选中索引值 ≠ quote.pcr_values → reason=PcrMismatch + 记录全部不匹配索引（蓝图 §4.4「PCR 重放不匹配 → 拒绝信任」）
5. 全过 → trusted=true，reason=Verified

`AttestResult { trusted: bool, pcr_mismatches: Vec<u8>, reason: AttestReason }`（derive Debug/Clone/PartialEq；蓝图 `reason: String` → 枚举，D11）。`AttestReason { Verified, NonceMismatch, SignatureInvalid, EventLogInconsistent, PcrMismatch, ServerRejected }`（derive Debug/Clone/Copy/PartialEq）。`AttestStats { quotes_verified: u32, trusted: u32, untrusted: u32, last_reason: Option<AttestReason> }` 随 verify 更新（§9 可观测，D11）+ `stats()`。

#### Scenario: 端到端可信（蓝图 §6.2/§7.1）
- **WHEN** SoftTpm measure BL/Kernel/Runtime 三级镜像 → generate(PCR[0..7], nonce) → 验证方持相同期望日志 verify
- **THEN** trusted==true、reason==Verified、mismatches 空、stats.trusted==1

#### Scenario: 篡改检测（蓝图 §4.4/§7.3）
- **WHEN** 期望日志中 Runtime 镜像被替换（攻击场景）或 nonce 不符或签名被篡改
- **THEN** 分别 reason==PcrMismatch（mismatches 含对应 PCR 索引）/ NonceMismatch / SignatureInvalid，均 untrusted

### Requirement: 远程传输抽象（lib.rs，D5）

The system SHALL provide `AttestTransport` trait（sync，D5，v0.110.0 SyncTransport/v0.111.0 OtaTransport 同先例）：`verify_remote(&mut self, attest: &RemoteAttestation) -> Result<AttestResult, AttestError>`（线上序列化/HTTP 语义归实现侧与集成层）；`MockAttestTransport { preset: Option<AttestResult>, fail_remaining: u32, pub calls: u32 }`：fail_remaining>0 递减 → `Err(TransportError)`；否则返回 preset 克隆（None → `Err(ServerRejected)`），calls+1。

`TpmError = TpmUnavailable / InvalidPcrIndex / ExtendFailed / ReadFailed / QuoteFailed`（5 变体，Debug/Clone/Copy/PartialEq；删除蓝图 C 返回码 payload——FFI 已移除，D4）；`AttestError = TpmUnavailable / InvalidPcrIndex / QuoteFailed / EmptyPcrSelection / TransportError / ServerRejected`（6 变体，同 derive；`From<TpmError>` 转换实现）。

#### Scenario: 远程通道故障（蓝图 §4.4）
- **WHEN** MockAttestTransport 注入 1 次失败后放行
- **THEN** 首次 verify_remote `Err(TransportError)`，第二次返回 preset 结果，calls==2

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动；eneros-crypto 仅被 path 引用，零源码改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/attestation/` → `crates/security/attestation/`（eneros-attestation） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；远程证明属安全体系，与 crypto/iec62351/secure-boot 同属 security 子系统 |
| **D2** | 蓝图 `docs/phase2/attestation.md` → `docs/security/attestation-design.md` | 记忆 §2.3.3 强制：文档按方向分类（docs/security/ 已有 secure-boot-design.md 等先例） |
| **D3** | 蓝图 `tests/attestation.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.113.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 extern "C" TPM FFI（tpm2_initialize/pcr_extend/pcr_read/quote + NonNull + unsafe + Drop shutdown）→ `TpmBackend` sync trait + `SoftTpm` 软件 TPM | 主机无 TPM 硬件不可测；no_std 阶段无 C 库链接；蓝图 §4.4/§5.1 本就要求「软件度量降级」——SoftTpm 即该降级方案的一等实现；真实 TPM2 FFI 适配器在集成层实现同一 trait；无 unsafe/NonNull |
| **D5** | 蓝图 `async verify_remote(server_url)` + HttpClient + serde_json → `AttestTransport` sync trait（`verify_remote(&RemoteAttestation)`）+ `MockAttestTransport` | no_std 无 async runtime/无 std::net（v0.110.0 D4 / v0.111.0 D4 同先例）；线上格式/HTTP 归集成层；本地验证逻辑由 AttestVerifier 承载可独立测试 |
| **D6** | 蓝图 `quote()` 不返回签名、`RemoteAttestation.signature = Vec::new()`（注释「TPM 签名」）永不填充的 bug → `quote()` 返回 `(PcrQuote, [u8; 64])`，SoftTpm 用内置 AK（SM2 密钥对）对 `quote_digest` 签名；验签归 `AttestVerifier` | 无签名的 Quote 不可远程证明（核心功能缺失）；蓝图 `signature: Vec<u8>` 修复为定长 64B |
| **D7** | 蓝图 `sm3_hash_concat` 未定义 → `pcr_extend_value(current, digest) = sm3(current ‖ digest)` 共享函数 | TCG PC Client 标准 extend 语义；SoftTpm/measure/replay 三方共用同一函数防实现分叉（v0.110.0 D11 CRC32 共享先例） |
| **D8** | 蓝图 `current_time_ms()` / `load_event_log()` 未定义全局函数 → `now: u64` 参数注入 + `TcgEventLog` 显式持有传递 | no_std 无系统时间/无全局状态（v0.110.0 D7、v0.111.0 D8 同先例）；集成层由 v0.12.0 RTC 供给时间 |
| **D9** | 蓝图 `HashAlgorithm { Sha256, Sm3 }` + `selected_banks: Vec<HashAlgorithm>` → SM3-only 单 bank（删除枚举与 Vec） | eneros-crypto 纯国密无 SHA-256（信创 §5.6 全程国密）；v0.111.0 D6 RsaSha256 占位同先例——不支持即不建模 |
| **D10** | ① 蓝图 `nonce.try_into().unwrap_or([0u8; 20])` 静默回退 → nonce 固定 `[u8; 20]` 参数，嵌入 PcrQuote 随 quote_digest 签名绑定；② 蓝图 `pcr_read(idx).unwrap_or([0u8; 32])` 吞错 → 显式错误传播；③ 蓝图 quote mask 位移未校验 idx → pcr_idx ≥ 24 返回 InvalidPcrIndex | 安全关键路径禁止静默默认值与吞错（v0.111.0 D11 同原则）；nonce 嵌入 quote 使签名显式覆盖防重放 |
| **D11** | 蓝图 `AttestResult.reason: String` → `AttestReason` 6 变体枚举（Verified/NonceMismatch/SignatureInvalid/EventLogInconsistent/PcrMismatch/ServerRejected，Copy）；新增事件日志自一致性检查（重放 attest.event_log 比对 quote）；新增 `AttestStats`（quotes_verified/trusted/untrusted/last_reason）落地 §9 可观测 | no_std Copy 错误模型对齐 v0.111.0/v0.113.0 惯例；String 理由不利机器审计；自一致性检查防止证明方提交与 quote 不符的日志 |
| **D12** | 性能「Quote < 100ms」（§6.3/§7.2）落地为 release 模式打印 + `ENEROS_PERF_GATE=1` 环境变量断言门禁 | v0.113.0 D13 已确立先例：主机纯 Rust SM2 实测超目标硬件指标（验签 161~214ms），目标硬件 SM2 加速后方可达标；口径文档化于设计文档 §7 |

## 测试规划（22 个）

| 编号 | 名称 | 断言要点 |
|------|------|---------|
| TPM1 | SoftTpm 初始 PCR 全零 | pcr_read(0..23) 全 [0u8;32] |
| TPM2 | extend 确定性 | == sm3(0‖digest) |
| TPM3 | extend 链式非幂等 | 两次 extend == sm3(sm3(0‖D)‖D) |
| TPM4 | pcr_idx=24 越界 | Err(InvalidPcrIndex) |
| TPM5 | 故障注入 | inject_failure(1) → Err(TpmUnavailable)，后续恢复 |
| TPM6 | quote 返回签名可验 | quote_digest + AK 公钥 sm2_verify == true；nonce/时间戳/选中值正确 |
| LOG7 | measure 追加 + extend | 日志 1 事件且 tpm bank 更新 |
| LOG8 | 空日志 replay 全零 | replay() == [[0u8;32];24] |
| LOG9 | replay == SoftTpm bank | 同一串 measure 后逐值相等 |
| ATT10 | generate 组装 | quote 选中 [0..7] + 64B 签名 + 日志克隆 |
| ATT11 | verify 快乐路径 | trusted + Verified + stats.trusted==1 |
| ATT12 | nonce 不符 | untrusted + NonceMismatch |
| ATT13 | 签名篡改 1 字节 | untrusted + SignatureInvalid |
| ATT14 | 期望日志多一事件 | untrusted + PcrMismatch + mismatches 索引正确 |
| ATT15 | 证明日志自一致性破坏 | untrusted + EventLogInconsistent |
| ATT16 | 错误 AK 公钥 | untrusted + SignatureInvalid |
| ATT17 | 空 pcr_indices | Err(EmptyPcrSelection) |
| MOCK18 | Mock 返回预设结果 | result 一致 + calls==1 |
| MOCK19 | 传输故障注入 | Err(TransportError) → 恢复，calls==2 |
| INT20 | 端到端三级度量证明 | BL/Kernel/Runtime measure → generate → trusted |
| INT21 | 端到端攻击：期望侧 Runtime 镜像被换 | untrusted + PcrMismatch（§4.4 拒绝信任） |
| PERF22 | quote 生成耗时打印/门禁 | release 打印；ENEROS_PERF_GATE=1 断言 < 100ms（D12） |
