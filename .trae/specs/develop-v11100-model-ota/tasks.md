# Tasks — v0.111.0 模型 OTA 推送

> Spec：`spec.md`（develop-v11100-model-ota）。T1→T2 顺序（T2 消费 T1 的 ModelInfo）；T3 依赖 T1+T2（编排方法消费客户端与加载器）；T4/T5 顺序收尾。

- [x] **T1：新建 model-ota crate 骨架 + lib.rs 基座 + signature.rs + ota_client.rs — 客户端/清单/验签**
  - [x] 1.1 `crates/agents/model-ota/Cargo.toml`：`eneros-model-ota`，workspace 继承，唯一依赖 `eneros-crypto = { path = "../../security/crypto" }`（D7）
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（ota_client/signature/model_loader）+ 重导出 + `OtaError`（11 变体：TransportError/DownloadFailed/InvalidManifest/HashMismatch/SignatureInvalid/NotInWhitelist/NothingToSwap/NoPreviousVersion/UnsupportedAlgorithm/InvalidConfig/SizeMismatch，derive Debug/Clone/Copy/PartialEq，D12）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 cloud-sync）
  - [x] 1.3 `src/signature.rs`：`verify_model_signature(data, info, pubkey) -> Result<(), OtaError>`（D7：算法门 → sm3_hash 比对 → Sm2Signature::from_bytes 64B → sm2_verify）
  - [x] 1.4 `src/ota_client.rs`：`SigAlgorithm`（2 变体，D6）+ `ModelSignature` + `ModelInfo` + `encode_manifest`/`decode_manifest`（magic 0x0A70，D5）+ `OtaClient`（new/check_update/download_model 断点续传/verify_model/current_model/stats，字段私有，D4/D11）+ `OtaStats`（4 字段，D12）
  - [x] 1.5 测试 OC1~OC10（10 个）+ SIG11~SIG13（3 个，真实 Sm2 签名往返用 eneros-crypto Sm2KeyPair/sm2_sign/CsRng，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-model-ota` 13/13 全过 ✅

- [x] **T2：model_loader.rs — 白名单 + 原子热切换 + 回滚**
  - [x] 2.1 `src/model_loader.rs`：`ModelInstance`（3 pub 字段，删 ref_count，D10）+ `HotLoader`（current: Arc<ModelInstance>/previous: Option<Arc<ModelInstance>>/loading: Option<ModelInstance>/white_list，字段私有，无锁 &mut self，D9）：`new(current, white_list)` / `load_new(data, info, now)`（sm3_hash ∉ white_list → NotInWhitelist）/ `swap()`（None → NothingToSwap；否则 previous=replace(current, loading.take()) 返回新 Arc）/ `rollback()`（None → NoPreviousVersion；否则 current/previous 交换）/ `current()` / `previous()`
  - [x] 2.2 测试 HL14~HL21（8 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-model-ota model_loader::` 8/8 全过 ✅

- [x] **T3：lib.rs OtaTransport/Mock + update_once/rollback_once 编排 + 集成测试 — 端到端 OTA 闭环**
  - [x] 3.1 lib.rs 追加 `OtaTransport` trait（`fetch_latest(current_version) -> Result<Option<ModelInfo>, OtaError>` + `download_range(model_id, offset, len) -> Result<Vec<u8>, OtaError>`，sync 无 Send+Sync bound，D4）+ `MockOtaTransport { latest, model_bytes, fail_remaining, chunk_size, pub download_calls }`（new/with_latest；fail_remaining>0 递减返回 TransportError，否则按 chunk_size 截断返回区间克隆 + download_calls+1，D4）
  - [x] 3.2 `ota_client.rs` 追加编排方法：`update_once(transport, loader, now)`（check → None → Ok(NoUpdate) → download → verify 失败 total_rejected+1 原样 Err → load_new（NotInWhitelist 同 total_rejected+1）→ swap → current_model=info + total_updates+1 + last_update_at=now → Ok(Updated)）+ `rollback_once(loader, now)`（rollback 成功 → current_model 回滚 + total_rollbacks+1）+ `OtaUpdateOutcome`（2 变体）
  - [x] 3.3 测试 INT22~INT25（端到端更新 / 篡改拒绝 loader 零变化 / 2 次非连续失败断点续传 / 切换后回滚）+ PERF26（100MB 下载 + SM3 校验 < 60s，`std::time::Instant` 仅 cfg(test)，D12）
  - 验证：`cargo test -p eneros-model-ota` 26/26 全过 ✅

- [x] **T4：workspace 接线 + 配置 + 设计文档**
  - [x] 4.1 根 `Cargo.toml` members 追加 `"crates/agents/model-ota"`（agents 段 cloud-sync 之后）
  - [x] 4.2 `configs/model-ota.toml`：`[ota_client]` max_retries=3 + `[hot_loader]` white_list + `[security]` trusted_pubkey 注入说明 + 中文注释 ≥7 点（签名+白名单双重验证 §7.3 / OtaTransport 抽象 D4 / 断点续传 §4.4 / 国密复用 D7 / 内存预算 2× 模型峰值 记忆 §5.6 / 性能口径 100MB<60s D12 / GPU 不适用 §6.6 / 下游 v0.112.0 云端孪生）
  - [x] 4.3 `docs/agents/model-ota-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 OTA 流程图重绘 + HotLoader 状态迁移图 current/loading/previous）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D12）+ 内存预算声明
  - 验证：`cargo metadata` 解析成功；crate 测试全过 ✅

- [x] **T5：版本同步 0.111.0 + 全量构建验证 + checklist 核验收工**
  - [x] 5.1 根 `Cargo.toml` version = "0.111.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` L3 注释；`gate.rs` 注释串尾 2 处追加 v0.111.0 类型清单（11 类型：OtaClient/ModelInfo/ModelSignature/SigAlgorithm/HotLoader/ModelInstance/OtaTransport/MockOtaTransport/OtaError/OtaStats/OtaUpdateOutcome）
  - [x] 5.2 §2.4.2 构建校验：C6 metadata / C7 本 crate 26 测试 + 全 workspace 回归（含 eneros-cloud-sync/eneros-crypto 零改动回归）/ C8 aarch64 交叉编译 / C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - [x] 5.3 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：C6~C11 全绿，checklist 全勾 + 验收记录已填，收工

- [x] **T6：修复 tasks.md 内容丢失（checklist C74 核验未通过）**
  - [x] 6.1 依据 spec.md 交付物与 checklist.md 分组重建 T1~T5 任务列表（任务标题复选框 + 子项复选框 + 验证行 + Task Dependencies 节）
  - [x] 6.2 按实际完成状态勾选全部子项并补验证行（cargo test 26/26、aarch64 交叉编译、fmt/clippy/deny 等 exit=0）
  - [x] 6.3 重核 checklist.md C74 并勾选，验收记录通过项数更新为 78/78

# Task Dependencies

- T1 先行（T2 依赖 ota_client.rs 的 ModelInfo/OtaError 基型）
- T2 depends on T1
- T3 depends on T1 + T2（update_once 编排消费客户端与 HotLoader）
- T4 depends on T3（文档需最终代码签名）
- T5 depends on T4
- T6 depends on T5（checklist C74 核验未通过的修复任务）
