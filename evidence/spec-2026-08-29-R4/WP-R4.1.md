# WP-R4.1 留证 · A3 受信反馈 wire 单源化（生产字节不变）

日期：2026-08-29 · 基线 0.314.20 @ b5d939b（WIP 树）· 执行：会话 R4 卷

## 改动面

| 文件 | 改动 |
|---|---|
| control-dispatch/src/feedback.rs | 新增 `PROD_FEEDBACK_MAPPING` 单源常量（值 5/4/1/6/6，别名现状文档化）+ 冻结测试 `prod_feedback_mapping_frozen` |
| control-dispatch/src/lib.rs | re-export `PROD_FEEDBACK_MAPPING` |
| control-services/src/telemetry.rs | `encode_trusted_with/decode_trusted_with` 映射驱动（槽序=字段声明序，一处物化）；原 `encode_trusted/decode_trusted` 变薄包装；decode 的 ot 特判覆盖并入统一回填循环 |
| control-services/src/dispatch_assembly.rs | 装配映射改消费单源（删除手抄 6/6 字面量） |
| control-services/src/bin/svc-telemetry.rs | 覆盖注释订正（过温未接入如实登记） |

## 判据核对（spec R4 §1 WP-R4.1）

- **C1 host 测试**：`cargo test -p eneros-control-services --lib telemetry::` →
  **13 passed / 0 failed**（含新增 `feedback_slots_roundtrip_independent`——
  相异键映射 {oc:8, ot:9} 五槽独立往返，体检称"今天没有任何测试能表达它"
  的场景已可表达并转正；含 `prod_mapping_aliasing_frozen_and_visible`——
  生产 6/6 别名在 wire 上显式可见：oc 槽字节 == ot 槽字节）。
- **C2**：`cargo test -p eneros-control-dispatch --lib feedback` →
  **3 passed / 0 failed**（含映射值冻结测试）。
- **C3 生产字节不变**：既有 `trusted_wire_roundtrip`（断言未改）通过；
  decode 别名行为保持（同键后写胜出 = 历史行为）。
- **C4 硬编码键清除**：`grep -n 'get(&6)' telemetry.rs` → 生产代码零命中
  （仅剩测试断言按点位名直查，属测试语义）；encode/decode 反馈段全部
  经 `m.grid_up/m.soc/m.dc_bus_v/m.overcurrent/m.overtemp` 进出。
- **C5 clippy/fmt**：control-dispatch/control-services clippy 干净（唯一
  warning `ai_worker.rs:605 std::panic` 为 WIP 预存，非本 WP 触碰文件，
  按 R3 不顺手修）；fmt 已过。

## 体检声称比对（报告 A3 卡）

| 体检证据 | 修复后状态 |
|---|---|
| telemetry.rs:371-372 双 `get(&6)` | ✓ 消除——槽位由映射字段驱动 |
| telemetry.rs:417-424 ot 无条件覆盖 oc | ✓ 消除——统一回填循环（键相异时独立保留，有测试锁定） |
| dispatch_assembly.rs:599-600 手抄 6/6 | ✓ 消费单源常量 |
| 测试映射 5/6 分开致缺陷不可测 | ✓ 新增独立往返测试使该场景可表达 |
| 数据面矫正（设备过温点） | ⏸ 拆出 WP-R4.4（HZ-THERM-001 拒充语义 → QEMU 门槛，见 spec §3） |

## 行为变更声明

生产 wire 字节：**零变更**（144B 布局与槽值逐字节一致）。FeedbackBundle
语义：**零变更**（6/6 别名保留并显式冻结）。唯一变化 = 代码结构与可测性。
