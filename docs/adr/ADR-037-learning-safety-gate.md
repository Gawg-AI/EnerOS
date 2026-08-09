# ADR-037：W5.3 学习输出安全闸——SafetyCatalog 权威边界 + 显式拒绝

- 状态：已接受（2026-08-09）
- 关联：蓝图 22 §2.W5.3（接 W2.5）；红线4（禁止静默丢弃/钳位）
- 验收映射：学习输出越界被拒 + 用旧值 ✓；不致灾测试 ✓

## 背景

W2.5 学习器（DeadbandAdaptive / ForecastCorrector）已有内联值域检查，
但存在**结构性缺陷**：提议值先 `clamp` 再校验 → 永不越界 →
`rejected_updates` 永不触发——"越界拒绝"形同虚设（红线4 违反）。

## 裁决

1. **`SafetyCatalog` 落在 safety-validator crate**（`catalog.rs`）：
   学习/控制参数硬界目录（死区界、修正系数上限、功率钳位界），
   是学习输出边界的**唯一权威**；学习器不得自行放宽。
2. **移除学习器内部静默 clamp**：提议值（3σ / EMA）不钳位直接过
   catalog 校验——越界 = 拒绝（用旧值）+ 计数（红线4：显式拒绝
   而非静默吸收）。输出侧 `correction()` 保留钳位（输出防御层，
   学习路径已保证 EMA 界内或已回退）。
3. **`with_catalog` 构造器**：DeadbandAdaptive/ForecastCorrector
   接真 catalog（`new(initial, min, max)` 保留兼容旧调用方/测试）。
4. **真实调用方接入**：energy-market-agent 3 处学习器构造全部切
   `with_catalog(SafetyCatalog::default_eps())`——"所有在线学习输出
   经 safety-validator + safety-catalog 校验"字面成立。

## 理由

- clamp 是"静默修正"，越界事实被吞掉——审计/告警无法感知"学坏"
  信号；显式拒绝 + 计数使拒绝可观测（W5.1 可入审计链）。
- catalog 放 safety-validator（安全侧）而非 dual-brain（学习侧）：
  边界定义与校验实现同 crate，无循环依赖，生产可经 W4.2 管理通道
  覆写（仍须满足目录语义）。

## 影响

- safety-validator：+`SafetyCatalog`（default_eps 与 ScheduleConfig
  默认对齐：死区 [0.1,10]、修正 ±0.5、钳位 ±100kW）；
- dual-brain learning：语义从"钳位收敛"变"越界拒绝用旧值"（既有
  测试同步更新为渐进收敛语义）；+2 W5.3 验收测试；
- energy-market-agent：学习器构造切 catalog 路径（207 测试全过）。
