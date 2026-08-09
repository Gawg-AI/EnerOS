# 蓝图/26-C 阶段 1 交付说明（WP-26.8 / 26.9 / 26.10）

> 日期：2026-08-10 ｜ 前置：26-B 已验收（门禁活着：g7-audit G7-PASS + 三门禁全绿，附于侦察）

## WP-26.8 IRQ 子系统（P0-1，S4，方案乙）

| 项 | 状态 | 证据 |
|---|---|---|
| shim 4 API + cap_irq_control | ✅ | sel4_shim.c/h + ffi.rs + safe.rs（host → HostUnsupported，红线 2） |
| IrqTable（bind/route/ack/clear，有界 16） | ✅ | root-task irq.rs；route 纯函数 + 有界拒绝 host 单测 |
| 事件循环接线（方案乙：独立 IRQ ntfn poll） | ✅ | service.rs 循环 IRQ poll + 计数 + Ack；irq_timer.rs（虚拟 timer IRQ 27 使能，避开 seL4 内核时钟 26） |
| 事件循环现状图（侦察） | ✅ | busy-poll 结构（hello call + fault poll，无主 ntfn Wait）→ 方案甲需 Wait 驱动大改 → 选乙（判据写入代码注释） |
| **QEMU 中断计数非零证据** | ⚠️ **PENDING** | QEMU 引导链阻塞：`bootctrl-resolve-fail error=Genesis`（UEFI 固件 2025.11 vs 既有成功日志 2024.02；系统盘 BOOTCTRL 双副本经 GPT dump 验证合法 Genesis）——与 IRQ 无因果，按 R26-5 登记 |
| 回归基线 | ✅ | workspace 326 套件全绿 + target aarch64 编译过（xpack 工具链） |

## WP-26.9 BSP/装配（P0-2）

| 项 | 状态 | 证据 |
|---|---|---|
| FDT 解析器 + DeviceInventory（步骤 1） | ✅ | hal/board device_tree.rs：FDT v17 完整 header 布局 + 结构块遍历 + FDT_END 终止；**真实数据测试**（dtc 编译 configs/qemu-virt.dts 嵌入）：pl011 uart@9000000（reg/interrupts 精确断言）+ armv8-timer 4 组 PPI（含虚拟 timer 1,13,11 = IRQ 27，WP-26.8 绑定依据）；坏魔数/截断负向 |
| PlatformAssembly composition root + net 装配链 + virtio-net 驱动（步骤 2-4） | ⚠️ **PENDING** | 依赖发现层+QEMU 验证（virtio-net 新写驱动需 QEMU 活体验证；五套 Transport 接线不改代码只改装配）；QEMU 引导链阻塞下无法验收 |
| M1（Modbus TCP 对通 + 104 slave 回环） | ⚠️ **PENDING** | 同上级联 |

## WP-26.10 真实块设备 + 掉电安全存储（P0-3，S5）

| 项 | 状态 | 证据 |
|---|---|---|
| VolatileMmio 真实后端（步骤 1-2） | ✅ | virtio_blk.rs：read_volatile/write_volatile + SAFETY；工厂 VirtioBlk 分派 fail-closed（Unsupported）——证伪旧"工厂直接 RamMmio"；host 测试走 create_mock_device（RamMmio 门控路径） |
| 防回归 | ✅ | 工厂无后端 → Unsupported；mock 路径读写往返；workspace 326 套件全绿 |
| QEMU virtio-blk 真实读写 + qemu-powercut ≥20 次掉电恢复 | ⚠️ **PENDING** | QEMU 引导链阻塞（同 WP-26.8） |
| DurableLog 层（S5 第二层） | ⚠️ **PENDING** | **差异登记**：26-C 所述"22-W3 既有成果（CRC/原子写/compaction/背压）"未以可复用类型落地（蓝图/22 W3 为数据管道蓝图）；DurableLog 需按 26-C §4.2 接口定义新写，待块设备层 QEMU 验证后实施（S5 分层锁死） |

## 差异与风险销项

| 项 | 说明 |
|---|---|
| 与规划差异 | ① 22-W3 DurableLog 无既有可复用实现（登记，S5 分层不变）；② QEMU 证据统一 PENDING（bootctrl 引导链阻塞，非 IRQ/存储缺陷；UEFI 固件版本差异为最大嫌疑，需在固件/引导链层面排查——登记为引导链问题，不顺手修） |
| R26-5（QEMU/真板阻塞） | 已触发：QEMU 引导链阻塞 → 代码层交付 + PENDING 管理（26-B 政策） |
| R26-6（IRQ 破坏 IPC/fault 拓扑） | 未发生：方案乙独立 ntfn + poll，fault ep/hello 路径零改动；workspace 零回归 |
| R26-7（持久化跳层） | 未发生：仅块设备层代码（S5 第一层），DurableLog/落盘未提前实施 |
| R26-11（DTB/boot 链） | 部分：FDT 解析器已交付（blob 输入，boot 链 DTB 指针待 26-C 后续确认——登记） |

## 蓝图/09 销项引用

| 09 条目 | 对应 | 状态 |
|---|---|---|
| P0-1（零中断绑定） | WP-26.8 | 代码层销项（QEMU 证据 PENDING） |
| P0-2（网卡未装配） | WP-26.9 | FDT 发现层销项（装配链 PENDING） |
| P0-3（RAM 模拟工厂） | WP-26.10 | 工厂 fail-closed 销项（掉电证据 PENDING） |
