# v0.111.0 模型 OTA 推送 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.111.0（P2-H 第 3 版，9 节齐全）。新建 crate `crates/agents/model-ota/`（eneros-model-ota，唯一依赖 eneros-crypto 复用国密）。蓝图/路线图检索确认无 v0.111.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

AI 模型需云端训练 → 签名 → 边缘热加载的远程迭代能力，免去现场维护（蓝图 §1）。无签名校验则模型可能被篡改（§2 阻塞项）。v0.110.0 已落地云边同步通道，v0.31.0/v0.32.0/v0.33.0 已落地国密 SM2/SM3 与 PKI 基座。本版实现 OTA 客户端（检查更新 + 断点续传下载）+ SM3 哈希/SM2 签名双重验证 + 白名单 + 原子热切换与回滚，打通「云端训练 → 签名 → 推送 → 验证 → 热加载」链路，为 v0.112.0 云端孪生与联邦 AI 持续演进提供模型更新通道。

## What Changes

- **新建** `crates/agents/model-ota/`（`eneros-model-ota`，no_std + alloc，零外部依赖，唯一 workspace 内 path 依赖 `eneros-crypto`，D7）：
  - `src/ota_client.rs`：`SigAlgorithm`（2 变体，RsaSha256 不可验证，D6）/ `ModelSignature` / `ModelInfo` / `OtaClient`（`check_update` + `download_model` 断点续传 + `verify_model` + `update_once`/`rollback_once` 编排）+ `OtaStats`（D12）+ manifest 二进制编解码（D5）
  - `src/signature.rs`：`verify_model_signature(data, info, pubkey)` 纯函数（SM3 哈希 → SM2 验签编排，D7）
  - `src/model_loader.rs`：`ModelInstance` / `HotLoader`（`load_new` 白名单 + `swap` 原子切换 + `rollback` 回滚 + `current`，&mut self 单线程惯例，D9/D10）
  - `src/lib.rs`：`OtaError`（11 变体，D12）/ `OtaTransport` trait + `MockOtaTransport`（D4）/ `OtaUpdateOutcome` + 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/model-ota.toml`：`[ota_client]` / `[hot_loader]` / `[security]` 三节 + 中文注释 ≥7 点
- **新增** `docs/agents/model-ota-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 26 个单元测试**（src 内嵌 `#[cfg(test)]`：OC×10 + SIG×3 + HL×8 + INT×4 + PERF×1）
- 根 `Cargo.toml`：members 追加 `"crates/agents/model-ota"`（cloud-sync 之后）+ version 0.110.0 → 0.111.0；`Makefile`（VERSION + L3 头部注释）/ `ci.yml` L3 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v11100-model-ota（新建）
- Affected code：`crates/agents/model-ota/`（新建）、`configs/`、`docs/agents/`、根 4 文件版本号
- 上游：v0.110.0 云边同步（OTA 通道基座）、v0.33.0 国密 eneros-crypto（sm3_hash/sm2_verify/Sm2Signature 复用，path 依赖）
- 下游：v0.112.0 云端数字孪生主节点（模型持续演进）；消费侧 v0.59.0 llm-engine / v0.61.0 model-deploy（热加载目标，本版不接线，集成层完成）

## ADDED Requirements

### Requirement: OTA 客户端与清单编解码（ota_client.rs）

The system SHALL provide `ModelInfo { model_id: String, version: String, hash: [u8; 32], size: u64, signature: ModelSignature, created_at: u64, capabilities: Vec<String> }`（derive Debug/Clone/PartialEq）；`ModelSignature { algorithm: SigAlgorithm, signature: Vec<u8>, timestamp: u64 }`（derive Debug/Clone/PartialEq；删除蓝图 `signer_cert` 字段——信任锚为构造注入的 `trusted_pubkey`，证书链验证归 v0.32.0 PKI 层，D11）；`SigAlgorithm { Sm2Sm3, RsaSha256 }`（derive Debug/Clone/Copy/PartialEq，RsaSha256 仅占位不可验证，D6）。manifest 二进制编解码（D5，零 serde 依赖）：`encode_manifest(&ModelInfo) -> Vec<u8>` / `decode_manifest(&[u8]) -> Result<ModelInfo, OtaError>`，帧布局全小端——`[magic u16 = 0x0A70][version u8 = 1][model_id_len u8 + model_id][version_len u8 + version][hash 32B][size u64][sig_algo u8（0=Sm2Sm3/1=RsaSha256）][sig_len u16 + signature][sig_timestamp u64][created_at u64][cap_count u8 + 每 cap（len u8 + bytes）]`；magic 错误/截断/字段越界 → `Err(InvalidManifest)`。

`OtaClient { current_model: ModelInfo, trusted_pubkey: Sm2PublicKey, max_retries: u32, stats: OtaStats }`（字段私有）：`new(current_model, trusted_pubkey, max_retries)`；`check_update<T: OtaTransport>(transport)` → `transport.fetch_latest(&self.current_model.version)`：None → `Ok(None)`；Some 且 version == current → `Ok(None)`（蓝图防御逻辑保留）；否则 `Ok(Some(info))`；`download_model<T>(transport, info)` 断点续传循环（§4.5）：info.size==0 → `Err(InvalidConfig)`；每次 `transport.download_range(&info.model_id, data.len() as u64, info.size - data.len() as u64)`——成功但 chunk 为空 → `Err(DownloadFailed)`（防蓝图死循环 bug）；成功追加，data.len() >= size 退出；失败 retries+1，retries > max_retries → `Err(DownloadFailed)`（同步 trait 下立即重试，退避由传输层自持，D4）；循环后 `data.len() as u64 != info.size` → `Err(SizeMismatch)`；`verify_model(data, info)` → 委托 `verify_model_signature`（由蓝图 `Result<bool>` 改为 `Result<(), OtaError>` 以区分 HashMismatch/SignatureInvalid 支撑 §4.4 安全告警，D12）。`OtaStats { total_updates, total_rejected, total_rollbacks, last_update_at }`（derive Debug/Clone/Copy/PartialEq，§9 可观测）+ `stats()` / `current_model()` 访问器。

#### Scenario: manifest 编解码往返（D5）
- **WHEN** 全字段 ModelInfo（含 2 个 capabilities）`encode_manifest` 后 `decode_manifest`
- **THEN** 解码结果与原值逐字段相等；magic 篡改或截断 → `Err(InvalidManifest)`

#### Scenario: 检查更新（蓝图 §4.5）
- **WHEN** transport 无更新 / 同版本 / 新版本三种情形
- **THEN** 分别返回 `Ok(None)` / `Ok(None)` / `Ok(Some(info))`

#### Scenario: 断点续传下载（蓝图 §4.4/§6.5）
- **WHEN** mock 模型 10 字节、chunk_size=4、第 2 次 download_range 注入 1 次 TransportError
- **THEN** 重试从 offset=4 继续（不重下前 4 字节），最终 data == 完整模型

#### Scenario: 下载失败（蓝图 §4.4）
- **WHEN** 连续失败 > max_retries，或 transport 返回空 chunk，或 info.size==0
- **THEN** 分别 `Err(DownloadFailed)` / `Err(DownloadFailed)` / `Err(InvalidConfig)`

### Requirement: 签名验证（signature.rs）

The system SHALL provide `verify_model_signature(data: &[u8], info: &ModelInfo, pubkey: &Sm2PublicKey) -> Result<(), OtaError>` 纯函数（D7 复用 eneros-crypto，禁止重复造轮子）：① `info.signature.algorithm != Sm2Sm3` → `Err(UnsupportedAlgorithm)`（D6）；② `sm3_hash(data) != info.hash` → `Err(HashMismatch)`；③ `info.signature.signature` 长度 != 64 → `Err(SignatureInvalid)`，否则 `Sm2Signature::from_bytes` 解码；④ `sm2_verify(&hash, &sig, pubkey)` → false ⇒ `Err(SignatureInvalid)`，true ⇒ `Ok(())`（签名消息为 SM3 哈希 32 字节，与蓝图 `sm2_verify(&signature, &hash, &ca_pubkey)` 语义一致）。

#### Scenario: 真实签名往返（蓝图 §6.1/§7.3）
- **WHEN** 测试用 eneros-crypto `Sm2KeyPair` + `sm2_sign` 对模型 SM3 哈希签名构造 ModelInfo
- **THEN** `verify_model_signature` 返回 `Ok(())`；篡改数据 1 字节 → `Err(HashMismatch)`；错误公钥 → `Err(SignatureInvalid)`

#### Scenario: 算法占位拒绝（D6）
- **WHEN** signature.algorithm = RsaSha256
- **THEN** 返回 `Err(UnsupportedAlgorithm)`

### Requirement: 热加载器（model_loader.rs）

The system SHALL provide `ModelInstance { info: ModelInfo, data: Vec<u8>, loaded_at: u64 }`（derive Debug/Clone/PartialEq；删除蓝图 `ref_count: AtomicU32`——`Arc` 强引用计数承载生命周期，D10）；`HotLoader { current: Arc<ModelInstance>, previous: Option<Arc<ModelInstance>>, loading: Option<ModelInstance>, white_list: Vec<[u8; 32]> }`（字段私有，`alloc::sync::Arc`，无锁 &mut self 单线程惯例，D9）：`new(current: ModelInstance, white_list)`（previous/loading = None）；`load_new(data, info, now)`：`sm3_hash(data)` 不在 white_list → `Err(NotInWhitelist)`（§4.3 拒绝 + 审计）；否则 `loading = Some(ModelInstance { info: info.clone(), data: data.to_vec(), loaded_at: now })`；`swap()`：loading 为 None → `Err(NothingToSwap)`；否则 `previous = Some(replace(current, Arc::new(loading.take())))`，返回新 current 克隆（删除蓝图 `swap_lock`/`AlreadySwapping`——&mut self 编译期排他，D9）；`rollback()`：previous 为 None → `Err(NoPreviousVersion)`；否则交换 current/previous（蓝图 `self.previous` 未声明字段 + `mem::replace(&mut *Arc)` 不可编译两处 bug 修复，D9）；`current()` 返回 `Arc<ModelInstance>` 克隆；`previous()` 返回 `Option<&Arc<ModelInstance>>`。

#### Scenario: 白名单拒绝（蓝图 §4.3/§7.3）
- **WHEN** 模型 SM3 哈希不在 white_list
- **THEN** `load_new` 返回 `Err(NotInWhitelist)`，loading 保持 None

#### Scenario: 原子切换与轮转（蓝图 §4.5）
- **WHEN** load_new(v2) → swap → load_new(v3) → swap
- **THEN** 第一次 swap 后 current==v2、previous==v1；第二次后 current==v3、previous==v2、loading==None

#### Scenario: 回滚（蓝图 §4.4/§6.4）
- **WHEN** 无 previous 调用 rollback → `Err(NoPreviousVersion)`；swap 后调用 rollback
- **THEN** current 恢复上一版本，previous 变为被替换下的版本

### Requirement: 传输抽象与端到端编排（lib.rs + ota_client.rs）

The system SHALL provide `OtaTransport` trait（sync，no_std 单线程惯例，不要求 Send+Sync，D4，v0.110.0 SyncTransport 先例）：`fetch_latest(&mut self, current_version: &str) -> Result<Option<ModelInfo>, OtaError>`（HTTP 204/JSON 语义由实现侧封装）；`download_range(&mut self, model_id: &str, offset: u64, len: u64) -> Result<Vec<u8>, OtaError>`；`MockOtaTransport { latest: Option<ModelInfo>, model_bytes: Vec<u8>, fail_remaining: u32, chunk_size: usize, pub download_calls: u32 }`（D4）：fetch_latest 返回 latest 克隆；download_range 中 fail_remaining>0 递减返回 `Err(TransportError)`，否则按 chunk_size 截断返回 `[offset, offset+len)` 区间字节克隆并 download_calls+1。

`OtaClient::update_once<T: OtaTransport>(&mut self, transport, loader, now) -> Result<OtaUpdateOutcome, OtaError>` 端到端编排（蓝图 §4.3 流程）：check_update → None ⇒ `Ok(NoUpdate)` → download_model → verify_model 失败 ⇒ `stats.total_rejected += 1` 原样返回 `Err`（§4.4 拒绝 + 安全告警）→ `loader.load_new(data, info, now)`（NotInWhitelist 同样 total_rejected+1）→ `loader.swap()` → `self.current_model = info`、`stats.total_updates += 1`、`stats.last_update_at = now` → `Ok(Updated)`；`rollback_once(loader, now)`：loader.rollback() 成功 → current_model 回滚为 previous info、total_rollbacks+1、last_update_at=now。`OtaUpdateOutcome { NoUpdate, Updated }`（derive Debug/Clone/Copy/PartialEq）。

#### Scenario: 端到端更新（蓝图 §6.2 集成测试）
- **WHEN** mock 持真实签名 v2 模型、白名单含其哈希：update_once
- **THEN** `Ok(Updated)`、loader.current().info.version=="2.0.0"、client.current_model 同步、total_updates==1、last_update_at==now

#### Scenario: 篡改拒绝（蓝图 §6.5 故障注入）
- **WHEN** mock 模型字节被篡改 1 字节（哈希不匹配）
- **THEN** update_once 返回 `Err(HashMismatch)`、total_rejected==1、loader.current 零变化

#### Scenario: 断点续传集成（蓝图 §6.5）
- **WHEN** 下载过程注入 2 次 TransportError（非连续）
- **THEN** update_once 仍 `Ok(Updated)`，download_calls ≥ 4，续传 offset 单调前进

#### Scenario: 切换后回滚（蓝图 §6.4 回归）
- **WHEN** update_once 成功后 rollback_once
- **THEN** loader.current 恢复 v1、client.current_model 回滚、total_rollbacks==1

#### Scenario: 下载性能（蓝图 §6.3/§7.2）
- **WHEN** 100MB 模拟模型下载（1MB/块）+ SM3 校验（cfg(test) Instant 口径，D12）
- **THEN** 总耗时 < 60s

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动；eneros-crypto 仅被 path 引用，零源码改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§5/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/model_ota/` → `crates/agents/model-ota/`（eneros-model-ota） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；OTA 为云边推送通道，与 v0.110.0 cloud-sync / v0.95.0 cloud-coordinator 同属 agents 子系统 |
| **D2** | 蓝图 `docs/phase2/model_ota.md` → `docs/agents/model-ota-design.md` | 记忆 §2.3.3 强制：文档按方向分类（cloud-sync-design.md 同目录先例） |
| **D3** | 蓝图 `tests/model_ota.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.110.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 `async check_update/download_model` + `HttpClient` + `server_url`/`download_dir` 字段 + `sleep().await` 退避 → `OtaTransport` sync trait（`fetch_latest(current_version)` + `download_range(model_id, offset, len)`）+ `MockOtaTransport`（fail_remaining 故障注入 + chunk_size 截断 + download_calls 计数，置于 lib.rs）；server_url/download_dir 移出 OtaClient；下载循环失败立即重试（退避由传输实现层自持） | no_std 无 async runtime/无 std::net/无 sleep（v0.110.0 D4 SyncTransport 同先例）；主机可测；真实 HTTP/gRPC 适配器在集成层注入；断点续传语义（offset 从已下载长度继续）保留 |
| **D5** | 蓝图 `serde_json::from_slice(&resp.body)` 解析 ModelInfo → 自定义二进制 manifest 编解码（`encode_manifest`/`decode_manifest`，magic 0x0A70 + version 1，全小端 TLV） | 零外部依赖（serde/serde_json 不入仓，v0.110.0 D11 同先例）；magic+version 支撑云端 API 版本演进 |
| **D6** | `SigAlgorithm { Sm2Sm3, RsaSha256 }` 保留 2 变体（对齐蓝图数据结构），但 RsaSha256 不可验证——`verify_model_signature` 遇之返回 `UnsupportedAlgorithm` | eneros-crypto 纯国密无 RSA（信创 §5.6 全程国密要求）；v0.110.0 D5 CompressionType 占位同先例 |
| **D7** | 蓝图 `sm3_hash`/`sm2_verify` 未指明实现 → 复用 eneros-crypto（path 依赖 `../../security/crypto`）：`sm3_hash(data) -> [u8;32]`、`sm2_verify(&hash, &Sm2Signature, &Sm2PublicKey)`、`Sm2Signature::from_bytes`（64B r‖s） | 记忆 §5.5/禁忌 14 禁止重复造轮子；国密实现已经安全评审（常量时间/零化/Drop），自研重引入风险 |
| **D8** | 蓝图 `current_time_ms()` 全局时间函数 → `now: u64` 参数注入（load_new/update_once/rollback_once） | no_std 无系统时间（v0.110.0 D7 / v0.108.0 D9 同先例）；集成层由 v0.12.0 RTC 供给 |
| **D9** | 蓝图 HotLoader 用 std `Arc/Mutex/AtomicU32` + `swap_lock` + `mem::replace(&mut *self.current.as_ref())`（不可编译：Arc 不可变借用）+ rollback 引用未声明的 `self.previous` 字段 → `alloc::sync::Arc` + 无锁 &mut self 单线程惯例：`current: Arc<ModelInstance>` / `previous: Option<Arc<ModelInstance>>` / `loading: Option<ModelInstance>`；swap 持 loading.take() + mem::replace 原子轮换并留存 previous；删除 swap_lock 与 AlreadySwapping 变体（&mut self 编译期排他） | 蓝图代码两处编译错误必须修复；v0.110.0 D4 单线程无 Send+Sync 惯例；Arc 仅在 current() 读侧克隆，写侧 &mut self 排他 |
| **D10** | 删除蓝图 `ModelInstance.ref_count: AtomicU32` | `alloc::sync::Arc` 强引用计数即生命周期管理，手写计数属重复造轮子（禁忌 14）；「旧模型引用归零自动释放」语义由 Arc drop 承载 |
| **D11** | ① 蓝图 `trusted_ca_pubkey()` 从本地存储 `load_ca_pubkey().unwrap_or_default()` → 构造注入 `trusted_pubkey: Sm2PublicKey`；② 删除蓝图 `ModelSignature.signer_cert: Sm2Cert` 字段；③ 白名单 white_list 构造注入 | ① 安全关键件禁止静默默认空值（空公钥语义不明，no_std 无本地安全存储抽象）；② 信任锚为注入公钥，证书链验证归 v0.32.0 PKI 层职责，本版不做链式验证（Karpathy 最小实现）；③ 白名单运维下发属集成层 |
| **D12** | 错误模型 `OtaError` = TransportError / DownloadFailed / InvalidManifest / HashMismatch / SignatureInvalid / NotInWhitelist / NothingToSwap / NoPreviousVersion / UnsupportedAlgorithm / InvalidConfig / SizeMismatch（11 变体，Debug/Clone/Copy/PartialEq，Copy 对齐 v0.95.0 CloudError 惯例；DownloadFailed 删除蓝图 String payload）；`verify_model` 由蓝图 `Result<bool>` 改 `Result<(), OtaError>` 区分哈希/签名失败支撑 §4.4 安全告警；新增 `OtaStats { total_updates, total_rejected, total_rollbacks, last_update_at }` 落地 §9 更新状态 metric；性能「100MB < 60s」落地为 cfg(test) Instant 主机断言 | 蓝图引用 OtaError 未定义；变体覆盖 §4.4 各失败面；bool 无法区分拒绝原因不利审计；性能口径与 v0.109.0/v0.110.0 D12 一致（真实网络时延为实验室项） |

## 测试规划（26 个）

| 编号 | 名称 | 断言要点 |
|------|------|---------|
| OC1 | manifest 编解码往返 | 全字段（含 2 capabilities）encode→decode 逐字段相等 |
| OC2 | manifest 坏 magic / 截断 | Err(InvalidManifest) |
| OC3 | check_update 无更新 | Ok(None) |
| OC4 | check_update 同版本/新版本 | None / Some(info) |
| OC5 | download_model 单 chunk 成功 | 字节完整一致 + download_calls==1 |
| OC6 | download_model 多 chunk + 1 次失败续传 | 重试 offset==已下载长度，最终字节一致 |
| OC7 | download_model 连续失败超限 | Err(DownloadFailed)，retries==max_retries+1 |
| OC8 | download_model 空 chunk / size==0 | Err(DownloadFailed) / Err(InvalidConfig)（防死循环） |
| OC9 | verify_model 哈希不匹配 | Err(HashMismatch) |
| OC10 | verify_model 签名无效（错公钥/坏签名） | Err(SignatureInvalid) |
| SIG11 | Sm2 真实签名往返 | eneros-crypto 签名 → verify_model_signature Ok |
| SIG12 | 篡改数据 1 字节 | Err(HashMismatch) |
| SIG13 | RsaSha256 占位 | Err(UnsupportedAlgorithm) |
| HL14 | load_new 不在白名单 | Err(NotInWhitelist)，loading 保持 None |
| HL15 | load_new 成功 | loading 就绪（swap 可行前置） |
| HL16 | swap 无 loading | Err(NothingToSwap) |
| HL17 | swap 成功 | current 更新 + previous 留存 + loading 清空 |
| HL18 | 连续两次 swap | previous 轮转正确（v1→v2→v3） |
| HL19 | rollback 无 previous | Err(NoPreviousVersion) |
| HL20 | rollback 成功 | current 恢复上一版 |
| HL21 | current() Arc 克隆 | 数据一致 + loaded_at 保留 |
| INT22 | 端到端更新全流程 | Updated + current_model 同步 + total_updates==1 |
| INT23 | 篡改模型拒绝 | Err(HashMismatch) + total_rejected==1 + loader 零变化 |
| INT24 | 断点续传集成（2 次非连续失败） | Updated + download_calls ≥ 4 |
| INT25 | 切换后回滚 | current 恢复 v1 + total_rollbacks==1 |
| PERF26 | 100MB 下载 + SM3 校验 | < 60s（cfg(test) Instant，D12） |
