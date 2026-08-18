# ADR-0040：tsn TsnDriver / NicApplier 标 trait-only scope，删 SoftSyncClock

> 状态：Accepted（2026-08-09）
> 决策者：架构深化实施（蓝图 23 W3.4）
> 前置：ADR-0026（两实现者才算真）；蓝图 23 §1 锁定 2。
> 审计证据（实施时 grep 复核）：
> - `trait TsnDriver`（driver_glue.rs:40）唯一实现者 `MockTsnDriver`（测试用）。
> - `trait NicApplier`（tas.rs:211）唯一实现者 `MockNicApplier`（测试用）。
> - `SoftSyncClock`（degraded.rs）零非测试调用方，方法体为对 `GptpClock` 的纯委托。
> - tsn 数据面已正式降级（v0.223.0 裁决）：`degradation::dataplane_available() -> false` 硬编码，
>   无 TSN 硬实时承诺（QEMU virt 与首款交付板无硬件时间戳/taprio）。

## 1. 背景

蓝图 23 §0.2 结论 2 指出 `TsnDriver` / `NicApplier` 仅 Mock 实现——按"两实现者才算真"
纪律（锁定 2），这两个 trait 是 hypothetical seam 候选。但 tsn 数据面已正式降级，
真实实现（netlink/taprio/硬件驱动）在具备能力的网卡接入前不存在——此时"删 trait"反而
损失测试面（Mock 驱动是算法验证的测试双），"留 trait"则需要诚实标注 scope。

## 2. 决策

1. **`TsnDriver` / `NicApplier` 标 trait-only scope**（doc 注释显式标注 + 指向本 ADR）：
   当前唯一实现者是 Mock，真实实现待硬件对接后按 ADR-0026 纪律"先有真实需求再加"。
   不删除——它们是算法层（TAS 门控列表、流调度）的测试双，删除会损失测试覆盖。
2. **删除 `SoftSyncClock`**：纯委托 wrapper（`handle_sync`/`handle_follow_up`/`offset_ns`
   全部转发 `GptpClock`），零非测试调用方。能力一致性检查 `verify_claim` 收敛到
   `SoftSyncClaim::verify_claim()`（逻辑不丢），新增 `soft_sync_clock()` 辅助构造
   `(GptpClock, SoftSyncClaim)` 供调用方直接使用。
3. **测试不减少**：`test_soft_sync_clock_claim_consistent` / `test_soft_sync_processes_sync` /
   `test_non_degraded_claim_inconsistent` 改写为用 `SoftSyncClaim::verify_claim` + `GptpClock`。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 删除 TsnDriver/NicApplier trait | 它们是 TAS/流调度算法的测试双（MockTsnDriver/MockNicApplier 有 91 个测试支撑）；数据面降级不等于算法无用，删除损失测试面 |
| 保留 SoftSyncClock | 零调用方的纯委托 wrapper；能力检查逻辑已收敛到 SoftSyncClaim，保留即死代码（W1 解除静音后编译器已可证明） |

## 4. 影响面

- `crates/protocols/tsn-time/src/driver_glue.rs`：TsnDriver doc 标注 trait-only scope。
- `crates/protocols/tsn-time/src/tas.rs`：NicApplier doc 标注 trait-only scope。
- `crates/protocols/tsn-time/src/degraded.rs`：SoftSyncClock 删除 → `SoftSyncClaim::verify_claim`
  + `soft_sync_clock()`；测试改写。
- `crates/protocols/tsn-time/src/lib.rs`：文档更新。
- 无跨 crate 影响（唯一消费者 vpp-scenario 不引用 SoftSyncClock）。

## 5. 验收

- `SoftSyncClock` grep 零命中（生产代码；仅文档注释记录删除）。
- `cargo test -p eneros-tsn-time` → 91 passed; 0 failed。
- `cargo build -p eneros-vpp-scenario` → 通过。

## 6. 追踪

- 蓝图：23 §4 W3.4；§1 锁定 2。
- 纪律：ADR-0026（两实现者才算真）、ADR-0028（删即删）。
- 遗留：lib.rs 内嵌测试搬出 `tests/`（规格"可选但强烈建议"，因行为等价变换风险低收益高，
  留待 G-I 总出口后按需执行）。
