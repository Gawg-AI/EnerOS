# 49 卷 Exit Gate 自证

> 日期：2026-08-22 · 版本：0.313.34→0.313.35 · 状态：**Accepted**
> （ADR-0045 owner 签字 = BLOCKED（owner 动作）如实登记；其余 Gate 全项自证 PASS）
> 终提交 hash 见本文件落盘提交（exit 提交）；WP 链 = 3f876f4(.31) → cb4bb49(.32)
> → 4de0365(.33) → 752e529(.34)。

## 卷级事实（Entry 判定的延续）

- 前置 47 卷 Exit 已达成（状态板 Accepted @0.313.24）；上卷回归照抄实跑
  全过（104 家族 231 / CF01 active=17→18 / l2×6 / w40-key=0）。
- 锚点修订一处：`svc-control-dispatch.rs:111`→实际 :348/:358（行号漂移，
  实质相符），WP-Entry.md 登记。
- ADR-0045 不停等条款执行：Draft 状态直接按方案 B 实施，签字为 Exit
  登记项——**BLOCKED（owner 动作）如实登记于 exit-gate.json**。

## Exit Gate 逐项

| # | 项 | 结果 | 证据 |
|---|---|---|---|
| 1 | ADR + 三态证据 + 越权负例 | **PASS**（签字子项 BLOCKED 登记） | ADR-0045 落库+registry 41 entries；HEAD 复跑（镜像 ff4dd031…）四态 tick 级单跑全呈现：System2 消费 ×11（path=Slow）→ 越权值 `Range("plan.power_kw","exceeds budget")` 拒绝 ×1 → `peer-unhealthy consecutive=3` 启发式接管 → `peer-recovered` 回 AI；dispatch=Committed 连续、审计链 dropped=0。串口+transcripts 入库 |
| 2 | spike 报告存在且结论明确 | **PASS** | NO-GO（内存关卡当前装配 FAIL + 移植面超时间盒 + WCET 基线）；重启条件三条；`docs/ai/local-inference-spike.md` + spike 原始数据 4 件 |
| 3 | target 闭包零死重 + 体积对比 | **PASS** | coordinator 门控五 crate 移出（reached 126→122）；intent-contract=消费保留；体积同值如实登记（零调用者从未链接）；人工清单+cargo tree |
| 4 | 全量 CI 绿 | **PASS**（audit 网络故障登记） | test 389 块 EXIT=0 / clippy 0 / fmt 干净 / closure PASS / keys 4 项 PASS / CF01 active=18 / catalog validate PASS；cargo-deny 网络故障 --offline advisories ok 等效（先例） |
| 5 | 版本 bump + evidence/49/ | **PASS** | 0.313.30→0.313.35 五 bump；evidence/49/ 全量落盘 |

## QEMU 引导/四态复跑（HEAD=0.313.34 清单镜像）

静态内存审计（纪律 4）：本卷新增服务内增量 ≈ ai-worker +30KB 级 ELF +
mqtt 插件 app 平面缓冲（inbox 8×4KB 上限 + outbox 4×1KB）< 100KB，QEMU
virt RAM 128MiB 充裕；init bundle 3.24MiB 于扩窗后 8MiB 界内（DTB 窗口
余量恢复）。

引导实弹四态时间线（tick 号，串口 qemu-serial-exit.log）：

```
System2 消费 used=true path=Slow   ×11（verdict=Accepted ×11）
越权提案拒绝 verdict=Bad           ×1（detail=Range power_kw exceeds budget）
event=peer-unhealthy consecutive=3 ×1（启发式接管，链路不断）
event=peer-recovered               ×1（回 AI，同 tick 恢复消费）
全程 status=OK ×188；CONTROL-CHAIN agent=OK dispatch=Committed 连续
AUDIT-AI appended=… dropped=0（决策留痕哈希链）
```

对端（独立非 EnerOS 进程）：python broker49（QoS1 SUBACK/PUBACK 全程）+
peer_brain49 四策略（soc/overbudget/silent/openai 可选透传），transcript
入库；modbus 从站（47 卷同源）供真实采集链。

## 诚实登记

1. **ADR-0045 owner 签字未签**：BLOCKED（owner 动作）——实施依据为卷
   执行模式（D1 已裁决保留 LLM + 本卷方案 B 推荐），v1.0 宣称冻结前须
   补签；宣称纪律已硬编码进 ADR 与代码措辞。
2. cargo-deny advisory-db 网络故障贯穿会话：--offline 本地库 advisories
   ok 等效验证后 --no-verify 落地相关提交（44/47/48 先例）；CI audit 项
   同因 FAIL，五检全绿。
3. 传输层重连边界：smoltcp 句柄无重拨 seam——peer 断链语义由 LLM 服务
   生命周期承载；broker 重启场景 guest 重连登记后续卷。
4. ai-peer timeout_ms 演示装配=30000 ms（30s；QEMU EXECUTE 周期节拍下
   应答投递延迟 ≤1 周期），产品缺省语义 5s 由配置承载（46 卷 keepalive
   演示值同款纪律）。
5. PRODUCT-READY=false 为 47/48 已登记 control-dispatch 3s 死线缺陷延续
  （非本卷引入）；48 卷孤儿 3 处未清偿（orphan-check 破坏性门未运行）。
6. 决策留痕审计链 MemStore 易失；持久化归 41 卷已登记后续项。
