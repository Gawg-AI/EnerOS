# Checklist — v0.111.0 模型 OTA 推送

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C crate 骨架 no_std / D ota_client.rs / E signature.rs / F model_loader.rs / G 传输抽象与集成 / H 配置与文档 / I 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：ota_client.rs / signature.rs / model_loader.rs 三模块 + OtaClient/ModelSignature/HotLoader 接口齐全
- [x] C2: 接口对齐 spec 接口契约：`OtaClient` 含 new/check_update/download_model/verify_model/update_once/rollback_once/current_model/stats；`HotLoader` 含 new/load_new/swap/rollback/current/previous；`verify_model_signature` 纯函数
- [x] C3: 数据结构对齐 spec：ModelInfo（7 字段）/ModelSignature（3 字段，无 signer_cert，D11）/ModelInstance（3 字段，无 ref_count，D10）/OtaStats（4 字段）
- [x] C4: `OtaError` 11 变体齐全（TransportError/DownloadFailed/InvalidManifest/HashMismatch/SignatureInvalid/NotInWhitelist/NothingToSwap/NoPreviousVersion/UnsupportedAlgorithm/InvalidConfig/SizeMismatch，D12），derive Debug/Clone/Copy/PartialEq
- [x] C5: `SigAlgorithm` 2 变体齐全（Sm2Sm3/RsaSha256），RsaSha256 验证 → UnsupportedAlgorithm（D6）
- [x] C6: 蓝图 §4.3 流程对齐：下载 → SM3 哈希校验 → SM2 签名验证 → 白名单检查 → 热加载 → 原子切换
- [x] C7: 断点续传对齐蓝图 §4.5：失败重试 offset 从已下载长度继续，超 max_retries → DownloadFailed
- [x] C8: SM3 + SM2 双重验证复用 eneros-crypto（sm3_hash/sm2_verify/Sm2Signature::from_bytes，D7），零自研密码学
- [x] C9: `OtaTransport` trait + `MockOtaTransport` 存在（D4），零 `std::net`、零 async、零 HttpClient、零 serde_json
- [x] C10: spec.md D1~D12 偏差表与 lib.rs crate 文档偏差表、设计文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: crate 位于 `crates/agents/model-ota/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/agents/model-ota"`（cloud-sync 之后）
- [x] C13: `Cargo.toml` 零外部依赖，唯一 path 依赖 `eneros-crypto = { path = "../../security/crypto" }`（跨子系统相对路径，D7）
- [x] C14: 文档位于 `docs/agents/model-ota-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C20: 零 unsafe；零 extern "C"；锁自由（无 std::sync::Mutex，&mut self 单线程惯例，D9）
- [x] C21: `cargo build -p eneros-model-ota --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 cloud-sync）

## D. ota_client.rs（C23~C33）

- [x] C23: `ModelInfo` 7 个 pub 字段（model_id/version/hash/size/signature/created_at/capabilities），derive Debug/Clone/PartialEq；`ModelSignature` 3 字段（algorithm/signature/timestamp）
- [x] C24: manifest 帧布局（D5）：magic u16 LE 0x0A70 + version u8 1 + model_id_len u8 + model_id + version_len u8 + version + hash 32B + size u64 + sig_algo u8 + sig_len u16 + signature + sig_timestamp u64 + created_at u64 + cap_count u8 + caps，全 LE
- [x] C25: encode→decode 往返逐字段相等（含 capabilities）；坏 magic/截断/字段越界 → InvalidManifest
- [x] C26: `check_update`：transport None → Ok(None)；同版本 → Ok(None)（蓝图防御保留）；新版本 → Ok(Some(info))
- [x] C27: `download_model`：info.size==0 → InvalidConfig；chunk 为空 → DownloadFailed（防蓝图死循环 bug）
- [x] C28: `download_model` 断点续传：第 n 次失败后重试 offset == 已下载字节数（不重下前缀）；完成后 data.len() == info.size
- [x] C29: `download_model` 连续失败 > max_retries → DownloadFailed；循环后长度不符 → SizeMismatch
- [x] C30: `verify_model` 委托 `verify_model_signature` 返回 `Result<(), OtaError>`（D12 非 bool）
- [x] C31: `OtaClient` 字段私有（current_model/trusted_pubkey/max_retries/stats）；`stats()`/`current_model()` 访问器存在
- [x] C32: `OtaStats` 4 字段（total_updates/total_rejected/total_rollbacks/last_update_at），derive Debug/Clone/Copy/PartialEq
- [x] C33: 测试 OC1~OC10 共 10 个全部通过

## E. signature.rs（C34~C38）

- [x] C34: `verify_model_signature(data, info, pubkey)` 纯函数存在，不持有状态
- [x] C35: 算法门：algorithm != Sm2Sm3 → UnsupportedAlgorithm（先于哈希/验签，D6）
- [x] C36: SM3 门：`sm3_hash(data) != info.hash` → HashMismatch
- [x] C37: SM2 门：signature 长度 != 64 → SignatureInvalid；`sm2_verify(&hash, &sig, pubkey)` false → SignatureInvalid；true → Ok(())
- [x] C38: 测试 SIG11~SIG13 共 3 个全部通过（真实 Sm2KeyPair/sm2_sign 往返 / 篡改 1 字节 / RsaSha256 占位）

## F. model_loader.rs（C39~C47）

- [x] C39: `ModelInstance` 3 个 pub 字段（info/data/loaded_at），无 ref_count（D10），derive Debug/Clone/PartialEq
- [x] C40: `HotLoader` 字段私有：current: Arc<ModelInstance> / previous: Option<Arc<ModelInstance>> / loading: Option<ModelInstance> / white_list: Vec<[u8; 32]>；无 Mutex/AtomicU32/swap_lock（D9/D10）
- [x] C41: `load_new`：sm3_hash(data) ∉ white_list → NotInWhitelist 且 loading 不变；∈ → loading=Some(ModelInstance{info 克隆, data 克隆, loaded_at=now})
- [x] C42: `swap`：loading None → NothingToSwap；否则 previous=旧 current、current=新 Arc、loading=None，返回新 current 克隆
- [x] C43: 连续两次 swap：previous 轮转正确（v1→v2→v3）
- [x] C44: `rollback`：previous None → NoPreviousVersion；否则 current/previous 交换（蓝图未声明字段 bug 修复，D9）
- [x] C45: `current()` 返回 Arc 克隆（数据一致 + loaded_at 保留）；`previous()` 返回 Option<&Arc<ModelInstance>>
- [x] C46: 全模块无锁（&mut self 编译期排他），AlreadySwapping 变体不存在（D9）
- [x] C47: 测试 HL14~HL21 共 8 个全部通过

## G. 传输抽象与集成（C48~C55）

- [x] C48: `OtaTransport` 为 sync trait，无 Send+Sync bound（D4，v0.110.0 SyncTransport 惯例）
- [x] C49: `MockOtaTransport`：fail_remaining>0 递减返回 TransportError；成功时按 chunk_size 截断返回 [offset, offset+len) 克隆 + download_calls+1
- [x] C50: `update_once`：无更新 → Ok(NoUpdate)，loader/stats 零变化
- [x] C51: `update_once` 成功：Ok(Updated) + current_model 同步新 info + total_updates+1 + last_update_at==now
- [x] C52: `update_once` 验证失败（HashMismatch/SignatureInvalid/NotInWhitelist）：total_rejected+1 + Err 原样返回 + loader.current 零变化（蓝图 §4.4 拒绝+告警）
- [x] C53: INT24 断点续传集成：2 次非连续失败后 Ok(Updated)，download_calls ≥ 4，续传 offset 单调
- [x] C54: `rollback_once` 成功：loader.current 恢复旧版 + client.current_model 回滚 + total_rollbacks+1（蓝图 §6.4 回归）
- [x] C55: PERF26 100MB 下载 + SM3 校验 < 60s（cfg(test) Instant 断言，D12）

## H. 配置与文档（C56~C61）

- [x] C56: `configs/model-ota.toml` 存在，`[ota_client]` + `[hot_loader]` + `[security]` 节齐全 + 中文注释 ≥7 点
- [x] C57: 配置中文注释覆盖：签名+白名单双重验证 §7.3 / OtaTransport 抽象 D4 / 断点续传 §4.4 / 国密复用 D7 / 内存预算 / 性能口径 D12 / GPU 不适用 / 下游 v0.112.0
- [x] C58: `docs/agents/model-ota-design.md` 存在，12 章节齐全
- [x] C59: 文档含 ≥2 个 Mermaid 图：OTA 流程图（蓝图 §4.3 重绘）+ HotLoader 状态迁移图（current/loading/previous）
- [x] C60: 文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C61: 文档含性能口径声明（D12）+ 内存预算声明（加载峰值 2× 模型大小，LLM ≤4GB 分区，记忆 §5.6）

## I. 版本同步与构建验证（C62~C78）

- [x] C62: 根 `Cargo.toml` version == "0.111.0"
- [x] C63: `Makefile` VERSION == 0.111.0 且 L3 头部注释同步
- [x] C64: `ci.yml` L3 版本注释 == v0.111.0
- [x] C65: `gate.rs` 注释串尾 2 处追加 v0.111.0 类型清单（11 类型）
- [x] C66: `cargo test -p eneros-model-ota` 26/26 通过
- [x] C67: eneros-cloud-sync / eneros-crypto 回归通过（零改动验证）
- [x] C68: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C69: `cargo build -p eneros-model-ota --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C70: `cargo fmt --all -- --check` 通过
- [x] C71: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C72: `cargo deny check advisories licenses bans sources` 通过（零新增外部依赖）
- [x] C73: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C74: spec.md / tasks.md / checklist.md 三件齐全且内容一致；tasks.md 全部复选框已勾选；无超范围交付（Karpathy Simplicity First）
- [x] C75: 内存预算声明已落地文档（峰值 2× 模型大小，LLM 7B INT4 ≤4GB 分区，蓝图 §43.6）
- [x] C76: eneros-crypto 零源码改动（仅 path 引用，git diff 为空）
- [x] C77: 蓝图 9 节模板核验：spec.md 已覆盖版本目标/前置依赖/交付物/详细设计/技术交底（选型表融入 D 偏差与文档）/测试计划/验收标准/风险/多角度要求
- [x] C78: Sm2PrivateKey 不出现在本 crate 任何存储/日志路径（仅验签用公钥，记忆安全约束）

## 验收记录

- **核验日期**：2026-07-20
- **核验人**：Trae Agent
- **通过项数**：78/78
- **核验方式**：
  - C1~C5/C23~C28/C34~C37/C39~C46/C48~C52：源码审阅（lib.rs / ota_client.rs / signature.rs / model_loader.rs 结构与接口签名逐项比对；蓝图 phase2.md §v0.111.0 §3 交付物/§4.3 流程/§4.5 断点续传关键代码对齐确认，含蓝图两处编译错误 bug 的 D9 修复核对）
  - C33/C38/C47/C53~C55/C66：`cargo test -p eneros-model-ota` 26/26 通过（OC1~OC10 ×10 + SIG11~SIG13 ×3 + HL14~HL21 ×8 + INT22~INT25 ×4 + PERF26 ×1，本轮重跑确认 1.12s exit=0）
  - C10/C60：spec.md §偏差声明 / lib.rs crate 文档 / model-ota-design.md §11 三处 D1~D12 偏差表逐字一致（PowerShell 提取 12 行 × 3 处 -cne 三向比对零差异）
  - C11~C15/C17~C20/C73/C76/C78：目录结构 + no_std 合规审阅（crate 位于 crates/agents/model-ota/ 且无 tests/ 目录；零 std:: 生产引用——std::time::Instant 仅 PERF26 cfg(test) 内；零 panic!/todo!/unimplemented!；零 unsafe；零 extern "C"；生产路径零 unwrap——unwrap 命中均在 #[cfg(test)] 模块；git status --porcelain 过滤 target/elf/bin/dtb/.idea/.vscode 零命中；git diff --stat -- crates/security/crypto 为空；Sm2PrivateKey 零生产存储/日志引用，仅测试模拟云端签名）
  - C16：`cargo metadata --format-version 1` exit=0（本轮重跑确认）
  - C21/C69：`cargo build -p eneros-model-ota --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` exit=0（前轮已验证，本轮采信）
  - C22/C56~C59/C61/C75：lib.rs crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明）、configs/model-ota.toml（[ota_client]/[hot_loader]/[security] 3 节 + 10 点中文注释覆盖 §7.3 双重验证/D4 传输抽象/§4.4 断点续传/D7 国密复用/内存预算/D12 性能口径/GPU 不适用/下游 v0.112.0）、docs/agents/model-ota-design.md（12 章节 + 2 Mermaid：§4.2 OTA 时序图 + §4.5 HotLoader 状态迁移图；§9 性能口径 + 内存预算声明：峰值 2× 模型大小、LLM 7B INT4 ≤4GB 分区）审阅
  - C62~C65：根 Cargo.toml version=0.111.0 / Makefile VERSION=0.111.0 + L3 注释 / ci.yml L3 注释 / gate.rs L144+L233 注释串尾 v0.111.0 类型清单（11 类型：OtaClient/ModelInfo/ModelSignature/SigAlgorithm/HotLoader/ModelInstance/OtaTransport/MockOtaTransport/OtaError/OtaStats/OtaUpdateOutcome）审阅
  - C67/C68：eneros-cloud-sync / eneros-crypto 回归 + 全 workspace 回归（--exclude eneros-kernel --exclude eneros-hello）exit=0 零回归（前轮已验证，本轮采信）
  - C70：`cargo fmt --all -- --check` exit=0（本轮重跑确认）
  - C71：`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` exit=0（0 warning，本轮重跑确认）
  - C72：`cargo deny check advisories licenses bans sources` exit=0（前轮已验证，零新增外部依赖）
  - C74：**通过（重核）**——首次核验未通过（tasks.md 因编辑事故仅余 20 字节依赖 fragment），已执行 T6 修复：依据 spec.md 交付物与 checklist.md 重建 T1~T6 完整任务列表（任务标题复选框 + 子项复选框 + 验证行 + Task Dependencies 节），全部复选框按实际完成状态勾选；spec.md / tasks.md / checklist.md 三件齐全一致，交付物未超 spec 范围
  - C77：spec.md（Why/What Changes/Impact/Requirements/偏差声明/测试规划）+ 设计文档 §1~§9 共同覆盖蓝图 9 节模板全部要素（版本目标/前置依赖/交付物/详细设计/技术交底/测试计划/验收标准/风险/多角度要求）
