# HW-001~HW-014 Task Card（G10 执行单元，本阶段冻结规格）

> 状态：SPEC（规格冻结，不执行）
> 归属：G10 实体硬件验收；本阶段只写规格，不实测寄存器
> 前置：D2000 数据手册/原理图/勘误、目标板 BOM、HIL 夹具与仪器

## HW-001 D2000 启动链验收

- 目标：BootROM→TF-A→U-Boot→EnerOS 唯一签名镜像可重复启动。
- 任务：冻结固件版本与 hash；实现 handoff conformance checker；冷/热
  启动各 100 次；错误签名/旧版本/损坏 DTB 拒绝并留诊断状态。
- 验收：L3 原始日志 + L4 独立解析工具验证格式与签名。
- 阻塞项：无硬件。

## HW-002 GIC/PSCI/SMP 验收

- 目标：可抢占中断、核间中断、二级核启动和多核一致性基线。
- 任务：GIC distributor/redistributor/CPU interface、SGI/PPI/SPI、
  PSCI CPU_ON；百万次 IPI/IRQ 压测；风暴注入与降核策略。
- 验收：L3 全核连续运行 + 逻辑分析仪 IRQ 延迟；L4 仪器独立测量。
- 阻塞项：无硬件；QEMU 对应验证已由 G3 DRV-005 覆盖（4 核 + IRQ 绑定）。

## HW-003 UART/GPIO 生产驱动验收

- 目标：早期控制台、运行时串口与 GPIO 真实驱动。
- 任务：波特率/校验组合双向长流、FIFO 溢出/错误恢复、GPIO 方向/上下拉/
  中断/去抖；flush 等待硬件完成。
- 验收：L3 多波特率波形测量；L4 独立 USB-UART 与逻辑分析仪。
- 阻塞项：无硬件；QEMU 对应 PL011 驱动服务（DRV-004）本阶段完成。

## HW-004 DMA 与缓存一致性验收

- 目标：连续物理页、虚实转换、clean/invalidate、ownership、SG。
- 任务：非一致性缓冲双向校验、跨核压力、边界长度、错误 IRQ；
  物理地址属于保留池且非 0。
- 验收：L3 双向校验 + 总线/设备侧数据校验；L4 外部校验。
- 阻塞项：无硬件；QEMU 对应 DMA pool + cache 原语（DRV-003）本阶段完成。

## HW-005 SMMUv3 验收

- 目标：最小权限映射、故障隔离、可诊断 SMMUv3 生产路径。
- 任务：command/event queue、stream table、context descriptor、TLB
  invalidation、fault 处理、bypass→受控域切换。
- 验收：L3 eMMC/MAC 受限域运行、越权 DMA 阻断并报告；L4 外部复核。
- 阻塞项：无硬件；QEMU 虚拟 profile（DRV-009）本阶段冻结规格。

## HW-006 eMMC 生产驱动验收

- 目标：真实控制器/DMA/介质语义。
- 任务：bus width/timing/CMD/DMA ring、错误恢复、健康统计。
- 验收：L3 24h 压力 + 随机掉电 1000 次；L4 离线工具一致性校验。
- 阻塞项：无硬件；QEMU 对应 virtio-blk（DRV-007）+ 掉电矩阵本阶段完成。

## HW-007 文件系统掉电一致性验收

- 目标：create/rename/delete/sync 断电后旧或新。
- 任务：真实介质上逐断点随机切电 ≥1000 次。
- 验收：L3 无已确认事务丢失、结构可挂载；L4 离线工具校验。
- 阻塞项：无硬件；QEMU 对应 CONFIG 事务存储（STO-002）断电矩阵本阶段完成。

## HW-008 TSDB 原子持久化验收

- 目标：chunk/索引/序号任意掉电可恢复、可检测损坏。
- 任务：WAL 或 temp→sync→rename、CRC 实测、双副本索引、恢复扫描。
- 验收：L3 随机切电 1000 次无静默丢失、无 chunk ID 回退；L4 独立解析器。
- 阻塞项：无硬件；QEMU 对应 TSDB 块后端（STO-004）本阶段完成。

## HW-009 Ethernet MAC/PHY DMA 验收

- 目标：真实 MAC/MDIO/PHY/DMA ring 收发。
- 任务：descriptor/tail/buffer 物址/MTU/中断、自协商、link 状态。
- 验收：L3 交换机双向压力 24h + 断链重连；L4 抓包仪逐包核对。
- 阻塞项：无硬件；QEMU 对应 virtio-net（DRV-008）本阶段完成。

## HW-010 RS485 生产驱动验收

- 目标：半双工 RS485 真实收发与 DE 时序。
- 任务：DE/RE 方向控制、前导/尾随时序、帧间隔时间戳、碰撞/错误处理。
- 验收：L3 全波特率/校验组合 + 波形测量；L4 商业 Modbus 仪表互操作。
- 阻塞项：无硬件；QEMU 对应虚拟 RS485 线级对端（DRV-011）本阶段完成。

## HW-011 CAN 生产驱动验收

- 目标：真实 CAN 控制器收发/过滤/时间戳/故障恢复。
- 任务：bit timing 计算与验证、mailbox/FIFO/filter、bus-off 恢复。
- 验收：L3 多节点/仲裁/错误帧/断线；L4 商用 CAN 分析仪逐帧互操作。
- 阻塞项：无硬件；QEMU 对应虚拟 CAN 线级对端（DRV-011）本阶段完成。

## HW-012 RTC/北斗 PPS 守时验收

- 目标：UTC/单调/PPS 驯服与失锁守时。
- 任务：RTC 读写/校准、PPS 硬件时间戳、PI/PLL 实际调频、holdover。
- 验收：L3 真实 PPS 连续测量 + 断/接天线；L4 独立授时设备比较。
- 阻塞项：无硬件；QEMU 对应 RTC 读取（DRV-010）本阶段完成。

## HW-013 Watchdog/Reset/Power 验收

- 目标：失效复位、复位原因、棕断保护、安全关机。
- 任务：watchdog tick 计算、window/pretimeout、复位原因持久化、
  棕断按预算 flush→标记→硬关断。
- 验收：L3 停止喂狗/死锁/欠压/掉电注入 + 示波器时序；L4 独立记录。
- 阻塞项：无硬件；QEMU 对应虚拟 watchdog/reset（DRV-010）本阶段完成。

## HW-014 TPM 2.0 验收

- 目标：板载真实 TPM 信任根。
- 任务：SPI/I2C transport、TIS、命令编码/解析、PCR/Quote/Seal。
- 验收：L3 冷启动/自检/随机数/PCR/NV/seal 全套；L4 独立 TPM2 工具校验。
- 阻塞项：无硬件、无 TPM 虚拟等价物；完全依赖 G10 真机。
