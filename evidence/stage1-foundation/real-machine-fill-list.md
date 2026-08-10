# 真机填空封闭清单（33 WP-33.7 定稿）

> 判定：**PENDING 集合 == 本清单 ⇔ 窗户纸状态**（33 §9）。任何 QEMU 可验证项
> 挂 PENDING = 验收不过（S9）。
> 模板：每项 = 板级动作 / 接线点 / 判据行格式 / 证据落盘位置。

## A. 27 §16 原清单（真机独占项）

| # | 填空项 | 板级动作 / 接线点 | 判据行格式 / 证据落盘 |
|---|---|---|---|
| A1 | 平台 dts 节点补全 | configs/{phytium-d2000,kunpeng-920}.dts 增加 SP805/网口/eMMC 节点（参照 qemu-virt.dts） | `probe bound compatible=... driver=...`；evidence/real-machine/ |
| A2 | PHY 芯片寄存器序列 | MDIO 框架 + `PhyAdapter{vendor_id, 初始化序列, 链路状态寄存器}` | `phy link-up`；evidence/real-machine/phy-*.log |
| A3 | eMMC/NVMe MMIO 后端 | VolatileMmio + 寄存器命令序列（参照 virtio_blk 后端接口） | 读写往返；evidence/real-machine/storage-*.log |
| A4 | 真实 MAC 装配 | MmioMacRegs 基址来自 dts（net/lib.rs HAL-004 线索） | `mac up`；evidence/real-machine/mac-*.log |
| A5 | modem 型号/AT 差异 | cellular crate 适配点（D10，31 机制已 host 验证） | `AT+CGDCONT` 应答；evidence/real-machine/cellular-*.log |
| A6 | 真对端设备接入 | 104 主站/61850 IED/Modbus 表计/MQTT broker | 互操作矩阵报告（14 号文 D2）；evidence/real-machine/interop/ |

## B. 33 §9 新登记（含 31 R31-2 转来项）

| # | 填空项 | 板级动作 / 接线点 | 判据行格式 / 证据落盘 |
|---|---|---|---|
| B1 | D2 virtio-net 完整驱动（R31-2 转来） | virtqueue 参照 virtio_blk（device_id=1，QEMU 槽 1 探测已见）；真机网卡后接 | `probe bound virtio-net0` + TCP 对通；evidence/real-machine/net/ |
| B2 | D10 cellular 真模组联调 | PPP TX/PAP 机制已 host 验证（31）；真模组接线 + APN 下发 | `ATD` 拨号成功 + PAP 认证；evidence/real-machine/cellular/ |
| B3 | D9 RS485 DE/RE GPIO 真接线 | divisor u64 修复已 host 验证；GPIO 引脚接线（`set_de_re` 注入） | `rs485 tx/rx` 判据；evidence/real-machine/rs485/ |
| B4 | 电源检测板级 `impl PowerRail` | DR-06 拆除后真实现（基址来自 FDT，走绑定表；禁硬编码 0x0906/0x0907） | `power ok` 判据行；evidence/real-machine/power/ |
| B5 | SP805/PL031 真机节点导出后绑定表条目激活 | 绑定表条目已在位（31）；真机 dts 节点导出后即激活 | `probe bound sp805/pl031`；evidence/real-machine/wdt-rtc/ |
| B6 | 时钟人工推进三处的真机时钟源接线（27 §16 原列项） | driver_probe 三常量 VIRTIO_REGION_BASE / PL011_CLOCK_HZ / SJA1000_CLOCK_HZ 的板级值 | `time_init` 判据；evidence/real-machine/clock/ |

## C. 33 WP-33.5 转来的 QEMU 侧缺口（真机/内核调试项，非本清单收口）

> 这些项**不**满足"QEMU 可验证项不得挂 PENDING"——登记为显式缺口，
> 修复路径 = 内核集成调试（34 或独立内核会话）：

| # | 缺口 | 现状（证据引用） | 修复路径 |
|---|---|---|---|
| C1 | IRQ→badge ntfn→poll 投递链 QEMU 不达 | 绑定链四根因已修（wp33-5-evidence.md）；timer 30/pl011 33 三组注入 0 计数；内核自身 IRQ 27 正常 | 内核 GIC/IRQ 状态机调试 |
| C2 | virtio-blk 块读路径 QEMU 返回 Io | durable format Io（wp33-5-evidence.md）；D4 仅 config space 活体 | 驱动 read_block/wait_used 调试（DR-09 used_elem.id 候选） |

## 判定记录

- 本清单定稿后，PENDING 台账 = A6 项（A1-A6 真机独占）+ B6 项（真机接线）
  + C2 项显式缺口。A/B 项为真机填空封闭集合；C 项为 QEMU 侧显式登记
  （不得宣称 33 DoD 第 2 项全销——中断计数与掉电判据行按"如实声明
  仍 PENDING 及原因"处理，33 §7 允许）。
