# Dead-Surface 审计清单（蓝图 23 · W1）

> 产物性质：W1.1/W1.2 的逐 crate dead 项裁决落档。
> 纪律：禁 crate 级 blanket `#![allow(dead_code)]` 作为交付态。每个 dead 项必须有裁决（在用 / 计划标注 / 删除）。
> 裁决依据：§1 锁定 1（interface 诚实原则）——所有 `pub` 必须满足 ① ≥1 非测试调用方 或 ② `#[doc(hidden)]` + 注释保留意图 + 进本清单。
> 构建方法：每个 crate 用默认 feature + `--features mock`（如适用）双跑，`cargo build -p <crate>` 收集 `dead_code` 警告。

## 事实基线纠偏（相对规格 §2 W1.1）

- 规格称"ai 子树 12 crate + agent-sdk 共 12 个 blanket allow"。
- 实测（grep `#![allow(dead_code)]`）：**ai 子树 11 个 crate** 有 crate 级 blanket allow（solver-core/milp/pareto/warm/model/energy-lp-model/safety-validator/infer-scheduler/model-deploy/gguf-loader/llm-engine）。
- `crates/agents/agent-sdk/src/lib.rs` **无 crate 级 blanket allow**（仅有 `ipc.rs`/`wire.rs` 内逐项 allow，符合纪律，不在 W1.1 范围）。
- 本审计按代码事实操作 11 个 ai crate。agent-sdk 已符合，不动。

## 逐 crate 裁决

### 1. solver-core ✅

- **位置**：`crates/ai/solver-core/src/lib.rs:51`（已删除 `#![allow(dead_code)]`）。
- **构建复核**：
  - `cargo build -p eneros-solver-core --features mock` → 零 dead_code 警告。
  - `cargo build -p eneros-solver-core`（默认 highs-ffi）→ 零 dead_code 警告。
  - `cargo build -p eneros-solver-core --tests` → 零 dead_code 警告。
- **消费者**：16 个 crate 依赖 solver-core（coordinator/mvp-scenario/solver-warm/fast-path/energy-lp-model/energy-market-agent/runner/safety-validator/twin-global/dual-brain/intent-contract/intent-parser/solver-milp/solver-pareto/solver-model 等）。
- **裁决**：**无 dead 项**。D4 偏差声明提到 `SolverError` 的 FFI 变体（FfiError/PassFailed/RunFailed/ParamError/ParamSetFailed）默认 mock 构建不可达——但编译器未报 dead，因这些变体经 `Display` impl 与 pub 暴露被判定为在用。无需逐项 allow。
- **结论**：移除 blanket allow，零后续动作。编译器成为守门人。

### 2. solver-milp ✅
- **位置**：`crates/ai/solver-milp/src/lib.rs:44`（已删除 blanket allow）。
- **lib 构建复核**：`cargo build -p eneros-solver-milp` → 零 dead_code 警告。
- **裁决**：无 dead 项。

### 3. solver-pareto ✅
- **位置**：`crates/ai/solver-pareto/src/lib.rs:46`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 4. solver-warm ✅
- **位置**：`crates/ai/solver-warm/src/lib.rs:47`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 5. solver-model ✅
- **位置**：`crates/ai/solver-model/src/lib.rs:40`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 6. energy-lp-model ✅
- **位置**：`crates/ai/energy-lp-model/src/lib.rs:37`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 7. safety-validator ✅（有逐项标注）
- **位置**：`crates/ai/safety-validator/src/lib.rs:43`（已删除 blanket allow）。
- **lib 构建复核**：删除 blanket allow 后出现 2 个 dead_code 警告，逐项裁决后清零。
- **dead 项 + 裁决**：
  - `ElectricalSafetyRule.{max_current_a, voltage_range, freq_range}`（electrical.rs:23-25）：**计划标注**。领域预留——电流/电压/频率是充电桩/储能真机保护量，当前 LP 模型只校验功率与 SOC。`new()` 已暴露参数，`default_safety_validator()`（validator.rs:37）已实例化。逐项 `#[allow(dead_code)]` + 注释 `W1.1: 待真机对接`。
  - `ProtectionCoordinationRule.{overcurrent_threshold, overvoltage_threshold, undervoltage_threshold, freq_protection}`（protection.rs:20-23）：**计划标注**。同上，保护配合策略预留。逐项 allow。
- **依据**：§1 锁定 4 例外 ②（目标侧暂未装配但计划要）。

### 8. infer-scheduler ✅
- **位置**：`crates/ai/infer-scheduler/src/lib.rs:52`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 9. model-deploy ✅
- **位置**：`crates/ai/model-deploy/src/lib.rs:53`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 10. gguf-loader ✅
- **位置**：`crates/ai/gguf-loader/src/lib.rs:52`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 11. llm-engine ✅
- **位置**：`crates/ai/llm-engine/src/lib.rs:43`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

## W1.1 既存缺陷（非本规格引入，记录备查）

以下 test 编译错误在 baseline（d55fc98，移除 allow 前）已存在，与 W1.1 无关：

- `solver-milp`/`solver-warm`/`solver-model`：`cargo build -p ... --tests` 报 `unresolved import eneros_solver_core::mock`。需 `--features mock`（各 crate 已定义 `mock = ["eneros-solver-core/mock"]` feature，但 `--tests` 不自动启用）。`cargo build -p ... --features mock --tests` 通过。
- `energy-lp-model`：`Cargo.toml` 无 `[features]` 段，无法启 mock；其内嵌 test（lib.rs:50 引用 `MockSolver`）在 baseline 即编译失败。

这些是 feature 配置缺陷，非 dead-surface 问题。蓝图 23 是结构等价变换，不在 W1 改 feature 行为。G-I 总出口回归时需用正确 feature 组合（见 §闸门）。

## W1.1 验收复核（grep 证据）

```
$ grep -rn '^#!\[allow(dead_code)\]$' crates/ai/*/src/lib.rs
(空——11 crate 无 crate 级 blanket allow)

$ 逐 crate lib 构建（默认 features）：dead_warn=0 errors=0（全 11 crate）
```

---

## W1.2 非 ai crate 裁决（7 个）

> 规格称"6 个"，实测全仓扫描为 **7 个**（protocols 4 + drivers 3，含 safety-catalog）。

### 12. agent-bus-dds ✅
- **位置**：`crates/protocols/agent-bus-dds/src/lib.rs:69`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。（注：cyclone_dds 骨架问题在 W3.3 单独处理，不靠 blanket allow 掩盖。）

### 13. control-gateway ✅
- **位置**：`crates/protocols/control-gateway/src/lib.rs:22`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 14. safety-catalog ✅
- **位置**：`crates/protocols/safety-catalog/src/lib.rs:27`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。

### 15. tsn-time ✅
- **位置**：`crates/protocols/tsn-time/src/lib.rs:139`（已删除）。
- **lib 构建复核**：零 dead_code 警告。
- **裁决**：无 dead 项。（W3.4 处理 trait-only scope 标注与测试搬出。）

### 16. fs ✅（依赖 storage，自身无 dead）
- **位置**：`crates/drivers/fs/src/lib.rs:68`（已删除）。
- **lib 构建复核**：零 dead_code 警告（其依赖 storage 的警告已在 storage 本 crate 处理）。

### 17. storage ✅（删除 2 项真死代码）
- **位置**：`crates/drivers/storage/src/lib.rs:56`（已删除）。
- **lib 构建复核**：删除 blanket allow 后暴露 2 个 dead_code 警告，就地清理后清零。
- **dead 项 + 裁决**：
  - `BadBlockTable.reserved_count`（bad_block.rs:29）：**删除**。构造时算 `reserved_start = total - reserved_count` 后，总数不再被读（`reserved_start` 已记录起点，完整性约束在 `new()` 的 assert 完成）。字段冗余，删除。
  - `virtio_blk.rs:482 pa_of()`：**删除**。物理地址转换方法，零调用方（配对的 `va_of` 在用）。真死代码。
- **回归验证**：`cargo test -p eneros-storage --lib` → 128 passed; 0 failed（含 bad_block + virtio_blk 测试）。

### 18. tsdb ✅（依赖 storage，自身无 dead）
- **位置**：`crates/drivers/tsdb/src/lib.rs:95`（已删除）。
- **lib 构建复核**：零 dead_code 警告。

## W1.2 验收复核（grep 证据）

```
$ grep -rn '^#!\[allow(dead_code)\]$' crates/*/src/lib.rs crates/*/*/src/lib.rs
(空——全 18 crate 无 crate 级 blanket allow)

$ cargo test -p eneros-storage --lib → 128 passed; 0 failed
```

## G-I1 根因闸门判定

- ✅ 全仓 18 个 crate（11 ai + 7 非 ai）无 crate 级 blanket `#![allow(dead_code)]`。
- ✅ dead 项清单逐项裁决落档：safety-validator 7 字段（计划标注）、storage 2 项（删除）、其余 15 crate 无 dead。
- ✅ 无新增 crate 级 blanket allow。
- **G-I1 通过**：编译器重新成为 dead surface 守门人。

---

## W2~W5 完成态摘要（蓝图 23 全工作流）

| 工作流 | 裁决/动作 | ADR | 验证证据 |
|---|---|---|---|
| W2.1 寄存器单一持有 | `initial_registers` 单点（count=5 契约），两处内联布局删除 | — | root-task host 49 测试过 + aarch64 编译过 |
| W2.2 删影子 API | `dispatch_frame`/`DispatchAction`/`config_service_roundtrip`/`service_name`/`load_services_from_bundle` 删除；MR 编解码真实路径测试补 1 个 | — | root-task 44→41 测试过（-6 影子 +1 真实 +...）；aarch64 编译过 |
| W3.1 federation | `SecureTransport` 删，`FederationChannel<D: VerticalEncryptDevice>` 泛型化；`real.rs::TlsTunnelDevice` 删 | ADR-0038 | 233 测试过；vpp-scenario/pentest 构建过 |
| W3.2 net | `Socket` trait（0 实现者）删 | — | 442 测试过；6 消费者构建过 |
| W3.3 dds | `cyclone_dds.rs`/`ffi.rs` 删（空骨架），`cyclone-dds` feature 移除 | ADR-0039 | 64 测试过 |
| W3.4 tsn | `TsnDriver`/`NicApplier` 标 trait-only scope；`SoftSyncClock` 删（逻辑收敛 `SoftSyncClaim::verify_claim`） | ADR-0040 | 91 测试过 |
| W4.1 energy-market | re-export 收缩（零消费者符号降级 cfg(mock)）；`build_lp_problem`/`build_weighted_lp` 合并 | ADR-0041 | 207 测试过；coordinator 120 / mvp 24 / cloud 85 全过 |
| W4.2 dual-brain/audit | 保留（`DecisionAudit` = security/audit 的决策语义门面，非重复体系） | ADR-0042 | 55 测试过（含 gr5 集成） |
| W4.3 root-task | `PhysPageMap`/`map_physical_pages`/`frame.rs` 删；`SpawnPlan.root` 降级 `SpawnRoot{name,stack_top}` | — | 41 测试过；aarch64 编译过 |
| W5.1 Period | 定义移到 market-types，market_feed re-export；CONTEXT.md §2 补条目 | — | 207 + 7 测试过；coordinator/cloud 构建过 |

**G-I 总出口验证（最终）**：
- `cargo test --workspace` → **326 套件 / 6828 测试全过，0 失败**。
- 全仓零 crate 级 blanket allow（grep 复核）。
- **QEMU 系统级运行**（`target/g1-evidence/qemu-serial.log`）：spawn 10 服务成功 + IPC hello 循环 150 tick 稳定 + **supervisor 完整闭环**（svc-fault fault → badge=4 识别 → restart 1000ms → restarted → svc-hello 隔离复验 → 再次 fault 循环）。
- 22 G-R1 遗留 4 bug 修复（bootctrl Crc 阻断期从未系统级验证）：① `tcb_bind_notification(tcb,2)`→`(tcb,ntfn)`；② fault badge 错位（notification→fault_ep mint，root 侧带 badge 副本）；③ `seL4_Poll` 无消息返回垃圾 → `sender!=0` 判据；④ `time_init` 缺失 → Arm64Timer + 内核 `KernelArmExportPCNTUser=ON`。

---

## 完成态（2026-08-09）

蓝图 23 全部 12 个工单完成。DoD 五条：

1. ✅ **编译器是守门人**（W1）：全仓 18 crate 无 crate 级 blanket allow，dead 项逐项裁决落档。
2. ✅ **不变量单一持有**（W2）：寄存器布局（initial_registers）、帧分发影子 API 已收敛/删除。
3. ✅ **seam 纪律统一**（W3）：ADR-0038/0039/0040 落档，federation/net/dds/tsn 无空骨架冒充。
4. ✅ **interface 诚实**（W4）：ADR-0041/0042 落档，暴露即真实在用或明确标注。
5. ✅ **领域词收敛**（W5）：Period 归位 market-types，CONTEXT.md 更新。
- 新增 ADR：ADR-0038 ~ ADR-0042（5 个）。
- QEMU 系统级回归 PASS（含 22 G-R1 遗留修复后的完整 supervisor 闭环）。

---

<!-- W2 起步 -->



---

## CF-01 登记（2026-08-11，28 系列蓝图 29 WP-29.2）

**事实：configs/ 目录 82 个 toml 中仅 `control-write.toml` 被真实加载
（crates/agents/device-agent/src/assembly_demo.rs，且仅测试路径）；其余 81 个
为纯文档模板，无 Rust 解析代码。**

- 处置（29）：`config-syntax` 门禁上线（host 侧 toml 全量解析，语法错 FAIL，
  含 compliance/、power/、time/ 子目录）；82 个文件头部全部声明
  `# consumer: <crate::path | 集成层占位（产线注入）>`；`config-refs` 门禁
  扩展校验 consumer 声明存在（S6：配置存在但无消费者必须显式声明）。
- **go-no-go 口径：不得将 configs/ 宣称为活配置**。产线注入为集成层动作，
  配置键值未经代码消费不构成能力证据（宣称一致性，G7）。
- **34 SE-11（决策②）**：agent-bus-dds 签名层（signing.rs + configs/
  signing_keys.toml + sm2 feature）已退役删除——零生产消费（grep 实证）；
  "传输层已有等价保护"依据：agent-bus-dds 标注 out-of-v1.0（ADR-021：
  v1.0.0 agent 总线改用 MQTT 真实可用），跨节点消息真实性由 MQTT/
  上层安全机制覆盖；go-no-go 不得宣称 DDS 消息签名能力。
- 遗留（34 域）：consumer 声明"深度校验"（声明的消费者存在性，GV-15）、
  孤儿模板清理（CF-02/03）在蓝图/34 关闭。

- **32 AG-03（决策 B）**：5 个市场/优化 TOML（configs/bid_strategy.toml、
  spot_market.toml、realtime_market.toml、domain_optimizer.toml、
  multi_objective.toml）标注 `# status: deferred (v1.x)`——零生产消费
  （grep 实证，对应符号仅测试使用）；蓝图/34 验收**不得**声称具备该能力。
  修复前状态：这些 toml 无任何消费方（audit 0804 登记）；32 采纳决策 B
  （S6/S8 纪律优先），决策 A（energy-market-agent 补 load_market_config
  闭环）待产品确认后另开工单。
