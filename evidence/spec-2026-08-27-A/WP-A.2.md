# WP-A.2 AI 电气护栏接入决策咽喉（SPEC-SERIES-2026-08-27/A）

## 判据结果

| 判据 | 结果 | 留证 |
|------|------|------|
| C1 host 单测 ≥6 | ✅（8 例） | engine.rs unit_tests：`guard_rejects_overvoltage/undervoltage/frequency_out_of_window/nan_telemetry_fails_closed/power_over_cap/ramp_conflict` + `guard_allows_nominal_dispatch` + `guard_projection_precedence_is_structural` |
| C2 QEMU 正例照常打穿 | ✅ | 同 WP-A.1 C3 镜像：MASTER-WRITE=171 / Committed=171 / SAFETY-REJECTED=0（守卫在岗零误伤）；audit 链 Passed 面由 47 既有 AUDIT-CHAIN 机制承载（本卷不改 MemStore 持久性，E 卷工作） |
| C3 越窗拒 target 侧 | ⚠ PARTIAL（环境受限登记） | SAFETY-REJECTED 串口行接线在码（engine.rs execute_core 步骤 4.5 拒绝分支 `[svc-control-dispatch] SAFETY-REJECTED reason={reason}`，image 内编译实证）；本环境无遥测注入面（TSDB 由 guest 内部采集，无外部改值通道），越窗快照不可构造 → 同 D 卷"无第二串口"先例登记 BLOCKED（owner 提供注入面后可补）。host 侧 8 例覆盖电压/频率/NaN/功率帽/爬坡全拒路径 |
| C4 无退化 | ✅ | 全量 7704→（最终计数见卷尾）；触达包 clippy 零警 |

## 装配要点

- 单咽喉：`execute_core`（run/run_intent 合核，WP-54.2）在 Authorized 与 rate-reserve 之间插入步骤 4.5——两翼同过。
- `project_system_state`：受信任遥测退化（NaN/±Inf/零时间戳）→ `SystemState::unknown()` fail-closed（不 panic 不猜值）。
- `SafetyGuardParams` + `safety_guard`：电压/频率窗口显式裁决（safety-validator 的 voltage/freq_range 为 D12 预留未消费，故上移显式化）+ ElectricalSafetyRule(max_power 复用 DispatchConfig) + ProtectionCoordinationRule(爬坡可配置) + 同时充放 Fatal 后置；任一违规/钳位 → Err 摘要。
- DispatchConfig 新增 `safety_voltage_window(340,420)/safety_freq_window(49.5,50.5)/safety_ramp_kw_per_min(200)`，×5 构造点同步（缺省=校验器蓝图默认，行为连续）。
- 拒绝路径：`DispatchError::SafetyRejected(reason)` + `audit(ExecOutcome::Failed, SafetyOutcome::Blocked, …)`（Blocked 真实置位）+ SAFETY-REJECTED 串口行。
- SafetyValidator 新增 `empty()` 构造器（参数化注册，不隐含演示默认）。
