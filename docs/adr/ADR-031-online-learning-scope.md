# ADR-031 — 轻量在线学习范围：deadband 自适应 + 偏差修正（22 W2.5）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W2.5 强约束 + §1 锁定 8 预设裁决）
- **关联**：ADR-025~030、`蓝图/22` §2.W2.5/W5.3、§8 G-R2

## 背景

22 §2.W2.5：`online`/`rl`/`marl` 零调用者（mock 死代码）；v1.0.0 学习 =
基于 W2.1 偏差指标的轻量增量（LP 参数微调 / deadband 自适应 / 安全钳位
松弛收紧），**禁止重训 LLM / PyTorch FFI / 权重更新不经安全校验**；学习
输出过 safety-validator 才生效；rl/marl 标注 `v1.0.0_scope = "deferred"`。

## 决策

1. **`dual-brain::learning` 模块**（新）：
   - `DeadbandAdaptive`：遥测死区按噪声统计自调（EMA + 3σ 规则，硬界
     [min, max] 由 safety catalog 提供）；`validate` = 值域 + catalog
     边界检查（NaN/越界拒绝用旧值，`rejected_updates` 计数）。
   - `ForecastCorrector`：LP 预测系数修正（偏差 EMA → 修正系数钳位
     ±max_correction，默认 ±50%）；`correct(prediction)` 输出修正值。
2. **接入**：energy_agent 观测相（W2.1）喂偏差给两个学习器（样本/EMA
   推进）；学习输出**不直接改变控制**——deadband 供遥测层使用（W3 数据
   管道消费），修正系数供预测层使用——两者均带内置安全闸。
3. **`rl`/`marl`**：文档标注 `v1.0.0_scope = "deferred"`（不删不声称）。
4. **可执行证据**：6 + 3 测试——死区收敛/回落/非法输入拒绝/安全界钳位/
   修正器学习与钳位/agent 接入/极端输入不致灾。

## 影响

- "从执行结果学习"落地为真实轻量增量（无重训/无 FFI）；安全闸内置
  （学坏不致灾：越界拒绝 + 硬界钳位）。
- 学习状态进程内（易失）——持久化随 W3.5 存储层（learner 状态落盘为
  后续项）。

## 备选方案

- PyTorch FFI / 在线重训：锁定 8 禁止（no_std 不可行），否决。
- RL/MARL 真实化：P2 市场驱动（deferred 标注）。
