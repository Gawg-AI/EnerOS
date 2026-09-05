# 蓝图 68 卷 68.1 WP-68.1.4 停顿级联探查记录（2026-09-05，重录版）

> 相位：BP67 G8 run#1（全服务冷启首窗、无人工干预）×3 窗，720s 预算/窗。
> 镜像：**68.1 卷终代码全链重建**（gate_image_raw 全链；含 ChunkedReceiver
> generation + service_slots 四词形观测 + 默认栈 32 KiB 修复——见执行卡 §F）。
> 工具：tools/stall-probe-681.sh；原始日志 stall-probe-all.log。
> 首三轮（stale 镜像）作废说明见执行卡 §F/§WP-68.1.4；旧数据存
> stall-probe-firstround-stale.md。

## 窗记录（修复后镜像，有效）

| 判据 | 65 基线（600s 窗） | run1 | run2 | run3 |
|---|---|---|---|---|
| PRODUCT-READY: true | ✓ | ✓ | ✓ | ✓ |
| HEARTBEAT-KILL / SUPERVISOR | 0 | **0** | **0** | **0** |
| ACQ-ROUND | 35 | 35 | 33 | 33 |
| OBS-REPORT flush | 21 | 21 | 21 | 21 |
| CHUNKED-SLOW（新观测；能量链 boot 窗热点，REG-BOOT-SLOW-FINAL） | — | 14 | 14 | 13 |
| CHUNKED-PAGE-EVICTED（新观测；同上热点 slot=0） | — | 1 | 1 | 1 |
| CHUNKED-SLOT-FULL / DUP-SESSION | — | 0/0 | 0/0 | 0/0 |
| ERR:BadIndex（迟到帧观测） | — | 0 | 0 | 0 |

（注：本表为**真·68.1 二进制**终录；中间一轮"全 0 慢调用"数据来自
漏编译 service_slots 的二进制，已废弃不留档。）

## 结论（照卷册 WP-68.1.4 步骤 2：不可复现支路）

**修复后镜像三窗全 CLEAN：三次未复现**。svc-telemetry 心跳/ACQ/INGEST
全存活（ACQ 节奏与基线一致、flush 周期逐字节同、rejected 同量级稳态
背压），零停顿级联，零 CHUNKED-SLOW（无任何 chunked 调用超 100ms），
四新词形零触发。

处置：按卡面登记 **"三次未复现"，以 WP-68.1.2 修复 + 观测词形作为
防御性关闭（观察项降级为"观测在位"）**；不断言根因（禁止无证据断言）。
若复现，SLOW/SLOT-FULL/DUP-SESSION/PAGE-EVICTED 任一词形将直接指认
具体交错面。

## 附：gate 实弹捕获的次生缺陷（本探查的首要副产物）

首两轮 gate 全链（pre-修复二进制）捕获**全系统静默冻结**（逐字节一致
冻结窗 serial 归档 frozen-window.qemu-serial.log）→ 定位为默认栈 16 KiB
贴线 + 68.1 服务循环帧增长 → 修复 = STACK_SIZE 0x4000→0x8000。完整
因果链见执行卡 §F。本探查的探针（CHUNKED-SLOW 等）为定位提供了
"仅冻结窗出现慢调用"的关键佐证。

## 执行注记

- 首版探查脚本 ACQ-ROUND 阈值误设 ≥100（对 65/67 基线 ×35 未先校准）
  → 已校准 ≥20 并留档。
- BP67 执行卡 G8 观察项 ×54 计数未入档一事见 candidates-registry
  REG-G8-X54（引用须注明未入档）。
