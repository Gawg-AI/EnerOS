# G10 硬件资格规格 — HW-001~018 接口契约（冻结）

> **Gate**: G10 — 实体硬件资格、量产与现场试点
> **蓝图引用**: MASTER §G10 / §21.18 / §20.5~20.6（进入条件、必须交付、Exit criteria、禁止捷径）
> **文档级别**: 规格冻结（SPEC）— 本阶段不执行任何真机寄存器测量
> **冻结日期**: 2026-08-04
> **前置文档**:
> - [d2000-hil-spec.md](./d2000-hil-spec.md) — D2000 platform contract + HIL 验收规格（HW-001~014 细节）
> - [hw-task-cards.md](./hw-task-cards.md) — HW-001~014 Task Card（G10 执行单元）
> - `configs/platform-contracts/phytium-d2000.json` — 机器可读平台契约（`schema: eneros.platform-contract.v1`）

---

## 0. 文档定位与权威性

### 0.1 本规格的作用

本规格冻结 G10 的 **18 项工作包接口契约**，使 G9 候选可以在无实体硬件条件下完成全部软件侧准备，并在 G10 进入时：

1. **软件侧零返工**：所有依赖硬件的代码路径已针对本契约编写，G10 只替换 `to-verify` 字段，不改 ABI。
2. **HIL 仪器可前置采购**：每项 HW-xxx 的接线、仪器、判据、记录要求已冻结，G10 启动即可执行。
3. **证据链可审计**：每项工作包的 L3（原始日志/波形）+ L4（独立解析/校验）要求冻结，禁止 G10 临时降低证据等级。

### 0.2 与现有文档的关系

| 文档 | 覆盖范围 | 与本规格关系 |
|------|---------|-------------|
| `d2000-hil-spec.md` | HW-001~014 的 HIL 接线/仪器/判据 | 本规格 §3 引用，不重复 |
| `hw-task-cards.md` | HW-001~014 的 Task Card | 本规格 §3 引用，不重复 |
| `phytium-d2000.json` | 机器可读 platform contract | 本规格 §2 引用，G10 替换 `to-verify` |
| **本规格** | **HW-001~018 全集接口契约 + 新增 HW-015~018** | **G10 权威冻结** |

### 0.3 编号说明（MASTER §21.18 权威）

MASTER §21.18 的 HW-001~018 编号为权威。早期 `hw-task-cards.md` 使用按子系统分组的旧编号，本规格以 MASTER 为准，并在 §3 交叉映射。**冲突时以本规格为准**。

---

## 1. G10 进入条件与 Exit Criteria

### 1.1 进入条件（MASTER §20.5）

- [ ] **G9 Passed**（见 [g9-candidate-report.md](../qa/g9-candidate-report.md)）— 当前 G9 候选状态为 PASS（等待 G10 硬件）
- [ ] 选定单一首发 board/SoC/revision、供应链和生命周期（飞腾 D2000 / FTC662）
- [ ] 获得完整 TRM、原理图、固件、errata、调试器、样机和厂商支持
- [ ] HIL 夹具、仪器、独立验证器就位

### 1.2 Exit Criteria（MASTER §20.5）

- [ ] 实体板从出厂介质冷启动，不依赖调试器/网络 loader
- [ ] 所有首发 I/O 在额定范围和故障条件下通过
- [ ] HIL/现场控制安全、性能、恢复和长稳达到冻结 SLO
- [ ] 工厂每台设备唯一身份、签名记录、readback、attestation 和测试报告完整
- [ ] 试点达到预定义运行周期与故障率，所有 P0/P1 问题闭环
- [ ] 商业、法务、支持、供应链、备件和 EOL 准备完成后才 Go

### 1.3 禁止捷径（MASTER §20.5）

- ❌ 同时首发多块板导致 BSP 和资格矩阵失控
- ❌ 只在开发板/JTAG 环境启动
- ❌ 把实验室一次成功当成量产稳定性
- ❌ 把 QEMU 测量结果当真机时延/DMA coherency/EMC/温度/介质寿命（G9 已禁止，G10 延续）
- ❌ 跳过 L4 独立验证只用 L3 自报告

---

## 2. 平台契约（机器可读，冻结 v0.1.0-draft）

### 2.1 契约文件

`configs/platform-contracts/phytium-d2000.json`，schema `eneros.platform-contract.v1`（见 `schema.json`）。

### 2.2 `to-verify` 字段清单（G10 必须替换）

以下字段当前为占位值，**G10 前任何 release 不得声明 D2000 支持**：

| 字段 | 当前值 | G10 来源 |
|------|--------|----------|
| `cpu.model` | `phytium-d2000 (FTC662)` | 手册核对 stepping |
| `gic.distributor.base` | `0x2f000000`（788529152） | 手册/板测替换 |
| `gic.redistributor.base` | `0x2f100000`（789577728） | 手册/板测替换 |
| `resources.uart0.range.base` | `0x28000000`（671088640） | 手册/板测替换 |
| `resources.gpio0.range.base` | `0x28010000`（671154176） | 手册/板测替换 |
| `dma.iommu.range.base` | `0x2b000000`（721420288） | 手册/板测替换 |
| `timer.frequency_source` | `to-verify` | 板测替换 |
| `memory[].base/size`（含 dma-pool） | 占位 | 板测替换 |
| `entropy` / `watchdog` / `rtc` | `[]` / `null` | G10 实测后填充 |

### 2.3 BSP adapter 接口（冻结，软件侧已实现）

```rust
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

QEMU 与 D2000 只允许在最前端 firmware/BSP adapter 有差异；manifest、boot manager 核心、BootInfo、槽状态机与内核路径必须相同（MASTER §6.1）。

---

## 3. HW-001~018 工作包接口契约

> 每项工作包定义：**目标 / 软件侧接口契约 / HIL 验收 / 证据等级 / QEMU 对应 / 阻塞原因**。
> HW-001~014 的 HIL 接线/仪器细节见 [d2000-hil-spec.md](./d2000-hil-spec.md) §4 与 [hw-task-cards.md](./hw-task-cards.md)，此处只补充接口契约与判据冻结。

### 3.1 HW-001 — 冻结首发 board/SoC/revision

| 维度 | 内容 |
|------|------|
| **目标** | 冻结首发硬件 BOM、SoC stepping、板 revision，锁定供应生命周期 |
| **软件侧接口契约** | `phytium-d2000.json` 的 `cpu.model` / `revision` 字段；BSP adapter 单一实现 |
| **HIL 验收** | BOM 冻结签字；供应商生命周期承诺书；stepping 与 errata 清单 |
| **证据等级** | L3：BOM + errata 文档；L4：供应链独立签字 |
| **QEMU 对应** | 无（QEMU 是开发平台，非首发板） |
| **阻塞原因** | 缺实体板与供应链签字 |

### 3.2 HW-002 — firmware/BootROM/TF-A/UEFI 链

| 维度 | 内容 |
|------|------|
| **目标** | BootROM→TF-A→U-Boot→EnerOS 唯一签名镜像可重复启动 |
| **软件侧接口契约** | boot manager handoff conformance checker（`crates/boot/boot-manager-uefi`）；BootInfo 结构冻结 |
| **HIL 验收** | 冷/热启动各 100 次；错误签名/旧版本/损坏 DTB 拒绝并留诊断状态；串口日志 + JTAG |
| **证据等级** | L3：原始串口/JTAG 日志；L4：独立 handoff 格式与签名校验工具 |
| **QEMU 对应** | qemu-prod 媒体启动 + bootctrl 断电（G3 已完成） |
| **阻塞原因** | 缺实体板与真实 BootROM/TF-A 二进制 |

### 3.3 HW-003 — RAM/DTB/PSCI/GIC/timer/UART

| 维度 | 内容 |
|------|------|
| **目标** | platform contract 实测：内存 map、DTB、PSCI、GICv3、architected timer、PL011 UART |
| **软件侧接口契约** | `PlatformAdapter` trait（§2.3）；`phytium-d2000.json` 的 `gic`/`timer`/`resources`/`psci` 字段 |
| **HIL 验收** | 全核连续运行、百万次 IPI/外部 IRQ、屏蔽/迁移/风暴注入；逻辑分析仪测 IRQ 延迟 |
| **证据等级** | L3：全核连续运行 + IRQ 延迟测量；L4：仪器独立测量 |
| **QEMU 对应** | GIC/timer/PSCI adapter、4 核启动、IRQ 绑定投递（G2/G3 已完成） |
| **阻塞原因** | 缺实体板（QEMU TCG 与真机 cache 模型不同） |

### 3.4 HW-004 — eMMC/NVMe backend

| 维度 | 内容 |
|------|------|
| **目标** | 真实介质 backend：断电/磨损/坏块语义 |
| **软件侧接口契约** | `BlockBackend` trait（`crates/drivers/storage`）；littlefs2 wear-level 接口 |
| **HIL 验收** | 连续 24h 压力；随机掉电 ≥1000 次；已确认事务不丢失且结构可挂载 |
| **证据等级** | L3：24h 压力 + 掉电矩阵；L4：离线 fsck 一致性校验 |
| **QEMU 对应** | virtio-blk 真实 I/O + 掉电矩阵（DRV-007/STO 已完成） |
| **阻塞原因** | 缺真实 eMMC/NVMe 介质（磨损/坏块无虚拟等价物） |

### 3.5 HW-005 — physical NIC

| 维度 | 内容 |
|------|------|
| **目标** | 物理 Ethernet MAC/PHY：性能/错误/恢复 |
| **软件侧接口契约** | `NicDriver` trait（`crates/drivers/net`）；smoltcp phy 抽象 |
| **HIL 验收** | 交换机双向线速/小包/大包/VLAN/断链重连/错误帧/24h 压力；抓包仪逐包核对 |
| **证据等级** | L3：24h 压力 + 断链重连；L4：抓包仪 + 交换机端口统计独立核对 |
| **QEMU 对应** | virtio-net 双向（DRV-008 已完成） |
| **阻塞原因** | 缺物理 NIC 与 PHY（自协商/link 状态无虚拟等价物） |

### 3.6 HW-006 — RS485/CAN/GPIO

| 维度 | 内容 |
|------|------|
| **目标** | 工业现场总线：电气/协议/HIL |
| **软件侧接口契约** | `SerialDriver` / `CanDriver` / `GpioPin` trait（`crates/drivers/fieldbus`） |
| **HIL 验收** | RS485 全波特率/DE 时序/多节点/碰撞；CAN 多节点/仲裁/bus-off 恢复；商业仪表互操作 |
| **证据等级** | L3：波形测量 + 互操作；L4：商业 Modbus/CAN 分析仪逐帧核对 |
| **QEMU 对应** | 虚拟 RS485/CAN 线级对端（DRV-011 已完成） |
| **阻塞原因** | 缺物理收发器与现场总线仪表 |

### 3.7 HW-007 — TRNG/TPM/TEE/security chip

| 维度 | 内容 |
|------|------|
| **目标** | 硬件信任根：熵/attestation/key |
| **软件侧接口契约** | `CsRng` trait（`crates/security/crypto`）；`Tpm2` client（`crates/security/tpm`）；attestation quote schema |
| **HIL 验收** | TRNG 熵测试（NIST SP 800-90B）；TPM 冷启动/自检/PCR/Quote/Seal/NV；独立 TPM2 工具校验 |
| **证据等级** | L3：全套 TPM 命令 + 熵测试；L4：独立 TPM2 工具 + 事件日志校验 |
| **QEMU 对应** | **无 TPM 虚拟等价物**（完全依赖 G10 真机） |
| **阻塞原因** | 缺板载 TPM 2.0 与 TRNG |

### 3.8 HW-008 — SMMU/DMA/cache coherency

| 维度 | 内容 |
|------|------|
| **目标** | DMA 隔离与压力：最小权限映射、故障隔离 |
| **软件侧接口契约** | `IommuProfile`（`PlatformAdapter::iommu()`）；`DmaState` 域记账 + `SYS_DMA_GRANT` 授权 |
| **HIL 验收** | eMMC/MAC 受限域运行；越权 DMA 阻断并报告；解绑后无残留映射；跨核压力 |
| **证据等级** | L3：越权阻断 + 跨核压力；L4：外部复核 SMMU 故障日志 |
| **QEMU 对应** | SMMUv3 虚拟 profile（DRV-009 冻结规格）；软件域记账已实现 |
| **阻塞原因** | 缺 SMMUv3 硬件（QEMU TCG 无真实 DMA 缓存竞争） |

### 3.9 HW-009 — watchdog/RTC/reset/power-loss

| 维度 | 内容 |
|------|------|
| **目标** | safe state/reason：失效复位、复位原因、棕断保护、安全关机 |
| **软件侧接口契约** | `WatchdogDriver` / `RtcDriver` trait；`ResetReasonStore`（bootctrl 分区） |
| **HIL 验收** | 停止喂狗/死锁/欠压/快速掉电注入；示波器测 pretimeout→flush→reset 时序；重启原因验证 |
| **证据等级** | L3：示波器时序 + 重启原因；L4：独立记录仪核对 |
| **QEMU 对应** | 虚拟 watchdog/reset（DRV-010 已完成） |
| **阻塞原因** | 缺物理 watchdog/RTC 与可编程电源 |

### 3.10 HW-010 — thermal/power/performance profile

| 维度 | 内容 |
|------|------|
| **目标** | throttling/边界：温度、功耗、性能曲线与降级策略 |
| **软件侧接口契约** | `ThermalZone` / `PowerBudget` trait；SLO 降级策略（`crates/observability/slo-alert`） |
| **HIL 验收** | 温箱全温区扫描；电压扰动；长时间性能曲线；throttling 触发与恢复 |
| **证据等级** | L3：温箱 + 电源轨迹；L4：独立仪器复核 |
| **QEMU 对应** | 无（QEMU 无真实热/功耗模型） |
| **阻塞原因** | 缺温箱、可编程电源、实体散热设计 |

### 3.11 HW-011 — board image/flasher

| 维度 | 内容 |
|------|------|
| **目标** | 客户/工厂 readback：出厂镜像烧录与回读验证 |
| **软件侧接口契约** | `xtask flash` 命令（fail-closed，禁止 `-kernel`/`dd if=`）；镜像 manifest + 签名 |
| **HIL 验收** | 实体 flasher 烧录；readback digest 与 manifest 一致；错误注入拒绝 |
| **证据等级** | L3：烧录 + readback 日志；L4：独立 digest 比对工具 |
| **QEMU 对应** | qemu-media 烧录（G3 已完成） |
| **阻塞原因** | 缺实体 flasher 与板载烧录接口 |

### 3.12 HW-012 — HIL parity suite

| 维度 | 内容 |
|------|------|
| **目标** | 与虚拟 contract 差异闭环：HIL 测试套件与 QEMU 对应项的差异全部解释或修复 |
| **软件侧接口契约** | HIL 测试 schema（`eneros.hil-result.v1`）；与 QEMU 证据 schema 对齐 |
| **HIL 验收** | 每项 HW-xxx 的 HIL 结果与 QEMU 对应项逐字段比对；差异归因（真机特性 vs 缺陷） |
| **证据等级** | L3：HIL + QEMU 对照矩阵；L4：独立差异审计 |
| **QEMU 对应** | 全部 HW-001~014 的 QEMU 对应项（见各项"QEMU 对应"） |
| **阻塞原因** | 缺 HIL 夹具与仪器 |

### 3.13 HW-013 — environmental/EMC/ESD

| 维度 | 内容 |
|------|------|
| **目标** | 目标认证报告：温湿度、EMC、ESD、振动（按产品认证标准） |
| **软件侧接口契约** | 无直接软件接口；环境数据进入 `ops-lifecycle` 的运维记录 |
| **HIL 验收** | 认证实验室报告；温湿度循环、EMC 辐射/传导、ESD 接触/空气放电、振动 |
| **证据等级** | L3：认证实验室原始数据；L4：认证机构签发报告 |
| **QEMU 对应** | 无（纯物理特性） |
| **阻塞原因** | 缺认证实验室预约与样机 |

### 3.14 HW-014 — secure lifecycle/fuse/debug lock

| 维度 | 内容 |
|------|------|
| **目标** | 不可逆步骤审计：secure boot fuse 烧录、debug port lock、key injection 生命周期 |
| **软件侧接口契约** | `SecureLifecycle` 模块（`crates/security/secure-boot`）；fuse 状态机；audit log schema |
| **HIL 验收** | fuse 烧录不可逆性验证；debug port lock 后 JTAG 拒绝；key injection 唯一性 |
| **证据等级** | L3：fuse/lock 操作日志；L4：独立验证器复测 JTAG 拒绝 |
| **QEMU 对应** | 无（QEMU 无 fuse/debug lock 硬件） |
| **阻塞原因** | 缺实体板 fuse 硬件（不可逆操作需样机） |

### 3.15 HW-015 — factory fixture/provisioning（新增）

| 维度 | 内容 |
|------|------|
| **目标** | 每台设备唯一 identity：工厂夹具、校准、出厂测试、追溯 |
| **软件侧接口契约** | `FactoryProvisioning` 接口（`crates/agents/ops-lifecycle/src/commissioning.rs` 已实现骨架）；设备 identity 注入 schema（`eneros.device-identity.v1`）；`ops-lifecycle/src/factory_reset.rs` 的 `PreserveRule::DeviceIdentity` 保留规则 |
| **HIL 验收** | 工厂夹具烧录唯一 device ID + 设备证书（SM2）；校准数据写入；出厂测试全项 PASS；readback 验证 identity 唯一性与签名 |
| **证据等级** | L3：每台设备的 provisioning 日志 + identity 证书；L4：独立验证器复测每台 identity 唯一且签名有效 |
| **接口契约冻结** | identity = `{device_id: UUIDv7, device_cert: SM2 DER, provisioned_at: Unix秒, factory_batch: string, hardware_rev: string}`；签名 = SM2 over `device_id || provisioned_at || factory_batch` |
| **QEMU 对应** | provisioning 流程逻辑已在 QEMU 验证（commissioning.rs 测试通过）；真机夹具属 G10 |
| **阻塞原因** | 缺实体工厂夹具与硬件 security chip（HW-007 前置） |

### 3.16 HW-016 — pilot manufacturing lot（新增）

| 维度 | 内容 |
|------|------|
| **目标** | 小批试产：良率、追溯、失败分析 |
| **软件侧接口契约** | `EolPolicy`（`crates/agents/ops-lifecycle/src/eol_policy.rs` 已实现）；`SparePartCommitment` schema；批次追溯记录格式 `eneros.manufacturing-lot.v1` |
| **HIL 验收** | 试产批次（≥N 台，N 由商业定）每台执行 HW-001~015 全套；良率统计；失败单元根因分析与闭环 |
| **证据等级** | L3：每台出厂测试报告 + 批次良率统计；L4：独立良率审计 + 失败分析报告复核 |
| **接口契约冻结** | 批次记录 = `{lot_id: string, devices: [{device_id, test_result, failure_mode?}], yield_rate: f32, analyzed_at: Unix秒}`；失败模式分类 = `{YIELD_DEFECT, CALIBRATION_FAIL, BURN_IN_FAIL, PROVISIONING_FAIL, OTHER}` |
| **QEMU 对应** | 无（试产是物理制造过程） |
| **阻塞原因** | 缺试产物料与产线 |

### 3.17 HW-017 — field pilot/SAT（新增）

| 维度 | 内容 |
|------|------|
| **目标** | 真实站点证据：现场 commissioning、site acceptance test（SAT）、有边界的设计合作客户 pilot |
| **软件侧接口契约** | `SiteAcceptanceTest`（`crates/agents/ops-lifecycle/src/commissioning.rs` 已实现 SAT 骨架）；SAT 报告 schema `eneros.sat-report.v1`；与 OPS-011（G8 site acceptance test）对齐 |
| **HIL 验收** | 真实客户站点安装；SAT 全项签字；预定义运行周期（≥T 天）故障率达标；所有 P0/P1 问题闭环；回滚预案演练 |
| **证据等级** | L3：SAT 签字报告 + 运行周期日志；L4：客户独立签字 + 第三方现场见证 |
| **接口契约冻结** | SAT 报告 = `{site_id, device_id, installed_at, accepted_at, accepted_by: 非实现方, test_items: [{id, passed, evidence_ref}], sign_off: SM2签名}`；运行周期 = `{duration_days, p0_count, p1_count, mtbf_hours, availability: f32}` |
| **QEMU 对应** | SAT 流程逻辑已在 QEMU 验证（commissioning.rs 测试通过）；真实站点属 G10 |
| **阻塞原因** | 缺真实客户站点与设计合作客户协议 |

### 3.18 HW-018 — supply/RMA/spare/EOL（新增）

| 维度 | 内容 |
|------|------|
| **目标** | 商业运营就绪：供应链、RMA、备件、EOL 全链路 |
| **软件侧接口契约** | `EolPolicy` + `SparePartCommitment`（`crates/agents/ops-lifecycle/src/eol_policy.rs` 已实现）；RMA identity rebind（`ops-lifecycle/src/rma.rs` 已实现，OPS-014）；secure erase（`ops-lifecycle/src/secure_erase.rs` 已实现） |
| **HIL 验收** | 供应链承诺书；RMA 流程演练（导出受控诊断 + 不泄露客户 secret）；备件库存与生命周期承诺；EOL 通知与补丁 SLA |
| **证据等级** | L3：RMA 演练日志 + 备件库存清单 + EOL 政策文档；L4：独立审计供应链 + RMA 数据擦除验证 |
| **接口契约冻结** | RMA 包 = `{device_id, rma_reason, diagnostic_bundle: 加密support-bundle, customer_secret_erased: bool, rebind_authorized: 双授权签字}`；备件承诺 = `{hardware_id, available_until: Unix秒, lead_time_days}`；EOL 政策见 `eol_policy.rs` 的 `VersionSupport`/`VulnerabilitySla`/`SparePartCommitment` |
| **QEMU 对应** | RMA/secure-erase/EOL-policy 流程逻辑已在 QEMU 验证（ops-lifecycle 测试通过）；真实供应链属 G10 |
| **阻塞原因** | 缺真实供应链合同与 RMA 产线 |

---

## 4. HW-001~018 依赖关系

```text
HW-001 (board 冻结) ─┬─→ HW-002 (固件链)
                     ├─→ HW-003 (RAM/GIC/timer/UART)
                     └─→ HW-014 (fuse/debug lock)
                           ↑
HW-007 (TRNG/TPM) ─────────┘
     ↓
HW-015 (factory provisioning) ──→ HW-016 (pilot lot)
                                       ↓
HW-017 (field pilot/SAT) ←────────────┘
     ↓
HW-018 (supply/RMA/spare/EOL)

并行执行（依赖 HW-001~003）：
  HW-004 (eMMC) / HW-005 (NIC) / HW-006 (RS485/CAN) / HW-008 (SMMU)
  HW-009 (watchdog/RTC) / HW-010 (thermal) / HW-011 (flasher) / HW-013 (EMC)

汇总：
  HW-012 (HIL parity) ← 全部 HW-001~011,013,014 结果
```

---

## 5. 软件侧 G10 前置准备（本阶段已完成）

以下软件侧工作已在 G8/G9 完成，G10 进入时**无需返工**，只需替换 `to-verify` 字段：

| 工作包 | 软件侧落位 | G10 动作 |
|--------|-----------|----------|
| platform contract | `configs/platform-contracts/phytium-d2000.json` | 替换 `to-verify` 字段 |
| BSP adapter | `crates/hal/board` + `crates/kernel/microkernel/src/hal/` | 实测后确认 trait 实现 |
| boot manager | `crates/boot/boot-manager-uefi` | handoff checker 已就绪 |
| storage backend | `crates/drivers/storage` (BlockBackend trait) | 替换为真实 eMMC 驱动 |
| net driver | `crates/drivers/net` (NicDriver trait) | 替换为真实 MAC 驱动 |
| fieldbus | `crates/drivers/fieldbus` (Serial/Can/Gpio trait) | 替换为真实收发器驱动 |
| TPM | `crates/security/tpm` (Tpm2 client) | 接入板载 TPM 2.0 |
| SMMU | `crates/hal/board` (IommuProfile) | 接入 SMMUv3 驱动 |
| watchdog/RTC | `crates/drivers/time` + `crates/drivers/watchdog` | 替换为真实驱动 |
| thermal | `crates/observability/slo-alert` (ThermalZone) | 接入温度传感器 |
| flasher | `xtask flash` (fail-closed) | 接入实体 flasher |
| secure lifecycle | `crates/security/secure-boot` | 接入 fuse/lock 操作 |
| factory provisioning | `crates/agents/ops-lifecycle/src/commissioning.rs` | 接入工厂夹具 |
| pilot lot | `crates/agents/ops-lifecycle/src/eol_policy.rs` | 填充真实批次数据 |
| SAT | `crates/agents/ops-lifecycle/src/commissioning.rs` | 真实站点签字 |
| RMA/spare/EOL | `crates/agents/ops-lifecycle/src/{rma,secure_erase,eol_policy}.rs` | 真实供应链合同 |

---

## 6. 证据等级与记录要求

### 6.1 证据等级（MASTER §00 契约）

| 等级 | 含义 | 适用 |
|------|------|------|
| L1 | 文档/声明 | 不作为交付证据（禁止捷径） |
| L2 | 自动化测试通过 | QEMU 侧证据 |
| L3 | 原始日志/波形/测量 | HIL 真机证据（实现方自报告） |
| L4 | 独立解析/校验/签字 | HIL 真机证据（非实现方独立） |

### 6.2 每项 HW-xxx 必须记录

- Git 提交 hash（构建来源）
- 构建命令与工具链版本
- 板卡 BOM 与固件 hash
- HIL 接线图与仪器型号
- 原始日志/波形/电源轨迹
- 阈值与判据
- 失败复现步骤（如有失败）

### 6.3 禁止捷径（证据侧）

- ❌ 用 L1 文档代替 L3/L4 实测
- ❌ 用 L3 自报告代替 L4 独立校验
- ❌ 用 QEMU 结果当真机结果（G9 已禁止，G10 延续）
- ❌ 跳过 L4 独立签字

---

## 7. 已知限制与风险

| 风险 | 影响 | 缓解 |
|------|------|------|
| D2000 全部寄存器值仍为占位 | G10 前任何 release 不得声明 D2000 支持 | `phytium-d2000.json` 的 `revision: 0.1.0-draft` |
| QEMU TCG 与真机 cache coherency 模型不同 | cache maintenance 原语在真机必须重新验证 | HW-008 强制 L3+L4 |
| 无 TPM 虚拟等价物 | TPM 相关验收完全依赖 G10 真机 | HW-007 无 QEMU 对应，明确标注 |
| HW-014 fuse 操作不可逆 | 一次失败即报废样机 | G10 需充足样机预算 |
| 试产良率不确定 | HW-016 可能阻塞 v1.0.0 | 商业预留备件与失败分析窗口 |
| 现场试点依赖客户配合 | HW-017 进度受客户影响 | 设计合作客户协议前置 |

---

## 8. 结论

本规格冻结 G10 硬件资格的 **18 项工作包接口契约**，使 G9 候选可以在无实体硬件条件下完成全部软件侧准备。G10 进入时：

1. **软件侧零返工**：所有 trait/schema/接口已冻结，G10 只替换 `to-verify` 字段
2. **HIL 仪器可前置采购**：每项判据与记录要求已冻结
3. **证据链可审计**：L3/L4 等级冻结，禁止 G10 临时降低
4. **新增 HW-015~018 完整覆盖**：factory provisioning / pilot lot / field pilot-SAT / supply-RMA-spare-EOL

**G9 候选报告引用本规格作为"硬件资格规格已冻结"的证据**（见 [g9-candidate-report.md](../qa/g9-candidate-report.md) §7）。
