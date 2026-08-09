# ADR-030 — LLM 有界延迟：InferScheduler 接入与超时降级（22 W2.3）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W2.3 强约束）
- **关联**：ADR-025（alloc_error_handler）、`蓝图/22` §2.W2.3、§8 G-R2

## 背景

22 §2.W2.3：coordinator 同步调 `llm_engine.infer()` 无超时；`InferScheduler`
（有 `is_timed_out`）存在但链路不使用。要求：LLM 调用经 InferScheduler 带
deadline；首次超时回退 fast-path；连续超时告警 + 降级模式；禁止单个 infer
阻塞超过 deadline；`InferParams` 增加 timeout 字段。

## 现状侦察（2026-08-09 代码为准）

1. `InferScheduler` 是异步调度器（`submit` + 轮询 `tick`；`tick` 内同步调
   `engine.infer`——单请求场景 tick 即执行）；`InferRequest::with_timeout`/
   `is_timed_out` 实现 deadline 检测（队列清理步）。
2. `InferParams` 无 timeout 字段；coordinator 慢路径直接 `engine.infer`。
3. 同步引擎（llama.cpp FFI）无法被抢占——**单次 infer 无法中断**。

## 决策

1. **coordinator 持有 `InferScheduler`**（`llm_infer_bounded`）：
   `submit(req)`（`with_timestamp` + `with_timeout(params.timeout_ms)`）→
   循环 `tick`（deadline 检测用**当前**时钟，每次轮询推进）→ 请求完成
   （成功/超时/失败）返回。**排队/调度不无限阻塞**；单次 infer 的时长由
   `params.max_tokens` 预算约束（同步引擎不可抢占——v1.0 能力边界，
   真抢占需异步推理后端，超出 no_std 单线程范围）。
2. **`InferParams.timeout_ms`**（默认 5000）：deadline 注入点。
3. **超时策略（execute 慢路径）**：
   - 首次失败/超时：回退 `fast_path.execute`（真查表+微调），
     `PathType::FastPath`，决策不阻塞；
   - 连续超时（`llm_timeout_streak >= 2`）：降级模式——用
     `last_successful_schedule`（上一次成功决策，安全钳位由 validator
     保证），`PathType::Degraded` + `degraded` 标记（上层告警/审计）；
   - 无上次成功决策时回退 fast-path。
4. **可执行证据**：`AlwaysFailEngine`（卡死/慢模型模拟）测试——LLM 失败
   → fast-path 回退不阻塞；连续调用不 panic；`timeout_ms` 默认 5000。

## 影响

- LLM 慢/卡死不再阻塞决策（有界等待 + 回退/降级链）；`PathType` 新增
  `Degraded` 变体（fast-path crate）。
- 调度器统计（timeout/失败计数）经 `InferScheduler::stats` 可观测
  （W4.1 指标接入点）。

## 备选方案

- **异步 LLM 后端（可抢占推理）**：v1.0 外（no_std 单线程 + llama.cpp
  同步 FFI）；ADR 记录为协议演进项。
- **超时 = 硬错误（不回退）**：违背"决策不阻塞"（W2 目标），否决。
