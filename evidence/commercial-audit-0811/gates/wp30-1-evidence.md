# WP-30.1 启动契约链单一事实源（RT-002/003/010/021/022）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §3（5 项一次 PR 全关）。

## RT-022（P2，常量三重不一致）
- boot-protocol 新增导出 `ENEROS_INIT_BUNDLE_MAX_LEN = ENEROS_MAPPED_WINDOW_LEN`
  （2 MiB，单一事实源）；删除 `MAX_INIT_BUNDLE_LEN=256MiB`（与 root-task
  64MiB/内核 2MiB 三重不一致）；validate 引用新常量。

## RT-010（P1，INIT_LOAD 浮动区间）
- boot-manager lib.rs：INIT_LOAD_START/END 改引用 boot-protocol 常量；
  validate 收紧 `load_address != INIT_LOAD_START → LoadRange`（不再允许
  8 MiB 区间内浮动——浮动即与内核固定映射/root-task 固定读静默断裂）。
- 判据：floating_init_load_address_rejected（浮动 +0x1000 → Err；窗口末端
  越界 → Err）。

## RT-003（P1，64MiB 上限越窗 VM fault）
- root-task：MAX_BUNDLE_LEN（64MiB）删除；窗口校验提取为 host 可测纯函数
  `bundle_len_within_window`（[64, 2MiB]），read_bundle_from_phys 引用。
- 判据：bundle_len_within_window_rejects_oversized（3MiB → false，判据；
  旧 64MiB 值 → false）。

## RT-002/RT-021（P1/P2，init_bundle_phys 写入无消费）
- root-task INIT_BUNDLE_PHYS 改引用 `ENEROS_INIT_BUNDLE_PHYS`（消除魔法数）；
  boot-manager validate 收紧后写端保证 load_address == 常量 → 两端由同一
  常量闭合。
- **豁免登记**：完整"消费 BootInfo.init_bundle_phys 字段"需 elfloader 传参
  通道（x1）——elfloader 为第三方构建产物（源码不在仓库，
  third_party/sel4-build/top/elfloader 仅产物），跨第三方 ABI 改造登记
  34 域；按蓝图 RT-002 修复方案过渡路径（一致性断言 + 常量闭合）执行。

## R30-1 风险登记
bundle >2MiB 需求需先扩内核窗口（BOOT-007 证据重出）。

## 回归
- boot-protocol 30 / boot-manager-uefi 5（+1 新判据）/ root-task 55（+2 新
  判据）全绿；QEMU 判据（BOOT-007 probe 行 + 13 行序列）在版本验收统一跑。
