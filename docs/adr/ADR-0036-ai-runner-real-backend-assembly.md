# ADR-0036：A1 eneros-ai-runner 真后端装配方案（断链 A 端到端入口）

- 状态：已接受（2026-08-09，21-B A1）
- 关联：蓝图/21-B §2.A1；ADR-0023（seL4 基线）；ADR-0034/0035（G-KERNEL spawn/IPC）
- 背景蓝图：`蓝图/21-B-商业化解阻塞-P0软件侧清偿开发蓝图.md`

## 问题

断链 A：AI 链零件全真（LlamaCppEngine / HighsSolver / SafetyValidator / 意图解析），
但从未有任何一个 release 二进制装配过它们——`DualBrainCoordinator::new`（coordinator.rs:107）
接受注入但无人注入。需要新增 `crates/ai/runner` 消灭"AI 链从未整机装配过"。

## 侦察确认的事实（以代码为准，蓝图偏差已对齐）

1. `DualBrainCoordinator::new(config, llm: Box<dyn LlmEngine>, solver: S, sink: Box<dyn CommandSink>)`
   签名与蓝图一致；但 `LlamaCppEngine::new(device)` 是**不可失败构造**（蓝图写 `new(...)?`，
   实际 `load_model(path) -> Result<(), LlmError>` 才是失败点）。
2. `SafetyValidator` 由 coordinator 内部 `new()` 注册（ElectricalSafetyRule + ProtectionCoordinationRule），
   不需要外部装配——蓝图"装配 SafetyValidator"在 coordinator 内已成立。
3. `energy_agent.rs:81` 的 `new_default` **已带** `#[cfg(any(test, feature="mock"))]` 门控
   （蓝图称"未门控"——该缺陷已在 R2-1 批次修复）；`EnergyAgent::new<S2: Solver>` 是泛型未门控
   构造器，即 A2 的 `new_real` 语义已存在，A2 仅需验收 + 命名对齐。
4. 模型文件实际位于 `third_party/test-model.gguf`（491MB，Qwen2.5-0.5B-Instruct-q4_k_m），
   蓝图写的 `third_party/llama.cpp/test-model.gguf` 不存在——路径以代码为准。
5. `PathSelector::select` 首次运行必走 SlowPath（`last_slow_path_ms == None`），
   runner 第一次 execute 即完整 7 步链路，无需额外强制。
6. Fatal 触发路径：`ElectricalSafetyRule` 硬编码 `soc_pct < 0.05 → Severity::Fatal`
   （electrical.rs:109-120），LP 的 SOC 边界 = `[soc_min·cap, soc_max·cap]`（kWh）。
   **真后端下构造 SOC<0.05 的确定性路径**：`ScheduleConfig.soc_final = Some(0.02)`
   （终值 SOC 约束）→ 末时段求解结果 soc_pct=0.02 → Fatal。无需 mock 注入。

## 决策

**crates/ai/runner 结构**：二进制 + 库（`lib.rs` 导出 `run_once` 便于测试，`main.rs` 薄壳装配）。

1. **装配**（main.rs 唯一入口，零 mock）：
   - `LlamaCppEngine::new(ComputeDevice::Cpu)` + `load_model(--model)`（真 llama.cpp）
   - `HighsSolver::new()?`（真 HiGHS）
   - `FileCommandSink`（本 crate 新建真实 sink：JSON Lines 落盘 + write_count 指标）
   - `DualBrainCoordinator::new(config, llm, solver, sink)`
2. **CLI 注入**（禁止硬编码）：`--model`（必填）、`--scenario normal|fatal-soc`、
   `--sink <path>`、`--price <csv>`（可选覆盖）、`--soc-min`（可选覆盖）。
3. **场景**：normal（低价激励充电的"谷电时段充电至 80%"语义，期望 sink 写入）；
   fatal-soc（`soc_final=Some(0.02)`，期望 `DualBrainError::SafetyFatal` + sink 零写入）。
4. **feature 纪律**：runner 不声明 mock feature；依赖链（dual-brain → llm-engine/llama-cpp、
   solver-core default highs-ffi）自带 release compile_error 熔断（lib.rs:56-66、solver-core lib.rs:64-72）。
5. **OS 承载标注（A3 细化）**：A1 本期在 host 侧闭环（llama/highs 静态链接原生
   二进制）。EL0 spawn 承载（runner 作为 EL0 服务经 G-KERNEL spawn + IPC）的
   现实约束与路径：
   - **约束**：test-model.gguf 491MB 无法进 init bundle（当前 213KB）；
     本机 x86_64 无法直接交叉运行 aarch64 llama 推理（QEMU TCG 模拟 0.5B
     推理单次分钟级）。
   - **承载路径（G-SW1 完成后的演进）**：① 决策核心（意图→LP→求解→安全→
     sink）交叉编译为 EL0 服务（HiGHS 纯 C++，aarch64 交叉编译成熟）；
     ② LLM 感知段作为 host/边缘模型服务，经 virtio-net 或 IPC 向 guest
     供给 intent JSON（产品形态：边缘 LLM 服务与调度服务分离）；③ 模型
     文件经 data 盘/网络加载（boot 链已有 virtio-blk 数据盘）。
   - **本期证据**：host 端到端闭环（A1 证据 + manifest 哈希锚定）+ QEMU
     承载能力（G-KERNEL 13/13：EL0 服务 spawn + IPC 闭环）——两者联合
     构成"OS 可承载决策链"证据，REQ-GS1-AI-001 注册入 evidence registry。

## 验收映射（21-B §2.A1 四条）

| 判据 | 实现路径 |
|---|---|
| 1. release 构建成功 | `cargo build --release -p eneros-ai-runner`（默认 features 真后端） |
| 2. 端到端非空 Intent → LP → 解 → 安全 → sink 写命令 | `--scenario normal` 真实模型推理 |
| 3. 零 mock | main.rs 无 mock 引用；`strings \| grep -i mock` 验证；依赖链 compile_error 熔断 |
| 4. Fatal 阻断 sink | `--scenario fatal-soc` → SafetyFatal + sink 零写入 |

## 风险

- LLM 输出 JSON 不合 ContractValidator 校验（小模型）：prompt 模板已强约束格式；
  normal 场景验收若 JSON 解析失败，以 LLM 真实输出为准调整温度/上下文（不改 mock）。
- Highs 求解 96 时段 LP 耗时：time_limit=0.5s 已由 coordinator 设置，可接受。

## 验收实战发现与修复（2026-08-09，端到端跑通过程）

1. **AVX-VNNI SIGILL（环境缺陷，修复在 llm-engine build.rs）**：libllama.a 以
   `-march=native`（GGML_NATIVE=ON）编译，携带 `vpdpbusd`（AVX-VNNI）指令；
   Zen3+ 运行机（Ryzen 6800U 无 AVX-VNNI）推理时 SIGILL（exit 132）。
   修复：build.rs cmake 显式 `-DGGML_NATIVE=OFF`（官方默认，可移植基线），
   本地重建 libllama.a 后 objdump 验证零 vpdpbusd。影响：任何 -march=native
   污染产物都会在异构机器上崩溃——CI 构建机/运行机指令集必须一致或关 native。
2. **字符串数字（真实 LLM 输出适配，修复在 intent-parser）**：Qwen2.5-0.5B
   输出 `"start_period": "0"`（数字带引号）→ `expected usize` 解析失败。
   修复：`normalize_llm_json` 新增递归 `coerce_string_numbers`（纯数字字符串
   → JSON 数字；非数字字符串保持，由契约校验拦截）。3 个新测试。
3. **UTF-8 跨 token（真实 llama.cpp 行为，修复在 llm-engine llama_cpp.rs）**：
   `llama_token_to_piece` 把多字节字符拆到相邻 token piece，逐 piece
   `from_utf8` 误报 `Utf8Error`。修复：跨 token 字节累积缓冲，仅完整 UTF-8
   序列才回调。中文输出由此可正常收集。
4. **prompt 业务引导（增强在 prompt-template）**：0.5B 模型在无决策规则时
   输出 Hold（与谷电充电场景矛盾 → LP 不可行 → 96 Fatal）。模板增加
   业务决策规则（低价→Charge / 高价→Discharge）+ few-shot 示例后，
   normal 场景稳定输出 Charge。模板接口不变，dual-brain 测试不受影响。

## A2 裁决（与 21-B §2.A2 判据 2 的字面差异）

蓝图 A2 判据 2"eneros-ai-runner 改用 new_real"与 A1 强约束"入口必须用
`DualBrainCoordinator::new` 装配"存在张力。裁决：**A1 优先**（G-SW1 入口
强约束字面），runner 保持 `DualBrainCoordinator::new` 直连；A2 的
`EnergyAgent::new_real` 作为 agent 层真后端装配点，新增端到端测试
`a2_new_real_with_highs_solver_end_to_end`（真 HighsSolver + on_tick 产出
调度方案）证明"真后端经 agent 构造器装配跑通"。理由：两条验收都闭环，
不互相破坏强约束；影响：A2 判据 2 以 agent 级端到端测试 + A1 端到端
证据联合满足。

