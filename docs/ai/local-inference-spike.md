# 本地推理 spike 报告（蓝图 49 WP-49.3）

> 日期：2026-08-22 · 时间盒：一个版本周期 · 性质：**可行性验证，不做产品接线**
> 关联：ADR-0045（方案 A = 本地推理，spike 验证不预设结论）；D1 裁决；
> ADR-0036（模型不入 init bundle）

## 结论：**NO-GO（v1.0 时间盒内）**

三关卡顺序执行，**关卡②（静态内存审计）对当前 QEMU 装配直接失败**，
关卡③（aarch64 no_std 交叉编译）实证移植面远超时间盒；关卡④数据已取得
（作为重启条件的基线留存）。**未向 target 闭包加入任何推理代码**
（本 spike 全部产物 = host 测量 + 文档；workspace/target 图零变更）。

## 关卡① 选型（候选清单）

| 候选 | 参数量 | 量化 | 权重体积 | 判定 |
|---|---|---|---|---|
| Qwen2.5-0.5B-Instruct | 0.5B(630M label) | Q4_K_M | 491,400,032 B（468.6 MiB，`third_party/test-model.gguf` 在库实测） | 主测对象（唯一在库实体） |
| SmolLM2-135M-Instruct | 135M | Q4_K_M | ~100 MiB（公开 GGUF 参考值，未下载） | 备选（重启条件候选） |
| Qwen2.5-0.5B | 0.5B | Q2_K | ~350 MiB（参考值） | 备选 |
| Llama-3.2-1B / TinyLlama-1.1B | ≥1B | Q4 | >700 MiB | 出局（>0.5B 红线） |

## 关卡② 静态内存审计（内存优先铁律：先审计后跑）——**当前装配 FAIL**

实测输入（host x86，llama.cpp b4000 静态链接，Qwen2.5-0.5B q4_k_m 全量载入）：

| 项 | 数值 | 依据 |
|---|---|---|
| 权重常驻 | 468.6 MiB | 模型文件字节数（q4 全量载入语义） |
| KV cache | 12 KiB/token（n_layer=24 × n_kv_head=2 × head_dim=64 × K/V × f16）；ctx 2048 = 24 MiB | GGUF metadata（runner-run.log） |
| 激活/计算缓冲 + 运行时 | ~85-95 MiB | host 实测 max RSS 574 MiB − 权重 − KV 反推 |
| **合计常驻** | **≈ 574 MiB（实测）/ ~593 MiB（ctx 2048 理论）** | `/usr/bin/time -v` |

对照预算：

| 环境 | 可用预算 | 判定 |
|---|---|---|
| QEMU virt 当前产品装配 | guest RAM 128 MiB（kernel/root/30 服务已占其中固定区） | ❌ **FAIL（差 4.5×）**——现有装配无法承载，扩 guest RAM 属平台装配变更非 spike 权限 |
| 目标板（按 2G 规划，51 卷冻结前待定） | OS+数据面后估算剩 ~1.5 GiB | ⚠️ 纸面可容 0.5B q4，但未经任何实机验证（D3：真机项 BLOCKED） |
| init bundle | 8 MiB 映射窗（49 收口刚扩），模型 468.6 MiB | ❌ 永久排除（ADR-0036 成立）→ 必须数据盘 + model-ota 供给链 |

**判定：内存关卡在可验证环境内 FAIL。** 2G 板卡纸面通过但属未验证推断，
按「验证大于声称」纪律不能计 PASS。

## 关卡③ aarch64 no_std 交叉编译可行性——**工程量超时间盒**

依赖清点（`nm` 实测，见 `evidence/49/spike/symbol-inventory.txt`）：

- **libggml.a**（undefined 308）：动态分配 13 类调用点（malloc/free/realloc/
  calloc/posix_memalign）、文件 IO（fopen/fclose）、线程面全套（pthread
  mutex/cond/create/join、affinity×2、sched_yield）、fork/exit/getenv/
  clock_gettime —— no_std 替换需自写分配器 shim + 单线程重写线程层 +
  时钟/文件注入 seam，数十个调用点逐项改造。
- **libllama.cpp 上层**（undefined 698）：C++ 异常（__cxa_* 五类）、STL
  （_Rb_tree/bad_alloc/throw_*）、pthread_once/mutex、mmap/munmap ——
  no_std 路径需要完整 C++ unwind 运行时或对上层做大规模重写。
- **ISA 兼容性本身无障碍**：`aarch64-none-elf-gcc -c ggml.c -ffreestanding`
  （时钟/线程桩后）推进至类型系统冲突即止，29 个 error 全部为 POSIX 面
  缺失（`evidence/49/spike/aarch64-freestanding-errors.log`），无 ISA
  不兼容证据；NEON 内核（ggml-aarch64.c）为可选加速路径。

**判定：FAIL（时间盒意义下）。** 「裁剪 ggml 核心」的真实工作量 =
线程层重写 + 分配器/文件/时钟 shim + 上层 C++ 运行时问题，是季度级工程，
不是版本周期级 spike；即便完成，仍被关卡②内存判据挡在当前装配外。

## 关卡④ host 原型延迟测量（WCET 输入，重启条件基线）

实测（runner 真后端 G-SW1 链路，scenario=normal，--demo-trusted-feedback）：

- 端到端 wall clock **3.21 s**（含 468.6 MiB 模型载入 + prompt eval +
  生成 + LP 求解 + 安全校验；Exit=0，sink 写入真实 proposal）
- LLM 输出为合法意图 JSON（中文，Qwen2.5 真推理，runner-run.log 存档）
- `latency_ms` 分解字段在 host 构建恒 0（eneros-time 未初始化的已知
  host 怪癖，如实登记）——分解计时以 target 侧重测为准

WCET 推算（保守）：target 单核 A53 类算力 ≈ host 桌面核 1/3–1/5 →
推理段秒级～十秒级，**超出控制回路预算**（svc-control-dispatch 3s 就绪
死线已是已知缺陷面，48 卷登记）；远程 peer 方案下 target 侧等待有界
（5 s 超时降级，49.2 实弹验证）。

## 重启条件（任一满足可重开 spike）

1. **换板带 NPU/加速器**（51 卷选板判据输入：≥2 TOPS NPU + ≥2 GB 独立
   内存域），经模型供给链（model-ota 数据盘）实机验证；
2. ≤135M 级模型（SmolLM2 类）在目标板上静态内存审计通过且推理延迟
   ≤ 决策周期（需先有真机）；
3. 出现可用的 Rust no_std 推理核心（如 Candle 类）覆盖目标架构，免除
   C++ 运行时问题——届时仅重跑关卡②③。

## 宣称纪律（ADR-0045 #4 重申）

本 NO-GO 不改变 v1.0 AI 宣称路径：**「AI 辅助决策（需配套站端服务）」**
（ADR-0045 方案 B，WP-49.2 实弹接线）；禁止宣称「设备端本地大模型」；
host llama.cpp / runner 仅为主线联调工具与本次 spike 测量载体，不得声称为
目标侧能力。

## 原始证据清单

- `evidence/49/spike/symbol-inventory.txt`（nm 符号清点全文）
- `evidence/49/spike/aarch64-freestanding-errors.log`（交叉编译错误实录）
- `evidence/49/spike/freestanding-shim.h`（冒烟桩，仅证 ISA 非移植方案）
- `evidence/49/spike/runner-run.log`（host 真后端测量原始输出，RSS/wall/
  LLM 输出全文）
