# 30 版本 RT-009 补充修复（蓝图 30 §1：driver_probe 本版本做 RT-009）

执行日期：2026-08-11。

## 修复
1. **取消 zip 静默截断**：`bound.zip(bindings)`（按短者截断，任一驱动 bind
   失败/跳过即整体错位）→ 显式索引配对 + 越界检查（bindings 索引超出
   bound → 打印 MISMATCH 判据行并跳过，不静默错位）。
2. **badge 解码防御**：`*badge - 1`（badge=0 时 u32 回绕 u32::MAX）→
   `badge_to_binding_index(badge)` 纯函数（checked_sub，0 拒绝 None）。

## 判据
- badge_to_index_decode：1→0、2→1、17→16、0→None（回绕防护）。

## 回归
root-task 61 全绿。QEMU IRQ 计数基线（PL011 收包）在版本验收统一跑。
