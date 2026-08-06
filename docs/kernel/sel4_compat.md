# EnerOS 微内核与 seL4 兼容性分析（v0.127.0）

> **版本**：v0.127.0
> **crate**：`eneros-microkernel`
> **蓝图依据**：`蓝图/phase3.md` §v0.127.0（§5.1 选型对比 / §5.4 难点 / §8.4 兼容）
> **架构决策**：ADR-0001（Phase 3 采用 seL4 深度定制，方案 A）
> **配套文档**：[architecture.md](./architecture.md) / [syscall_spec.md](./syscall_spec.md)
> **最后更新**：2026-07-22

---

## 1. 目的与 ADR-0001 定位

依据 ADR-0001（方案 A），Phase 3 实际执行路径为 **seL4 深度定制 + 国产 CPU 适配 + 自研 Secure Boot**。本自研 Rust 微内核接口规范（[`eneros-microkernel`](../../crates/kernel/microkernel/src/lib.rs)）作为：

1. **fallback 参考**：seL4 路径在信创认证或深度定制验证中受阻时的自研内核启动基线；
2. **信创认证架构输入**：信创认证需提交的内核架构设计文档的代码化输入。

本文件的核心目标是**差异最小化**：逐项建立 EnerOS syscall 与 seL4 API 的映射，控制两者语义差异，以降低 v0.156.0（seL4 迁移/回归版本）的迁移成本（蓝图 §5.4 难点 / §8.4 兼容风险："与 seL4 API 差异需控制，否则 v0.156.0 迁移成本激增"）。

---

## 2. 设计理念对比

蓝图 §5.1 选型对比表原样收录：

| 项 | seL4（Phase 1-2） | 自研 Rust 微内核（Phase 3） | 选择理由 |
|----|----|----|----|
| 语言 | C | Rust | 内存安全，避免 UB |
| 验证 | 全形式化 | 选择性形式化 | 工程成本可控 |
| 能力模型 | 原生 capability | 继承 | 兼容用户态代码 |
| 代码量 | ~1 万行 C | < 3 万行 Rust | 可审计 |
| 国产化 | 含第三方依赖 | 完全自主 | 信创要求 |

---

## 3. 迁移映射表

EnerOS [`SyscallNum`](../../crates/kernel/microkernel/src/syscall.rs)（16 编号）与 seL4 API 的逐项映射：

| 编号 | EnerOS SyscallNum | seL4 API | 差异说明 |
|------|-------------------|----------|---------|
| 0 | `Yield` | `seL4_Yield` | 语义一致：让出剩余时间片，转入就绪队尾 |
| 1 | `ThreadCreate` | `seL4_Untyped_Retype`(TCB) + `seL4_TCB_Configure` | seL4 拆分为 Retype（对象分配）+ Configure（属性配置）两步；EnerOS 聚合为单个 syscall |
| 2 | `ThreadDelete` | `seL4_CNode_Delete`（TCB 槽） | seL4 无专用线程删除 API，经能力删除回收 TCB；EnerOS 提供专用销毁入口 |
| 3 | `IpcSend` | `seL4_Send` | 语义一致（seL4 端点阻塞发送语义） |
| 4 | `IpcRecv` | `seL4_Recv` | 语义一致：阻塞接收 |
| 5 | `IpcCall` | `seL4_Call` | 语义一致：同步发送并等待回复（蓝图 §4.2 契约即 seL4_Call 模型） |
| 6 | `MapPages` | `seL4_ARM_Page_Map` | EnerOS 简化参数：支持页数批量映射，seL4 为单页映射 |
| 7 | `UnmapPages` | `seL4_ARM_Page_Unmap` | 语义一致：解除映射 |
| 8 | `CapInsert` | `seL4_CNode_Copy`（+`seL4_CNode_Mint`） | EnerOS Insert 语义 ≈ Copy + Mint 的子集（插入时指定权限位） |
| 9 | `CapDelete` | `seL4_CNode_Delete` | 语义一致：删除槽位能力并回收 |
| 10 | `CapMove` | `seL4_CNode_Move` | 语义一致：槽位间移动，源槽位移除 |
| 11 | `Notify` | `seL4_Signal`（notification） | 语义一致：通知信号量投递，唤醒或累积 |
| 12 | `WaitFor` | `seL4_Wait`（notification） | 语义一致：阻塞等待并取走通知字 |
| 13 | `Schedule` | 无直接对应（seL4 调度由内核自动进行） | EnerOS 显式触发调度决策，为 ARINC 653 分区调度预留 |
| 14 | `TcbConfigure` | `seL4_TCB_SetPriority` / `seL4_TCB_SetAffinity` | seL4 为分项设置接口；EnerOS 聚合为单一配置接口（优先级 + 亲和性掩码一次设置） |
| 15 | `VSpaceMap` | `seL4_ARM_Page_Map`（指定 VSpace 根） | 参数模型一致：显式指定目标 VSpace 能力槽，等价于 seL4 以 VSpace 根为调用对象 |

---

## 4. 关键差异分析

### 4.1 编号空间

- **EnerOS**：`#[repr(u16)]` 非负编号（0~15，预留至 `0x00FF`），追加式扩展，禁插入/重排；
- **seL4**：API 号为负值常量（如 `seL4_SysCall = -1` 风格的内核保留编号空间）；
- **影响**：编号仅存在于 trampoline 边界，用户态库（`sel4-sys` 封装层）屏蔽差异，迁移不涉及编号语义变更。

### 4.2 消息传递模型

- **EnerOS**：消息经消息寄存器 word 传递，最多 **8 个 word**（`args[1]` 起）；
- **seL4**：消息经 **IPC Buffer**（每线程固定消息缓冲区）+ 消息寄存器（`seL4_MessageInfo` 描述长度）传递；
- **影响**：8 word 以内的短消息两者等价；长消息路径（IPC Buffer 拷贝）在 EnerOS 骨架中未定义，属后续 IPC 子系统实现版本的扩展点。

### 4.3 能力槽模型

- **EnerOS**：继承 seL4 **CSpace / CNode / 能力槽**概念（蓝图 §5.1"能力模型：继承"），`CapInsert` / `CapDelete` / `CapMove` 与 `seL4_CNode_*` 一一对应；
- **差异点**：`CapInsert` 将 Copy 与权限收缩（Mint）聚合为一次调用，属 seL4 两 API 的子集语义，迁移时可无损降级为 Copy+Mint 组合。

### 4.4 启动信息模型

- **EnerOS**：[`BootInfo`](../../crates/kernel/microkernel/src/boot.rs) 为极简四字段（`kernel_entry` / `dtb_phys` / `ram_region` / `hart_id`），由 Bootloader 构造传入；
- **seL4**：`seL4_BootInfo` 为富结构（含 untyped 区间列表、空闲 CNode 槽位区间、IPC Buffer 地址等）；
- **影响**：EnerOS BootInfo 不含能力系统自举信息（能力自举由启动子系统经 DTB/内存信息推导），fallback 路径下 root 任务初始化流程与 seL4 root task 有结构性差异，属已知且接受的差异。

---

## 5. 迁移指引（fallback 路径）

当 EnerOS 自研内核作为 fallback 路径启用时，Phase 1-2 基于 seL4 的用户态代码适配点如下：

1. **调用边界替换**：`crates/runtime/sel4-sys/` 的 seL4 syscall 调用 → 替换为本规范 [syscall_spec.md](./syscall_spec.md) 的 syscall 编号 + ARM64 trampoline（`svc #0`，`x8` 编号 / `x0`~`x5` 参数）；
2. **线程创建适配**：原 `seL4_Untyped_Retype` + `seL4_TCB_Configure` 两步调用 → 聚合为单次 `ThreadCreate`（入口/栈顶/优先级三参数），适配层负责合并参数；
3. **线程删除适配**：原 `seL4_CNode_Delete`（TCB 槽）→ 替换为 `ThreadDelete`（tid 语义），适配层需维护"TCB 能力槽 → tid"的映射；
4. **其余 14 项**：语义一致或为子集/超集关系（见 §3 映射表），适配层做薄封装即可。

**差异集中度**：全部迁移差异集中在**线程创建 / 线程删除两处聚合语义**及 §4.2/§4.4 所述消息缓冲与启动信息模型，其余接口为一一对应，满足蓝图 §5.4"与 seL4 的 API 差异最小化"要求。
