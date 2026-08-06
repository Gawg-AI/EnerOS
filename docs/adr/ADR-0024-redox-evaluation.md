# Redox vs seL4 内核基座评估结论（2026-08-03）

> 背景：EnerOS 内核基座决策期间，用户要求评估 github.com/redox-os/redox
> （Rust 微内核）是否值得替换 seL4（C + 形式化验证）或此前规划的自研 Rust
> 微内核。本结论基于对 redox-os/kernel 源码（38,848 行 Rust，提交 04db129）
> 与 seL4 官方资料的交叉评估。

## 结论：不值得替换 seL4；维持 seL4 路线

| 维度 | Redox | seL4 | 对 EnerOS 的影响 |
|---|---|---|---|
| 架构 | 微内核（Plan 9 风格 scheme + POSIX syscall），内核含 scheme(8k 行)/ACPI/futex/proc 等 | 极简 L4 微内核（~1 万行 C）：线程/CSpace/VSpace/IPC/中断 | seL4 信任基更小 |
| 验证 | 无形式化验证；Rust 内存安全（unsafe ~675 块，2.6%） | Isabelle/HOL 完整功能正确性证明 + 二进制验证 + ISK 信息流 | 工业安全论证只有 seL4 可行 |
| 确定性 | Round Robin，时间片 ~6.75ms，无优先级/无 deadline | MCS 混合关键性调度（优先级 + 时间预算，已验证） | EnerOS 10ms 周期/<1ms 抖动只有 MCS 满足 |
| Capability | FD 级能力（官方标 WIP），无内核对象能力树 | CSpace 能力空间，授权/撤销经形式化验证 | EnerOS 7 层纵深安全需要 CSpace |
| AArch64 | 移植进行中：3.4k 行代码齐全，CI 仅编译、无运行测试、无发布镜像 | 官方 tier-1：QEMU virt 官方平台 + seL4test 回归 | 生产可用性 seL4 完胜 |
| 成熟度 | 16.5k stars，桌面 OS 定位，0.9.0(2025-09)，无工业部署/LTS | seL4 Foundation，航空/汽车/国防部署，认证生态（DO-178C/ISO 26262） | 长期维护与供应链 seL4 占优 |
| 语言 | 全 Rust（真实优势） | C/汇编（验证补偿）；用户态 Rust 生态成熟（rust-sel4） | EnerOS 产品代码仍可用 Rust |

## 关键否决项

1. **确定性**：Redox 调度无优先级/预算，无法满足 EnerOS 的实时控制要求。
2. **Capability 安全**：Redox 无 CSpace 级内核能力模型，安全保证不达 EnerOS 纵深
   防御要求。
3. **AArch64 成熟度**：移植进行中、无运行验证，作为生产基座风险过高。
4. **认证路径**：Redox 无；seL4 可支撑 DO-178C A 级等最高等级。

## 对"替换自研微内核"的判断

自研 Rust 微内核（crates/kernel/microkernel）在 G2 QEMU 验证中暴露系统性缺陷
（异常返回/SMP/GIC），已证伪"自研可行"；Redox 虽为 Rust，但其内核同样未在
AArch64 上运行验证，且确定性/capability 模型与 EnerOS 需求不符。**seL4 是唯一
同时满足：AArch64 QEMU 官方支持 + 形式化验证 + CSpace capability + MCS 确定性
+ 工业认证生态的选项**。

## 可借鉴点（记录不采用）

- Redox 的 scheme/用户态驱动异步模型（SQE/CQE，类 io_uring）与 EnerOS 的
  Control Bus 理念同构，可作为 EnerOS 用户态服务框架的设计参考。
- Rust 语言安全性值得在 seL4 用户态（rust-sel4）中充分利用。
