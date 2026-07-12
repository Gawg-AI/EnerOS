# EnerOS v0.15.0 — 多核启动与 IPI Spec

> **蓝图依据**：`蓝图/phase0.md` §v0.15.0（第 3181-3349 行）
> **合规性**：蓝图 §43.1（no_std）、§43.2（非瓶颈版本，签名可编译）
> **Phase 0 定位**：P0-E 起点，多核能力奠基

---

## Why

EnerOS 当前（v0.14.0 及之前）仅运行在单核上。多核是性能与隔离的关键前提——RTOS 独占 Core 0、Agent 跑 Core 1+ 的混合关键性架构依赖多核能力。v0.15.0 需要实现 SMP 启动（Secondary Core 唤醒）、核间中断（IPI）、核间通信通道，使所有核启动并运行，为 v0.16.0 多核调度和 v0.17.0 内存一致性奠基。

---

## What Changes

### 新增
- 新建顶层 crate `smp/`（`eneros-smp`），提供多核启动、IPI、核间通道
- 新建文档 `docs/smp-boot-design.md`（SMP 启动设计）
- 新建文档 `docs/ipi-mechanism.md`（IPI 机制）

### 修改
- workspace `Cargo.toml`：members 添加 `"smp"`，version 升至 `0.15.0`
- `Makefile`：VERSION 升至 `0.15.0`，新增 `smp-build` / `smp-test` 目标
- `.github/workflows/ci.yml`：版本标识 v0.15.0，新增 smp crate cross-build 步骤
- `ci/src/gate.rs`：注释含 v0.15.0

### 不修改（外科手术原则）
- **不修改** HAL crate（`eneros-hal`）的 GICv3 实现（当前是单核简化版，多核 redistributor 扩展不在本版本范围）
- **不修改** kernel/hello/panic/time/watchdog 等现有 crate

---

## 关键设计决策（Karpathy 原则应用）

### D1：使用 PSCI 而非唤醒地址寄存器
**原因**：蓝图 §4.5 用唤醒地址寄存器（`SECONDARY_ENTRY_REG = 0xD8`），但蓝图 §5.4 明确指出"唤醒地址寄存器 SoC 特定，需 BSP 适配"——这是难点。PSCI（Power State Coordination Interface）是 ARM 标准，QEMU `virt` 原生支持，飞腾/鲲鹏也支持，更可移植且更简洁。

**方案**：`wake_secondary()` 通过 `hvc #0` 调用 PSCI `CPU_ON`（函数号 `0x8400_000E`），传入目标 core 的 MPIDR 和入口地址。无需 SoC 特定唤醒地址寄存器适配。

### D2：不依赖 HAL，自身实现 aarch64 专属代码
**原因**：HAL 的 GICv3 是单核简化实现（[gicv3.rs](file:///e:/eneros/hal/src/arm64/gicv3.rs) `locate_current_core_gicr` 返回固定地址）。修改 HAL 会违反外科手术原则。`eneros-smp` 自身实现 `read_core_id()`（`mpidr_el1`）、SGI 发送（`icc_sgi1r_el1`）、PSCI 调用（`hvc`），与 panic crate v0.14.0 的 `read_core_id()` 模式一致。

### D3：CoreInfo 表用 spin::Mutex，不用 static mut
**原因**：蓝图 §4.5 用 `static mut CORES`，在 no_std 多核场景下不安全。改用 `spin::Mutex<[CoreInfo; 8]>` 保护并发访问。CoreState 用 `AtomicU8` 存储以支持无锁状态查询（避免锁竞争）。

### D4：channel.rs 实现 per-core 消息邮箱
**原因**：蓝图 §3 列出 channel.rs（~150 行）但未详细设计。设计为 per-core 固定大小（16 槽）的 `IpiMsg` 环形队列。`ipi_send` 时往目标 core 邮箱塞消息 + 发 SGI 触发中断；目标 core 的 IPI handler 从自己邮箱取消息并分发。

### D5：aarch64 专属代码用 cfg gate
**原因**：`mpidr_el1`、`icc_sgi1r_el1`、`hvc`、`sev`/`wfe` 是 aarch64 专属。host 测试时提供 stub。参考 v0.6.0 HAL 和 v0.14.0 panic 的 `#[cfg(target_arch = "aarch64")]` 模式。

### D6：依赖最小化
**原因**：Karpathy 简洁原则。仅依赖 `spin`（Mutex）和 `heapless`（环形队列）。不依赖 `eneros-hal`（D2）、不依赖 `eneros-time`（IPI 不需要时间戳）。

---

## Impact

- **Affected specs**：P0-E 起点；为 v0.16.0 多核调度（核亲和性/绑核）、v0.17.0 内存一致性（TLB shootdown）奠基
- **Affected code**：
  - 新增：`smp/Cargo.toml`、`smp/src/{lib.rs, boot.rs, ipi.rs, channel.rs}`
  - 修改：`Cargo.toml`（workspace）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
  - **不修改**：HAL/kernel/hello/panic/time/watchdog 源码
- **依赖关系**：
  - 依赖 v0.6.0 HAL 核心（已满足）—— 但不直接依赖 HAL crate（D2）
  - 无新增外部依赖阻塞
- **回归风险**：低。新增 crate，不触碰现有代码

---

## ADDED Requirements

### Requirement: SMP 启动框架
系统 SHALL 提供 no_std SMP crate `eneros-smp`，支持 Secondary Core 唤醒、核间中断和核间通信通道。

#### Scenario: 初始化 SMP
- **WHEN** 调用 `smp_init(core_count)`
- **THEN** 初始化 CoreInfo 表，所有 core 状态为 Offline，记录核数

#### Scenario: 唤醒 Secondary Core
- **WHEN** 调用 `wake_secondary(core_id, entry)`
- **THEN** 通过 PSCI `CPU_ON` 唤醒目标 core，目标 core 状态变为 Booting

#### Scenario: Secondary Core 上线
- **WHEN** Secondary Core 执行 `secondary_entry()`
- **THEN** 读取自身 core_id，标记状态为 Online，等待 IPI

### Requirement: CoreState 状态机
系统 SHALL 维护每个 core 的状态：Offline → Booting → Online → Halted。

#### Scenario: 状态转换
- **WHEN** core 唤醒时
- **THEN** Offline → Booting
- **WHEN** secondary_entry 完成初始化时
- **THEN** Booting → Online
- **WHEN** core 故障或关机时
- **THEN** Online → Halted

#### Scenario: 唤醒超时容错
- **WHEN** Secondary 唤醒失败或超时
- **THEN** 标记 Offline，BSP 继续单核运行（蓝图 §4.4）

### Requirement: 核间中断 IPI
系统 SHALL 通过 GICv3 SGI（Software Generated Interrupt）实现核间中断。

#### Scenario: 发送 IPI
- **WHEN** 调用 `ipi_send(target, msg)`
- **THEN** 将 msg 塞入目标 core 邮箱，写 `icc_sgi1r_el1` 发 SGI 触发中断

#### Scenario: 广播 IPI
- **WHEN** 调用 `ipi_broadcast(msg)`
- **THEN** 向所有核（target=0xFFFF）发 SGI

#### Scenario: 注册 IPI handler
- **WHEN** 调用 `register_ipi_handler(msg_type, handler)`
- **THEN** 将 handler 存入静态表，对应 msg_type 的 IPI 到达时调用

### Requirement: IPI 消息类型
系统 SHALL 支持蓝图定义的 IPI 消息类型。

#### Scenario: 消息类型
- **WHEN** 使用 IpiMsg 枚举
- **THEN** 支持 Reschedule / Shutdown / TlbShootdown(u64) / Custom(u32) 四种

### Requirement: 核间通信通道
系统 SHALL 提供 per-core 消息邮箱作为 IPI 数据载体。

#### Scenario: 邮箱投递
- **WHEN** ipi_send 投递消息到目标 core 邮箱
- **THEN** 消息入队（若满则丢弃，不阻塞）

#### Scenario: 邮箱取消息
- **WHEN** IPI handler 被触发
- **THEN** 从自身 core 邮箱取出所有待处理消息并分发

---

## MODIFIED Requirements

### Requirement: Workspace 版本基线
workspace `Cargo.toml` 的 version 从 `0.14.0` 升至 `0.15.0`，members 列表新增 `"smp"`。

---

## REMOVED Requirements

无。本版本为纯新增。

---

## 蓝图符合性核对

| 蓝图条目 | 对应实现 |
|---------|---------|
| §1 核心目标：SMP 启动/Secondary 唤醒/IPI/核间通道 | boot.rs / ipi.rs / channel.rs |
| §3 交付物：boot.rs(~200行)/ipi.rs(~180行)/channel.rs(~150行) | 三模块对应 |
| §3 接口：wake_secondary/ipi_send/ipi_broadcast/smp_init/register_ipi_handler | 完整实现 |
| §4.1 CoreInfo/CoreState/IpiMsg 数据结构 | 完整实现 |
| §4.4 错误处理（唤醒超时→Offline 单核运行；IPI 丢失→SGI 保证可靠） | 实现 |
| §5.4 难点：唤醒地址 SoC 特定 | D1：用 PSCI 替代，消除 SoC 适配难点 |
| §6 测试计划（单元 CoreState≥80%/集成所有核打印 ID/性能 IPI<5μs/回归 v0.14.0/故障注入） | checklist 覆盖 |
| §7 验收标准（所有核启动/IPI 可通信/延迟<5μs/文档齐全） | checklist 覆盖 |
| §43.1 no_std | `#![cfg_attr(not(test), no_std)]` |
| §43.2 非瓶颈版本签名可编译 | 所有 trait/struct 签名完整可编译 |
| §43.3 GPU | N/A（蓝图 §6.6 明确） |
| §8.5 坑点：Secondary 必须先初始化 GIC redistributor | secondary_entry 中调用 GIC redistributor 初始化（自身实现，不修改 HAL） |
