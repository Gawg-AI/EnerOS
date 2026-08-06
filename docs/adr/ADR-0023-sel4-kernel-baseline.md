# ADR-0023：seL4 微内核作为 EnerOS 内核基座

> 状态：Accepted（2026-08-02）
> 决策者：产品负责人（用户指令）
> 前置：G2 自研微内核（crates/kernel/microkernel）QEMU 验证暴露系统性缺陷（SVC ELR 语义、
> 调度 TTBR0 切换缺失、kstack 预映射、GICv3 配置、异常返回帧交互），修复成本高且
> 风险不可控；用户决策完全替换为 seL4。

## 1. 背景

原蓝图（MASTER 修订前）规定"自研 Rust microkernel 唯一产品主线"。G2 阶段在 QEMU 上
真实验证时，自研内核存在以下未经验证/有缺陷的底层机制：

1. QEMU TCG 的 SVC 异常 ELR 语义与 ARM 标准存在差异（ELR=svc+4），异常返回路径
   与帧交互在多次修复后仍导致用户寄存器被污染（x8 落入 UART 地址、执行流漂移）。
2. 调度器上下文切换不切换 TTBR0，多进程地址空间隔离不成立。
3. 内核自身运行在 TTBR0（低地址），切换 TTBR0 后当前线程内核栈在新表不可见。
4. GICv3 初始化缺失 ICC_PMR/ICC_CTLR/GICR_CTLR/IGROUPR/ARE_NS 等关键配置。
5. 全局锁释放与 daifclr 顺序错误导致首个 IRQ 死锁。

上述问题表明该内核从未在任何平台真实运行过（仓库无 QEMU 系统证据），继续修复属于
"重写内核"级别工作量，且与 G3 驱动/存储/网络（依赖可靠的内核 IPC/capability/MMIO）
冲突。用户决定：采用 GitHub 上成熟稳定的开源微内核 **seL4** 完全替换。

## 2. 选型依据（GitHub 调研，2026-08-02）

| 候选 | 星级 | 成熟度 | AArch64 QEMU | 微内核模型 | 结论 |
|---|---|---|---|---|---|
| seL4/seL4 | 5.7k | 工业级、形式化验证 | 官方 qemu-arm-virt | capability+IPC+EL0 | **采用** |
| redox-os/redox | 16k | 类 Unix 单体内核 | ARM64 支持不成熟 | 非微内核 | 否 |
| arceos-org/arceos | 775 | 实验性模块化 OS | 支持 | unikernel 非进程隔离 | 否 |
| tock | — | 嵌入式 TCB | 支持 | 单地址空间分区 | 否 |
| 其他 Rust microkernel（GitHub 搜索） | <100 | 教学/小众 | 部分 | — | 否 |

seL4 满足：形式化验证（唯一）、capability 模型（与 EnerOS cap 设计同构）、AArch64
QEMU 官方支持（qemu-arm-virt/gic-v3/4 核）、用户态 Rust 生态（rust-sel4）、工业部署
（汽车/航空/国防）。

## 3. 架构映射（EnerOS 能力 → seL4 原语）

| EnerOS 概念 | seL4 原语 |
|---|---|
| EL0 进程/服务 | seL4 TCB + CSpace + VSpace（独立地址空间） |
| capability 授权/撤销 | seL4 CSpace（mint/copy/revoke/delete） |
| IPC 消息/端点 | seL4 Endpoint（Send/Recv/Call/Reply/Notify） |
| MMIO 映射 | seL4_ARM_Page_Map + DeviceMemory 帧 |
| IRQ 绑定 | seL4_ARM_IRQHandler（Control/SetIRQHandler/Ack） |
| DMA 缓冲 | seL4 物理帧（Untyped 派生） |
| 调度/优先级 | seL4 MCS（budget/period）或标准优先级 |
| 时间 | seL4 Time 对象（MCS）/定时器用户态驱动 |
| 中断控制器 | 用户态 GIC 驱动（IRQ cap 抽象） |
| 系统调用 ABI | seL4 syscall ABI（libsel4 + rust-sel4） |

## 4. 迁移范围与影响

- **内核**：crates/kernel/microkernel 移除产品地位（research 保留），seL4 内核镜像由
  正式构建链产出（elfloader 打包）。
- **boot 链**：UEFI boot manager（G1 产物）保留验签/测量/回滚职责，加载 seL4
  elfloader（含 root task 镜像）——seL4 自 elfloader 启动到 EL1，root task 运行于 EL0。
- **用户态服务**：root task（Rust）经 seL4 API 创建服务进程；libeneros/abi-syscall
  适配为 seL4 ABI（或直接使用 rust-sel4）。
- **驱动**：drv-svc（virtio-mmio/blk/net/rng、PL011）保持设备驱动逻辑，访问层替换为
  seL4 MMIO/IRQ cap；platform contract（DRV-001）继续作为设备资源单一事实源。
- **存储**：storage 服务（CONFIG/DATA/LOGS/TSDB）保持，块后端经 seL4 用户态
  virtio-blk 驱动。
- **网络**：drv-net + smoltcp 栈保持，经 seL4 virtio-net 驱动与外部对端互通。
- **门禁**：G2 判定重定义为"seL4 启动 + root task EL0 + capability/IPC 闭环"；
  G3 判定不变（真实 virtio I/O、断电矩阵、驱动重启、故障注入），全部在 seL4 上验收。

## 5. 禁止捷径（沿用 MASTER 规则）

- 不把 seL4 自带能力（验证、隔离、IPC）伪称为 EnerOS 自研；产品价值 = seL4 之上的
  服务/驱动/存储/协议/Agent 闭环。
- 不允许用 seL4 的 C 教程 demo 冒充 EnerOS 系统验收；证据要求不变。
- 不允许跳过 D2000 真机验证（G10）或把 QEMU 通过当作真机通过。

## 6. 后续开发入口

- 构建：seL4 kernel（qemu-arm-virt）+ elfloader + root task（Rust，rust-sel4）。
- 仓库落位：`third_party/seL4/`（vendored 固定版本 + 构建脚本）、
  `crates/runtime/sel4-root/`（root task）、`xtask sel4-build` 命令。
- 蓝图：MASTER 已同步修订（seL4 基座唯一产品路径），本 ADR 为后续开发唯一依据。
