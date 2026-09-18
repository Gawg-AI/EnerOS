# R10 复查修复完成交接（2026-09-19）

接续 `manual-review-handoff-20260918.md`（六缺口判定）。本轮在提交 216a7e5 上全部修复并实弹验证。

## 结论

**六缺口全部修复 + R10 门 21/21 判据 PASS + 全仓 9097 测试 0 失败 + 候选链在最终提交树上闭环
（candidate-check 0 violation，仅 CORE-70-R11 BLOCKED/EXT-70-HW）。**

- 审查基线 da9e46f → 修复提交 216a7e5（代码/配置/门）→ 文档提交（登记册/本交接）。
- 每缺口均有红/绿测试或实弹判据，未以改文档/降判据换取 PASS；ADR-0054 未改动一字。

## 六缺口 → 修复摘要

| # | 缺口（复查判定） | 修复 | 验证 |
|---|---|---|---|
| 1 | 60s 控制窗违反 ADR-0054:34-35 | CONTROL_HORIZON_MS 回退 30s；演示时域 4×7s=28s；ttl=30；synth %8；新鲜度窗 30s | typed_chain 时域红绿测（超域拒/边界受/过期取消） |
| 2 | 持有输入无保留 | HeldInputRetainer（TEL 7/8 码）+ event_face 钉/剪枝跳过 + 准入 fail-closed + 终态释放 + 重启重钉 | t8 压力钉存活/释放回收（变异咬合）+ 引擎四测 |
| 3 | 实际回读不可查询 | post_apply_readback 独立读命令点 → Saga readback → HQR1 | 独立读值≠设定值红测 + 门 A4c 实弹 ×12 |
| 4 | 三资源×4时段未贯通 + 8 上限 | peer 计划级 64（65 拒）/单批 8 分离；asset3 全手术；容量链六处（PAYLOAD_MAX/配置区/MGT1/栈/堆/AI_RELAY+APP_MSG） | run14 实弹 12 分配守恒 + 门 A1-A7 |
| 5 | 读回判据误放行 | r10_readback_oracle.py 序列敏感 + 7 负例自检；守恒对账脚本；门全面重订 | 旧红例保留复现（旧=2/新=0）+ 负例集归档 |
| 6 | 候选身份未闭环 | 提交后干净树重跑马拉松 + 录制（source_commit=HEAD ∧ dirty=∅） | candidate-check 0 violation |

## 实弹调试中咬出的次生缺陷（全部已修，含根因）

1. **svc-config 12KiB 栈帧溢出**（配置区 16→24 块后 `[0u8; CONFIG_REGION_BYTES]` 栈数组）——启动
   静默冻结根因；出栈改堆。
2. **device-agent 堆 OOM**（3×276,040B MetricsCollectorImpl ingress 超 768KiB 共享堆）——1MiB bin
   专属堆。
3. **AI_REQUEST_MAX=1024 / APP_MSG_MAX_LEN=1024 双墙**（3 资产请求 JSON 实测 1079B）——零请求/
   每 tick link_err 根因；对齐 4096 + 尺寸回归锁。
4. **门 truncate 不清零旧盘**——跨运行残持有计划泄漏进新窗（A 窗判定污染）；rm+truncate。
5. **B 窗杀时机**——合并计划可在 +12s 前整体完成 ⇒ 重启窗无未发步；改飞行中杀（首 Applied+3s，
   计划先完成则等下一个）。
6. host 层 18/23 页分页传输验证（排除传输层嫌疑后定位到 mqtt 应用面）。

## 马拉松与录制（实际执行）

- 马拉松 1（提交 216a7e5 干净树）：vol1-vol7/r03 全 rc=0；vol89 与 R10 首跑失败——实弹咬出两处
  次生缺陷（应答 1664B 悬崖分片重组 + 计划在途 System1 抑制，提交 091a487）。
- 马拉松 2（提交 091a487 最终树，受影响门重跑）：vol4/vol89/R10 全 rc=0；R10 门 21/21 判据
  完整 PASS（intents=6+skipped=6=12 守恒、三资产写、读回×8≥6、readback=Some×12）。
- vol1 压力线保持：3967 轮 / 3549 agent=OK 提交（>512 原压力线）。
- 录制：`bash evidence/70/70.15/record_r10_chain.sh`（vol7 日志名对齐 vol7-evidence.log）。
- 终验：`cargo xtask evidence42 candidate-check`（无 --index，自动定位当前 HEAD 索引）。

## 剩余限制（如实）

- CORE-70-R11 BLOCKED（EXT-70-HW 实板未到货）——外部硬件依赖，非本轮范围。
- run11/run13/马拉松首跑资产 3 写失败已根因关闭：reason= 实弹定位 = SafetyRejected ramp-limit
  （|0-5|kW/0.9s=5.5kW/s 超合同 3.333kW/s，System1 与计划步交错写）；修复 = 计划在途 System1
  抑制（DISP_HELD_TICK 应答在途计数 + EA 抑制，红测变异咬合）。
- MQTT-HS ReadFailed 自检噪音全程递增为既有形态（旧证据同形），不影响连接/发布；未归因。
- 4×15s 原配置语义如 ADR-0054:34-35 要求**不被容纳**：超窗计划提交即拒（fail-closed），跨代续走
  "新快照→新认证"链（DEC-70-13 ③），本演示以 4×7s 时域收敛进 30s 窗。
