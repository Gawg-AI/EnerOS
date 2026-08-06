# EnerOS 用户态 seL4 API 兼容层实施计划（v0.128.0）

> **版本**：v0.128.0（Phase 3 P3-A 第 2 版）
> **crate**：`eneros-microkernel`（[`crates/kernel/microkernel/`](../../crates/kernel/microkernel/src/lib.rs)）
> **蓝图依据**：`蓝图/phase3.md` §v0.128.0（§1 版本目标 / §3 交付物 `Sel4CompatLayer` trait）
> **架构决策**：ADR-0001（Phase 3 采用 seL4 深度定制，方案 A）
> **配套文档**：[sel4_compat.md](./sel4_compat.md)（差异事实表）/ [architecture.md](./architecture.md) / [build_system.md](./build_system.md)
> **最后更新**：2026-07-22

---

## 1. 兼容层目标

蓝图 §1 明确本版本业务价值之一：**兼容层设计降低 Phase 2 用户态代码迁移成本**。兼容层（`Sel4CompatLayer`）的核心目标是让 Phase 1-2 基于 seL4 API 编写的用户态代码，在启用自研内核 fallback 路径时能够**平滑迁移**——调用边界经薄封装翻译/聚合为本内核 syscall，而非逐处重写。

设计原则（承袭 v0.127.0，见 [sel4_compat.md](./sel4_compat.md) §1）：

- **差异最小化**：逐项建立 EnerOS syscall 与 seL4 API 的映射并控制语义差异，避免 v0.156.0（seL4 迁移/回归版本）迁移成本激增（蓝图 §5.4 难点 / §8.4 兼容风险）；
- **ADR-0001 定位**：Phase 3 主路径为 seL4 深度定制（方案 A），兼容层服务于 fallback 路径——seL4 路径在信创认证或深度定制验证中受阻时，自研内核可作为启动基线承接既有用户态代码；
- **契约先行**：本版本仅交付 trait 契约与 mock 测试，实现属 v0.156.0（见 §2 阶段计划），保证契约在中间版本演进中保持稳定。

---

## 2. 阶段计划

| 阶段 | 版本 | 内容 | 状态 |
|------|------|------|------|
| 契约定义 | **v0.128.0（本版本）** | [`compat.rs`](../../crates/kernel/microkernel/src/compat.rs) 定义 `Sel4CompatLayer` trait（蓝图 §3 原样两签名：`translate_syscall` / `shim_thread_create`）；CPT1~CPT3 mock 测试验证契约（translate 命中/未命中、shim 返回 Ok/Err）；**不含任何实现主体** | 本版本交付 |
| 启动验证 | v0.129.0 | Bootloader + 内核入口：内核首次在 ARM64 真机/QEMU 启动；shim 依赖的 syscall 执行环境尚未就绪 | 后续版本 |
| 子系统落地 | v0.130.0+ | 调度/内存/IPC/能力等子系统逐一实现（[architecture.md](./architecture.md) §4），`SyscallNum` 各编号获得真实可执行语义——shim 的翻译目标在这些版本逐步变为可执行；v0.131.0 启用 MMU 后 `.rodata` 只读等硬件级保障生效 | 后续版本 |
| 实现落地 | **v0.156.0** | `Sel4CompatLayer` 实现：`translate_syscall` 全映射表、ThreadCreate/ThreadDelete 聚合 shim、能力槽 ↔ tid 簿记；Phase 2 用户态代码经兼容层回归验证 | 目标版本 |

**中间版本衔接说明**：v0.128.0 → v0.156.0 之间，兼容层契约的稳定性依赖两条既有约束——① `SyscallNum` 编号追加式扩展、禁插入/重排（[sel4_compat.md](./sel4_compat.md) §4.1，预留至 `0x00FF`），翻译目标编号不会漂移；② 蓝图 §3 的 trait 签名为接口类交付物，任何修改需走版本蓝图修订。子系统实现版本（v0.130.0+）只填充 syscall 语义，不改变编号与签名，故 v0.156.0 实现兼容层时契约零漂移。

---

## 3. shim 语义设计

### 3.1 translate_syscall 映射表

`translate_syscall(&self, sel4_num: u16, args: &[usize]) -> Option<SyscallNum>` 的翻译**事实基础**为 [sel4_compat.md](./sel4_compat.md) §3 的 16 行迁移映射表（EnerOS `SyscallNum` 0~15 ↔ seL4 API 逐项对应），本文件不重复收录，仅定义翻译规则分组：

| 翻译规则 | 涉及编号 | 语义 |
|---------|---------|------|
| 直译（一一对应或子集/超集关系，薄封装即可） | 0 `Yield`、3 `IpcSend`、4 `IpcRecv`、5 `IpcCall`、6 `MapPages`、7 `UnmapPages`、8 `CapInsert`、9 `CapDelete`、10 `CapMove`、11 `Notify`、12 `WaitFor`、13 `Schedule`、14 `TcbConfigure`、15 `VSpaceMap` | 返回 `Some(对应 SyscallNum)`；`CapInsert` 为 Copy+Mint 子集语义，可无损降级为组合调用（[sel4_compat.md](./sel4_compat.md) §4.3） |
| 聚合 shim | 1 `ThreadCreate`、2 `ThreadDelete` | 见 §3.2 / §3.3 |
| 无对应 | 16 编号以外的 seL4 调用 | 返回 `None`，由调用方决定降级或报错 |

编号空间差异（EnerOS 非负 `u16` vs seL4 负值常量）仅存在于 trampoline 边界，由用户态库（`sel4-sys` 封装层）屏蔽，迁移不涉及编号语义变更（[sel4_compat.md](./sel4_compat.md) §4.1）。

### 3.2 ThreadCreate 聚合 shim

**差异事实**（[sel4_compat.md](./sel4_compat.md) §3 编号 1）：seL4 线程创建拆分为两步——

1. `seL4_Untyped_Retype`(TCB)：从 untyped 内存分配 TCB 对象到 CNode 槽（对象分配）；
2. `seL4_TCB_Configure`：配置 TCB 属性（优先级、IPC Buffer、CSpace/VSpace 根等）。

EnerOS `ThreadCreate` 将对象分配与属性配置**聚合为单个 syscall**。`shim_thread_create` 的内部展开语义：

1. **参数合并**：适配层截获 seL4 调用序列的 Retype 参数（对象类型 = TCB）与 Configure 参数，合并为 `ThreadCreate` 单次调用的参数包（契约签名为 `name` + `entry` 两参；完整参数面为入口/栈顶/优先级三参数，见 [sel4_compat.md](./sel4_compat.md) §5 迁移指引第 2 条）；
2. **单次陷入**：对内核只发起一次 `ThreadCreate` syscall，内核侧一次完成 TCB 分配、初始化并挂入就绪队列；
3. **返回映射**：成功返回 `Ok(tid)`；失败返回 [`KernelError`](../../crates/kernel/microkernel/src/error.rs)（如 `OutOfMemory` / `InvalidArgument` / `NotSupported`）；
4. **簿记**：适配层记录"TCB 能力槽 ↔ tid"映射，供 ThreadDelete shim（§3.3）反查。

### 3.3 ThreadDelete shim

**差异事实**（[sel4_compat.md](./sel4_compat.md) §3 编号 2）：seL4 无专用线程删除 API，经 `seL4_CNode_Delete`（TCB 槽）删除能力、由内核回收 TCB 对象；EnerOS 提供专用销毁入口 `ThreadDelete(tid)`。

shim 回收语义：

1. **目标翻译**：适配层按 §3.2 的簿记将 `CNode_Delete` 的目标 TCB 能力槽反查为 `tid`，改发 `ThreadDelete(tid)`；
2. **回收对齐**：seL4 的 Delete 仅撤销能力（对象回收由内核引用计数完成），本内核 `ThreadDelete` 为显式销毁——适配层需保证删除前目标线程已安全退出，该语义校验点属 v0.156.0 实现范畴；
3. **簿记清理**：销毁成功后清除对应"能力槽 ↔ tid"映射项，后续对该槽的 Delete 按空槽语义处理。

---

## 4. 不兼容面与兜底策略

| 不兼容面 | 事实（出处） | 兜底策略 |
|---------|-------------|---------|
| 显式 `Schedule`（编号 13）无 seL4 对应 | seL4 调度由内核自动进行，无直接对应 API（[sel4_compat.md](./sel4_compat.md) §3 编号 13） | ① seL4 来源代码不会发出该调用，shim 层直通即可，无适配负担；② 自研内核原生代码使用该 syscall 时按 **ARINC 653 分区调度语义直译**——显式触发调度决策，为分区时间片配置预留，无 shim 需求 |
| 消息寄存器 8 word vs IPC Buffer | EnerOS 短消息经消息寄存器传递（最多 8 word）；seL4 经 IPC Buffer + `seL4_MessageInfo`（[sel4_compat.md](./sel4_compat.md) §4.2） | 8 word 以内短消息两者等价，直译；长消息路径由**封装层适配**——适配层负责 IPC Buffer 与消息寄存器之间的拷贝/拆装（EnerOS 长消息路径属后续 IPC 子系统实现版本的扩展点） |
| 启动信息模型差异 | `seL4_BootInfo` 富结构（untyped 区间/空闲槽位/IPC Buffer 地址）vs EnerOS 极简四字段（[sel4_compat.md](./sel4_compat.md) §4.4） | 已知且接受的差异：能力自举由启动子系统经 DTB/内存信息推导；root task 初始化流程存在结构性差异，**不属 shim 可抹平面**，迁移时重写 root task 初始化序列 |
| `CapInsert` 子集语义 | EnerOS Insert ≈ seL4 Copy + Mint 子集（[sel4_compat.md](./sel4_compat.md) §4.3） | 可无损降级为 Copy+Mint 组合调用，shim 层做一拆二即可 |

**差异集中度结论**（承 [sel4_compat.md](./sel4_compat.md) §5）：全部迁移差异集中在**线程创建/线程删除两处聚合语义**及消息缓冲、启动信息模型，其余接口一一对应——兜底策略覆盖全部已知不兼容面，无未规划项。

---

## 5. 与 sel4_compat.md 的分工

| 维度 | [sel4_compat.md](./sel4_compat.md)（v0.127.0） | 本文档（v0.128.0） |
|------|----------------------------------------------|-------------------|
| 定位 | **差异事实表**（What） | **实施计划**（How / When） |
| 内容 | 16 行迁移映射、编号空间/消息模型/能力槽/启动信息四项差异分析、fallback 迁移指引 | 阶段计划（契约 → 实现）、shim 内部展开语义、不兼容面兜底策略 |
| 回答的问题 | 两套 API "差在哪" | 兼容层"何时做、怎么做、做不到的怎么兜" |
| 更新时机 | `SyscallNum` 编号或 seL4 对标版本变更时 | 阶段里程碑达成或兜底策略调整时 |

两文档单向依赖：本文档引用 sel4_compat.md 的映射表与差异分析为事实基础，sel4_compat.md 不依赖本文档。

---

## 6. 源码索引

| 文件 | 内容 |
|------|------|
| [`crates/kernel/microkernel/src/compat.rs`](../../crates/kernel/microkernel/src/compat.rs) | `Sel4CompatLayer` trait 契约（`translate_syscall` / `shim_thread_create`）+ CPT1~CPT3 mock 测试 |
| [`crates/kernel/microkernel/src/syscall.rs`](../../crates/kernel/microkernel/src/syscall.rs) | `SyscallNum` 16 编号（翻译目标编号空间） |
| [`crates/kernel/microkernel/src/error.rs`](../../crates/kernel/microkernel/src/error.rs) | `KernelError` 八变体（shim 失败返回类型） |
| [`docs/kernel/sel4_compat.md`](./sel4_compat.md) | 差异事实表（16 行映射，本文档 §3 的事实基础） |
| [`docs/kernel/syscall_spec.md`](./syscall_spec.md) | syscall 接口规范（trampoline ABI：`svc #0`，`x8` 编号 / `x0`~`x5` 参数） |
