# 30 版本验收报告（蓝图/30 §10 DoD）

执行日期：2026-08-11。版本：30（运行时/微内核深化）。
8 个提交：aee7f05（WP-30.1）/ 957f180（WP-30.2）/ 7616df0（WP-30.3）/
a0020c6（WP-30.4）/ aca97ad（WP-30.5）/ bc7142a（WP-30.6）/ 67cb2be（WP-30.7）/
ab2d221（RT-009 补充）。证据全量落盘 `evidence/commercial-audit-0811/gates/`。

## DoD 逐项

| # | 判据 | 状态 | 证据 |
|---|---|---|---|
| 1 | RT-001~031 全部关闭或登记豁免 | ✅ | 31/31 处理（见下表）；豁免仅 P2 且理由入 wp30-* 证据 |
| 2 | 零硬件依赖 grep 判据全过 | ✅ | 0x08000000/0x080A0000/0x09000000/0x09010000 于 hal/runtime 生产路径归零（仅 PlatformConfig 表/测试保留） |
| 3 | QEMU 回归：13 行断言、IRQ 计数、UART 收包、bundle 启动 | ✅ | 见下（QEMU 活体） |
| 4 | WP-27.5/27.7 按 27 原规格验收 | ✅ | durable-log crate（S5 第三层）+ elf-loader 重定位（黄金向量） |
| 5 | cargo test 触及 crate 全绿 + 侦察报告齐备 | ✅ | workspace 全量（见下）+ recon-wp30-1.md 等 |

## RT 覆盖核查（31/31）

| WP | RT 项 | 说明 |
|---|---|---|
| 30.1 | 002/003/010/021/022 | 启动契约链单一事实源（常量统一 + validate 收紧 + 窗口上限） |
| 30.2 | 004/005/006/007 | bundle 验签式解析 / entry checked_add / 段重叠（validate_segments 统一）/ 窗口溢出 |
| 30.3 | 001/028/029/030 | WDT 映射守卫 / try_lock + 创世诚实化 / 真实服务名 / 文档对齐 |
| 30.4 | 008/011/016/024/025/027 | timer IRQ 从 DTB / GIC provider 化 / SAFETY 注释 / GICR 有界 / UART/NET 注入 / OnceCell |
| 30.5 | 012~015/017~020/023/026/031 | ABI/sel4-sys 卫生批 11 项 |
| 30.6 | —（WP-27.7） | elf-loader 重定位 |
| 30.7 | —（WP-27.5 + RT-028①） | DurableLog 机制 + 创世根真实 SM3 |
| 补充 | 009 | IRQ 接线取消 zip + badge 解码防御 |

## 豁免登记（P2，理由入证据）

- RT-002/021 完整通道（rust_entry 消费 init_bundle_phys 字段）：需 elfloader
  传参 ABI（第三方构建产物源码不在仓库）→ 34 域。
- RT-028① 持久化接线（DurableLog 挂 BlkAdapter）：随 33 域 WP-27.4 掉电
  证据出证（机制已交付）。
- RT-011 GIC init_with 装配调用点：31 域 PlatformCtx 聚合时接线。
- RT-024 真机 GICR 超时路径：QEMU 恒就绪，有界轮询已落地（行为层关闭）。
- RT-017/018 QEMU 行为回归：版本验收 QEMU 跑。

## QEMU 活体（DoD 3）— 已核验

- 重建 qemu-prod 镜像（SOURCE_DATE_EPOCH=1785465257，commit=ab2d221，
  BOOTCTRL seq=0）+ 540s 延长运行（QEMU_EXIT=124 = seL4 无 ACPI 关机预期）。
- **序列门 13/13 PASS**（含漂移形态：`[svc-fault] FAULT triggered` +
  `SUPERVISOR: svc-fault fault → restart`）。
- RT-029 判据：SUPERVISOR 日志服务名真实（svc-fault）——`fault → restart in
  1000ms (fault_count=1)` → `restarted (total=1)` → `svc-hello still
  responsive (tick=85)` 两轮完整隔离周期。
- RT-008 判据：`timer IRQ from fallback` 行数 0（DTB arm,armv8-timer 提取
  成功 → 27）；BOOT-007 probe 行在案。
- 与 29 基线逐项一致（probe-report bound=2/unbound=23/failed=31、IRQ 27/33
  内核占用事实相同）——零回归。
- 证据：gates/gkernel-qemu-boot-30.log（97214 B）。

## 治理自查

- gate 四子命令（29 修真后）：config-refs/config-syntax/mock-leak/orphan-check
  ——版本验收重跑全绿（29 提交后未回退）。
- mock 门控：SE-05 后无新 Mock 面（30 无新增 mock 生产面）。
- 豁免登记：RT-002/021（elfloader ABI，34 域）、RT-028① 接线（33 域出证）、
  RT-011 init_with 装配点（31 域 PlatformCtx）。
