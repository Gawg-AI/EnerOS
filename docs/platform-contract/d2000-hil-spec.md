# D2000 Platform Contract 与 HIL 验收规格（G10 输入，本阶段冻结）

> 文档级别：规格冻结（不实测寄存器）
> 阶段：G3（v0.233.0~v0.252.0 BSP 与关键驱动）
> 规则：D2000 相关寄存器、时序、DMA cache coherency 与 SMMU 只进入本规格，
> 不得伪造真机通过证据；所有 `to-verify` 字段在 G10 由数据手册/板级测量替换。

## 1. 范围

本规格是 G10（HW-001~HW-014）的输入，冻结：

1. D2000 platform contract（机器可读 JSON，`configs/platform-contracts/phytium-d2000.json`）。
2. BSP adapter 接口（内核 hal 侧契约）。
3. HIL 测试规格（每项验收的接线、仪器、判据与记录要求）。
4. HW-001~HW-014 Task Card（G10 执行单元）。

## 2. Platform contract（v0.1.0-draft）

`schema: eneros.platform-contract.v1`。当前字段来源状态：

| 字段 | 当前值 | 来源 | G10 动作 |
|---|---|---|---|
| cpu.model | phytium-d2000 (FTC662) | 手册 | 核对 stepping |
| gic.distributor | 0x2f000000 | 占位 | 手册/板测替换 |
| gic.redistributor | 0x2f100000 | 占位 | 手册/板测替换 |
| resources.uart0 | 0x28000000 | 占位 | 手册/板测替换 |
| resources.gpio0 | 0x28010000 | 占位 | 手册/板测替换 |
| dma.iommu | 0x2b000000 (smmu-v3) | 占位 | 手册/板测替换 |
| reset_reason | bootctrl-partition | 设计 | 与 boot manager 对齐 |

禁止：把 `to-verify` 字段当作真值进入生产代码；禁止在 QEMU 上运行 D2000
contract 后宣称 D2000 已验证。

## 3. BSP adapter 接口（冻结）

内核 hal 侧只依赖以下 adapter（`crates/hal/board` 与
`crates/kernel/microkernel/src/hal/` 统一接口）：

```text
trait PlatformAdapter {
    fn gic_bases() -> (PhysAddr, PhysAddr);        // GICD, GICR
    fn uart_base() -> PhysAddr;                    // early console
    fn timer_irq() -> u32;                         // PPI
    fn psci_conduit() -> PsciConduit;              // hvc | smc
    fn dma_cache_ops() -> DmaCacheOps;             // clean/invalidate 语义
    fn iommu() -> Option<IommuProfile>;            // SMMUv3 或 None
    fn reset_reason_storage() -> ResetReasonStore; // bootctrl 分区
}
```

QEMU 与 D2000 只允许在最前端 firmware/BSP adapter 有差异；manifest、boot
manager 核心、BootInfo、槽状态机与内核路径必须相同（MASTER §6.1）。

## 4. HIL 验收规格（G10 执行）

每项必须满足 00 契约的证据等级 L3/L4 记录要求（Git 提交、构建命令、
工具链、板卡 BOM、固件 hash、接线、原始日志、波形/电源轨迹、阈值、
失败复现步骤）。

### HW-001 启动链（BootROM→TF-A→U-Boot→EnerOS）
- 冷/热启动各 100 次；错误签名/旧版本/损坏 DTB 均拒绝并留下可诊断状态。
- 仪器：串口日志 + JTAG。

### HW-002 GIC/PSCI/SMP
- 全核连续运行、百万次 IPI/外部 IRQ、屏蔽/迁移/风暴注入，无丢 EOI、
  死锁或越界延迟；逻辑分析仪独立测量 IRQ 延迟。

### HW-003 UART/GPIO
- 多波特率双向长流、溢出/奇偶/帧错、GPIO 输入输出与中断波形测量。

### HW-004 DMA/缓存一致性
- 非一致性缓冲模式双向校验、跨核压力、边界长度和错误 IRQ；
  描述符/缓冲物理地址均非 0 且属于保留池。

### HW-005 SMMUv3
- eMMC/MAC 受限域运行，越权 DMA 阻断并报告；解绑后无残留映射。

### HW-006 eMMC
- 连续 24h 压力；随机掉电 ≥1000 次，已确认事务不丢失且结构可挂载。

### HW-007 文件系统掉电一致性
- create/rename/delete/sync 各断点随机切电 ≥1000 次。

### HW-008 TSDB 原子持久化
- 持续采集同时随机切电 ≥1000 次；无已确认数据静默丢失、无 chunk ID 回退。

### HW-009 Ethernet MAC/PHY DMA
- 交换机双向线速/小包/大包/VLAN/断链重连/错误帧/24h 压力；
  抓包仪与交换机端口统计逐包核对。

### HW-010 RS485
- 全波特率/校验组合、长线/终端/多节点/碰撞；DE 与数据波形测量；
  与商业 Modbus 仪表及串口分析仪互操作。

### HW-011 CAN
- 多节点负载、不同 bit rate、仲裁、错误帧、断线、bus-off 恢复；
  与商用 CAN 分析仪/ECU 逐帧互操作。

### HW-012 RTC/北斗 PPS 守时
- 真实北斗 PPS 连续测量、断开/恢复天线验证 holdover 与重捕获；
  与时间间隔计数器或独立授时设备比较。

### HW-013 Watchdog/Reset/Power
- 停止喂狗、任务死锁、欠压、快速掉电注入；测量
  pretimeout→flush→reset/关断时序并验证重启原因；示波器/可编程电源记录。

### HW-014 TPM 2.0
- 冷启动、自检、随机数、PCR 扩展、NV 单调计数、seal/unseal、错误
  locality；独立 TPM2 工具/验证器校验 quote 签名与事件日志。

## 5. 本阶段（G3）在 QEMU 上的对应验证

| HIL 项 | QEMU 对应（本阶段完成线 L0~L2） | 证据 |
|---|---|---|
| HW-001 | qemu-prod 媒体启动 + bootctrl 断电 | qemu-media-result / PowerCut |
| HW-002 | GIC/timer/PSCI adapter、4 核启动、IRQ 绑定投递 | eneros.g2/g3.v1 |
| HW-003 | PL011 console service（DRV-004） | svc-drv-console 证据 |
| HW-004 | DMA pool 连续分配 + cache 原语（DRV-003） | dma-alloc/cache 证据 |
| HW-005 | SMMUv3 虚拟 profile（DRV-009 规格） | 本规格 §6 |
| HW-006 | virtio-blk 真实 I/O + 掉电矩阵（DRV-007/STO） | qemu-g3 断电矩阵 |
| HW-007 | CONFIG 事务断电旧/新（STO-002） | 断电矩阵 |
| HW-008 | TSDB 块后端（STO-004） | 断电矩阵 |
| HW-009 | virtio-net 双向（DRV-008） | svc-drv-net 证据 |
| HW-010~011 | 虚拟 RS485/CAN 线级对端（DRV-011） | 外部对端互操作 |
| HW-012 | RTC 读取（DRV-010） | svc-drv 证据 |
| HW-013 | 虚拟 watchdog/reset（DRV-010） | reset reason 重现 |
| HW-014 | TPM 无虚拟等价物 | G10 真机 |

## 6. SMMUv3 虚拟 profile（DRV-009）

QEMU virt 平台默认无 SMMU；`-machine virt` 不支持附加 SMMUv3。
sbsa-ref 支持 smmu。本阶段冻结：

1. **软件域语义**：`DmaState` 域记账 + `SYS_DMA_GRANT` 授权 + 进程退出
   撤销（已实现，`prod/dma.rs`）；无 SMMU 平台由平台契约保证地址能力。
2. **QEMU SMMU 验证（可选路径）**：若后续启用 sbsa-ref + `-device
   smmu`，`configs/platform-contracts/qemu-sbsa-ref.json` 记录 stream-id
   映射，非法 DMA 阻断证据输出 `eneros.g2.v1 test=smmu-fault-rejected`。
3. **D2000**：contract 已声明 smmu-v3 占位；G10 按 HW-005 实测。

禁止把"软件记账"称为"IOMMU 隔离"（MASTER §8.4 禁止捷径）。

## 7. 已知限制与风险

- D2000 全部寄存器值仍为占位（来源 to-verify），G10 前任何 release
  不得声明 D2000 支持。
- QEMU virt 与 D2000 的 cache coherency 模型不同：QEMU TCG 无真实 DMA
  缓存竞争；cache maintenance 原语在真机必须重新验证（HW-004）。
- 无 TPM 虚拟等价物：TPM 相关验收完全依赖 G10 真机。
