# SPEC-SERIES-2026-08-29/R4 · Wave 1 执行摘要

日期：2026-08-29 · 代码基线 0.314.20 → 收口 **0.314.21**（@ b5d939b + WIP 树）

## 卷级结论

Wave 1 三 WP（R4.1=A3 wire 单源 / R4.2=C5 死副本删除 / R4.3=C2 handover
收编）**全部完成**，判据全过，全量测试无回归。

## 全量验证

```
cargo test --workspace --no-fail-fast
→ 394 个测试目标，PASSED=7815，FAILED=0，CARGO_EXIT=0
→ 完整日志 /tmp/r4-full-test-full.log（grep FAILED|error = 0 行）
```

基线对照：52 终验实测 7586（evidence/52/exit-gate.md：389 块/7586 passed/0
failed）→ SPEC-E 续作 7779 → SPEC-C 复验 7780 → 7815（只升不降，含本卷
新增 4 测试：feedback_slots_roundtrip_independent /
prod_mapping_aliasing_frozen_and_visible / prod_feedback_mapping_frozen /
handover-abi 冻结面扩展）。

clippy：触碰 7 包除 WIP 预存 ai_worker.rs:605（std::panic，非本卷触碰）
外干净；fmt 全过。

## 判据索引

- WP-R4.1：[WP-R4.1.md](WP-R4.1.md)（C1–C5 全过；生产字节零变更）
- WP-R4.2：[WP-R4.2.md](WP-R4.2.md)（C1–C3 全过；零消费者纯删除）
- WP-R4.3：[WP-R4.3.md](WP-R4.3.md)（C1–C5 全过；值逐字节不变）

## 遗留与登记

- **WP-R4.4（A3 数据面矫正）**：设备过温点接入 + PROD_FEEDBACK_MAPPING
  翻转 + QEMU 全链复验（HZ-THERM-001 拒充面）——owner 排期。
- **QEMU 例行**：本卷无 ABI 值变更，无强制 image raw 门槛；下次 QEMU 窗口
  按既有 SOP 重链接镜像即可。
- Cargo.lock 版本同步随 0.314.21 bump 完成核验。
- 未提交（工作树含 SPEC 系列 WIP，提交分桶纪律按 F 卷先例由 owner 把关）。
