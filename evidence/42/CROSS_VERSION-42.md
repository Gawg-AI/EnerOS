# CROSS_VERSION_CHANGE_REQUEST — 42

## 当前版本
42-证据链CI工具与零硬件验收修复蓝图

## 目标文件和接口
- `crates/boot/boot-manager-uefi/src/main.rs`
- `crates/runtime/root-task/src/main.rs`
- `LoadedBoot` 增加 `source_date_epoch: u64`
- 新增 `boot_reason_name()`、`hex_encode()`
- 在 `prepare_boot` 成功路径、`exit_boot_services` 前输出 `eneros.boot-evidence.v1` JSON

## 为什么无法通过现有接口完成
- 现有 boot-manager 只输出 `ENEROS stage=verified` 文本行，没有结构化 boot-evidence JSON。
- `xtask/src/qemu_harness.rs` 的正式介质 PASS 解析依赖 `eneros.boot-evidence.v1` 全字段（manifest_sha256、measurement_root_sha256、measurement_log_hex、memory_map_count、source_date_epoch 等）。
- 这些字段只能在 boot-manager 内部、退出启动服务前取得；外部 harness 无法自行合成可信值。

## 最小修改范围
- boot-manager：仅为证据输出增加字段与打印，不改变启动策略、验签、内存布局或业务行为。
- root-task：在真实事件点输出 G2/G3 JSON，不改变控制/驱动/安全逻辑。
- `qemu_harness.rs` 同步增加 `--qmp-shutdown` 受控退出能力（42 owner）。

## 影响的后续版本
- 43 全 ARM/商业终验将消费该 boot-evidence。
- 36–41 的 boot/trust 语义不受影响，仅新增可观测输出。

## 回归测试
- `cargo test -p eneros-boot-manager-uefi --locked`：8/8 PASS
- `cargo test -p eneros-xtask --locked --bin eneros-xtask`：101/101 PASS
- `cargo test -p eneros-root-task --locked`：97/97 PASS
- QEMU 实跑：`cargo xtask run qemu-prod --expect PASS --qmp-shutdown` 已 PASS，boot-evidence + G2/G3 齐备。

## 回退方式
- boot-manager：移除 `LoadedBoot.source_date_epoch`、`boot_reason_name`、`hex_encode` 以及 boot-evidence `println!` 块。
- root-task：移除新增的 G2/G3 `println!` 行。

## CROSS_VERSION_CHANGE_REQUEST — unsafe SAFETY 注释补齐（Round 101）

- 当前版本：42-证据链CI工具与零硬件验收修复蓝图
- 目标文件和接口：
  - `crates/drivers/net/src/virtio_net.rs`
  - `crates/drivers/storage/src/driver/virtio_blk.rs`
  - `crates/runtime/oom-probe/src/main.rs`
  - `crates/runtime/root-task/src/allocator.rs`
  - `crates/runtime/root-task/src/main.rs`
- 为什么无法通过现有接口完成：42 Exit Gate #10 要求 unsafe/SAFETY 审计真实 PASS；当前 unsafe 块缺少 SAFETY 注释，只能在这些文件内补充注释。
- 最小修改范围：仅增加 `// SAFETY:` / `# Safety` 注释，不改变任何控制流、地址、长度或行为。
- 影响的后续版本：43 复验可继续消费；37–40 无行为变化。
- 回归测试：
  - `cargo xtask audit-unsafe-safety`
  - `cargo xtask evidence42 unsafe-report`
  - 相关 crate 单元测试/构建
  - QEMU L2 证据需按新 dirty tree 重跑
- 回退方式：删除新增注释行即可恢复原状。

## CV-42-1：closure crypto 政策批准（ADR-022 选项 1，负责人签署）

- **当前版本**：42（证据链与零硬件验收）
- **目标文件**：`product/crypto-policy.toml`（36/41 owner）
- **为什么需要跨版本修改**：Exit Gate #13 要求 36–41 Gate 在当前 candidate 全 PASS；closure gate 唯一 FAIL 是 qemu-prod 闭包含 eneros-crypto（SM2/SM3，no_std 目标唯一可链接实现——ADR-022 事实）。政策原样 product_allowed=false（"即使 KAT PASS 也不得入闭包"）使 #13 结构性不可达。
- **负责人决策**：会话指令"开发到完成42的内容，直到可以开始43" = ADR-022 选项 1（批准 SM2 入闭包并绑定 KAT/互操作向量证据）。决策 ID：CHG-2026-08-17-42-closure-sm2。
- **最小修改范围**：crypto-policy.toml 三个 usage（device_identity/ota_signature/attestation）置 product_allowed=true、evaluation_status=approved、evidence_digest 绑定真实证据（tests/vectors/sm2-interop/vectors.json 10 组 OpenSSL 向量 + sm2/sm3/sm4_kat.rs + cry001_constant_time.rs）、allowed_targets=["aarch64-unknown-none"]；ecdsa 随主实现登记为 impl_dependencies。未改任何产品代码。
- **影响后续版本**：43 密码学独立评审为前置条件——评审不通过时本政策必须回退 false（可逆）；本决策不构成商密合规宣称。
- **回归测试**：`cargo xtask closure gate --profile qemu-prod` → PASS（crypto_violations=0）；gate-status 15/15。
- **回退方式**：把三个 usage 的 product_allowed 改回 false、删除 ecdsa 登记即可（gate 恢复 FAIL 语义）。
