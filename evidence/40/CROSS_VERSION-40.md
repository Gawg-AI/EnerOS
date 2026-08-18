# CROSS_VERSION-40 — 跨版本改动登记（蓝图 40）

> 按 35-00 §9 纪律：必须修改非 40-owned 文件时停止并登记。以下均为 40 达成
> Exit Gate 所必需的最小改动；36/37/38/39 各域改动已在 40 全仓回归
> （7308/0）+ QEMU 装配层（PRODUCT-READY: true）验证，无行为回退。

| ID | 目标文件 | 接口/改动 | 为何无法经 40 owned 接口完成 | 最小范围 | 影响后续版本 | 回归 | 回退 |
|---|---|---|---|---|---|---|---|
| CVR-40-01 | `crates/runtime/root-task/src/allocator.rs` | 默认堆 3 MiB→4 MiB | 25 服务 bundle 解析期 OOM（used=3130304）——root 堆容量不在 40 owned 路径 | 常量默认值 | 41+ 服务增长预算 | 全仓 7308/0 + QEMU 4 轮 | 恢复 3 MiB（25 服务会 OOM） |
| CVR-40-02 | `crates/runtime/root-task/src/main.rs`、`spawn.rs` | per-service 栈机制（`CONTROL_SERVICE_STACK_PAGES` + `plan_spawn_dyn_aware_with_stack`），当前表置空 | 控制服务大栈需按名覆盖（全局放大超 ROOT_CNODE 4096 槽） | 新增 wrapper + 空表（默认路径不变） | 42 profile stack_pages 接线可复用 | QEMU PRODUCT-READY true | 表置空即回退 |
| CVR-40-03 | `crates/ai/solver-core`、`crates/ai/llm-engine` | release 无后端 compile_error 移除（mock release 熔断保留）；solver-core `default=[]`（highs-ffi 显式启用） | target 闭包按 WP-40.4 "profile 不声明"构建——无后端是合法状态；host 消费者已显式启用 feature | 门控放宽 + default 变更 + runner/energy-market-agent 等显式 highs | 41/42 构建 | 全仓 7308/0（runner/energy-agent 显式 highs 后全绿） | 恢复门控（target 服务无法构建） |
| CVR-40-04 | `crates/drivers/drv-svc/src/allocator.rs` | `heap-large` feature（384 KiB） | svc-device-agent 的 Box 化 EnvelopeIngress 需 ~276 KiB 堆分配；默认 64 KiB 保持小服务紧凑 | 新增 feature + 默认不变 | 41+ | QEMU svc-device-agent READY | 去掉 feature |
| CVR-40-05 | `crates/agents/device-agent/src/ingress.rs` | `metrics` 字段 Box 化 + 构造步骤诊断打印 | EnvelopeIngress 276,440 B 栈持有 → rustc 274 KiB 栈清零帧 → 16 KiB 服务栈 VM fault | 字段类型 + 构造点 | 41+ 服务 | device-agent 51 测试 + QEMU READY | 恢复栈持有（QEMU fault） |
| CVR-40-06 | `crates/agents/device-agent/Cargo.toml` | `energy-market-agent`/`eneros-fs` 可选（agent-runtime/persist-fs feature） | target 服务闭包不得携带 llama/highs/littlefs C 构建（P1-18/WP-40.4） | feature 化（默认开） | 41+ | 全仓 7308/0 | 恢复直接依赖 |
| CVR-40-07 | `crates/ai/dual-brain`、`crates/ai/infer-scheduler` | llama-cpp 依赖 feature 化（`llm-native`） | target 闭包不声明 LLM 后端（WP-40.4） | Cargo feature + 调用方启用 | 41+ | 全仓 7308/0 | 恢复直连 |
| CVR-40-08 | `xtask/src/product_manifest.rs` | `REQUIRED_SERVICE_BASELINE` + validate 检查 + `control.dispatch` kind | 蓝图 40 §7 要求删除任一 required 服务 Gate 阻断（36/37 Gate 机器化） | 常量 + validate 追加 | 41+ 服务增删须同步基线 | mutation 实弹（删除→FAIL，恢复→PASS） | 移除基线 |
| CVR-40-09 | `xtask/src/image_factory.rs` | `build_user_elf` 特判：`eneros-control-services` → `--features sel4`、`eneros-root-task` → `--features sel4-eipc` | EIPC 生产通道（Sel4EipcChannel/Sel4IpcClient）仅在 target 构建启用（host 不引入 sel4-sys）；profile 无 features 通道 | 两处包名特判（与 svc-pie 特判同模式） | 41+ 服务构建 | 全仓 host 测试 + target 构建 + QEMU | 移除特判（EIPC 不可用） |
| CVR-40-10 | `third_party/sel4-build-rust.sh` | root-task 构建加 `--features sel4-eipc` | root 驱动控制链（INGEST/TICK）的 Sel4IpcClient 需要 sel4 feature | 构建命令追加 | 41+ | QEMU CONTROL-CHAIN 实弹 | 移除 feature（驱动不可用） |
| CVR-40-11 | `crates/agents/agent-sdk/src/lib.rs` | sel4 feature 门控 `not(sel4_target)` → `not(target_os = "none")` | `sel4_target` 为 sel4-sys 的 per-crate cfg（build.rs 注入不传播到依赖者）——原门控使 sel4 feature 在 target 构建误触发，**Sel4EipcChannel 从未真正 target 编译过** | 门控条件 | 41+ | target 构建 + QEMU | 恢复旧门控（target 无法构建） |
| CVR-40-12 | `crates/drivers/drv-svc/src/lib.rs` | 新增 `now_ns()`（单调纳秒，EIPC 客户端超时注入） | EIPC 客户端 `Sel4IpcClient::new` 需要 `fn() -> u64` 纳秒时钟 | 一行函数（与 now_ms 同源） | 41+ | target 构建 | 移除（客户端超时退化） |
| CVR-40-13 | `product/profiles/qemu-prod.json` | 新增 `svc-hang`（心跳超时注入参考服务，optional）+ `svc-hang` priority=1 | 蓝图 §9 isolation 行实弹靶（svc-fault 同性质的注入参考服务）；priority=1 与 root 同优先级——任何更高优先级死循环都会饿死 root（seL4 严格优先级抢占），心跳判定无法运行 | 服务条目 + 资源 | 42 profile | QEMU HEARTBEAT-KILL 实弹 | 移除服务条目 |
| CVR-40-14 | `crates/runtime/root-task/Cargo.toml` | 新增 `eneros-agent-sdk`（EIPC 客户端+槽位常量）与 `eneros-safety-validator`（遥测样本构造）依赖 + `sel4-eipc` feature | root 作为采集链代理/编排触发器构造样本并驱动控制链 | 依赖 + feature | 41+ | 全仓测试 + target 构建 + QEMU | 移除依赖（驱动不可用） |

**承接关系（更新）**：40 收尾后 3 项 PARTIAL 全部实弹达成（EIPC 端到端投递、
supervisor 心跳 kill、QEMU 外部虚拟设备写与读回）——Exit Gate 12/12 PASS。
仍诚实登记：真实协议设备（Modbus/IEC104）与采集链的**设备侧** capability
移交（MMIO/DMA/IRQ cap 授予 EL0 服务——CVR-39-02 的剩余面）为 41 Entry 前置；
root 采集代理注入样本为移交完成前的 QEMU 装配路径（样本经 evaluator 全量校验）。
