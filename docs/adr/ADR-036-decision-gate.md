# ADR-036：W5.2 决策闸——速率+能量双闸在 coordinator 层（含 FastPath）

- 状态：已接受（2026-08-09）
- 关联：蓝图 22 §2.W5.2；锁定 2（背压）、红线4（不静默）
- 验收映射：速率超限被拒+告警 ✓；能量超限被拒 ✓

## 背景

control-gateway 有租约但无 AI 级全局闸。要求：速率上限（N 决策/分钟）
超限丢弃+告警；能量预算（周期 kWh）超限拒绝。两闸 fail-closed。

## 裁决

1. `dual-brain/src/gate.rs`：`DecisionGate` 双闸一体——
   - **速率闸**：1 分钟滑动窗口（VecDeque 时间戳）；
   - **能量闸**：滚动周期累计（周期到期重置）；
   - `check`（组合）/ `check_rate` / `check_energy` / `record` 拆分；
   - fail-closed：`max=0` = 全拒（宁可误伤不失控）；
   - 被拒计数独立（`decision.rejected.rate/.energy`，W2.4 约定）。
2. **接入点修正**（测试暴露的设计缺陷）：速率闸必须在 `execute`
   **入口**（快/慢/降级全部决策路径过闸）——初版只放慢路径，
   FastPath 会绕过闸门（t18b 失败发现）。
3. 能量估算：`|net_power_kw| × SCHEDULE_WINDOW_HOURS(0.25h)`（15 分钟
   调度窗，`schedule_energy_kwh` helper）；FastPath 同样过能量闸。
4. 记账时机：命令下发成功后才 `record`（失败不占额度）。
5. 超限返回 `DualBrainError::GateRejected(GateDecision)`（携带原因，
   调用方告警 + W5.1 审计）。

## 影响

- coordinator：+`gate` 字段（默认保守可用 60/分钟、1000kWh/小时；
  显式 `new(0, 0.0, _)` = fail-closed 全拒）+ `with_gate` builder；
- 测试：gate.rs 4 单测 + coordinator 3 集成（t18b 速率/t18c 能量正路径
  ——注入非零 LP 解/t18d 记账）。
