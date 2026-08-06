# EnerOS 微内核 syscall 接口规范（v0.127.0）

> **版本**：v0.127.0
> **crate**：`eneros-microkernel`
> **源码**：[`crates/kernel/microkernel/src/syscall.rs`](../../crates/kernel/microkernel/src/syscall.rs)
> **蓝图依据**：`蓝图/phase3.md` §v0.127.0（§3 交付物 / §4.2 接口定义 / §5.2 关键技术 / §9.2~9.3）
> **最后更新**：2026-07-22

---

## 1. 概述

syscall 是内核对外暴露的**唯一入口**。用户态不直接触达任何子系统，所有服务请求均经 syscall 陷入内核、由 trampoline 层完成参数搬运与校验后分发。

本版本定义 [`SyscallNum`](../../crates/kernel/microkernel/src/syscall.rs) 共 **16 个编号**（0~15），并以 `#[repr(u16)]` 对外暴露为稳定 ABI：

- 编号空间预留扩展至 `0x00FF`（蓝图 §9.2：避免 ABI 频繁变更导致用户态 trampoline 与内核失配）；
- **新增 syscall 必须追加编号，禁止插入或重排既有编号**；
- 用户态 trampoline 按数值匹配编号，不得依赖枚举声明顺序。

入口签名（由各 arch 实现 trampoline 后分发）：

```rust
pub type SyscallHandler = fn(SyscallNum, &[usize]) -> Result<usize, KernelError>;
```

---

## 2. ABI 说明

### 2.1 ARM64（目标架构）

- **陷入指令**：用户态经 `svc #0` 陷入内核态；
- **寄存器约定**：
  - `x8` = syscall 编号（`SyscallNum` 的 `u16` 判别值）；
  - `x0`~`x5` = 参数（trampoline 搬运为 `&[usize]` 传入 handler）；
  - `x0` = 返回值（`Ok` 时为非负结果值，`Err` 时按 `KernelError` 判别值编码）。

### 2.2 `ecall` 表述澄清（偏差 D7）

蓝图 §4.2 述及"用户态通过 `ecall` 指令陷入"，与 §5.2"ARM64 用 `svc #0`，x86 用 `syscall`"表述矛盾。**以 §5.2 为准**：`ecall` 为 RISC-V 指令，本项目目标架构为 ARM64，陷入指令统一为 `svc #0`。

### 2.3 x86_64

仅保留接口占位（[`SyscallHandler`] 签名架构无关），无 trampoline 实现——属 ADR-0001 平台裁剪项（BSP 收敛为飞腾+鲲鹏+QEMU 3 平台）。

---

## 3. 参数校验契约（蓝图 §9.3）

- 所有 syscall 入口的**参数校验统一在 trampoline 层完成**；
- 非法参数一律返回 [`KernelError::InvalidArgument`](../../crates/kernel/microkernel/src/error.rs)；
- **校验失败不进入子系统**：具体 handler 收到的 `args` 已通过 ABI 层校验，handler 仅做语义层校验（如能力权限、对象状态），不重复校验 ABI 层参数。

---

## 4. 十六个 syscall 契约表

> 参数均经 `&[usize]` 传入（`args[i]` 对应 ABI 寄存器 `xi`）。下表契约与 [`syscall.rs`](../../crates/kernel/microkernel/src/syscall.rs) 文档注释一致；**可能错误码为骨架预定义，实现期可增补**；阻塞性列为合理推断（实现期以子系统设计为准）。

| 编号 | 名称 | 语义 | 参数（`&[usize]` 位置与含义） | 返回 `Ok(usize)` 语义 | 可能错误码 | 阻塞性 |
|------|------|------|------------------------------|----------------------|-----------|--------|
| 0 | `Yield` | 主动触发调度，当前线程让出 CPU，转入就绪队尾 | 无参数 | `Ok(0)` 成功让出 | `BadState`（当前线程状态不可调度） | 非阻塞，立即返回 |
| 1 | `ThreadCreate` | 按入口地址、栈顶与优先级创建线程，进入就绪态 | `args[0]`=入口地址，`args[1]`=栈顶地址，`args[2]`=优先级 | `Ok(tid)` 新线程标识号 | `InvalidArgument`（入口/栈地址非法）、`OutOfMemory`（TCB 或内核栈配额耗尽） | 非阻塞 |
| 2 | `ThreadDelete` | 终止指定线程并回收 TCB 与内核栈资源 | `args[0]`=目标线程标识号 | `Ok(0)` 销毁成功 | `InvalidArgument`（线程不存在）、`PermissionDenied`（无权销毁）、`BadState`（不可销毁状态） | 非阻塞 |
| 3 | `IpcSend` | 同步发送 IPC 消息，不等待回复；对端无接收者时按 endpoint 语义排队或阻塞 | `args[0]`=目标 endpoint 能力槽，`args[1]` 起为消息寄存器 word（最多 8 个） | `Ok(n)` 实际发送的消息 word 数 | `InvalidCapability`（能力槽无效）、`PermissionDenied`（无发送权限） | 可阻塞（依 endpoint 语义） |
| 4 | `IpcRecv` | 阻塞接收 IPC 消息 | `args[0]`=本地 endpoint 能力槽 | `Ok(n)` 接收到的消息 word 数（消息经消息寄存器回传） | `InvalidCapability`、`PermissionDenied`（无接收权限）、`Timeout`（带超时变体时限内未收到） | 阻塞 |
| 5 | `IpcCall` | 同步发送并等待回复（单次往返，等价 IpcSend + IpcRecv 的原子组合） | `args[0]`=目标 endpoint 能力槽 `cap`，`args[1]` 起为消息寄存器数组 `msg`（最多 8 个 word） | `Ok(n)` `recv` 到的字节数 | `InvalidCapability`、`PermissionDenied`（无调用权限）、`Timeout`（带超时变体时限内未获回复） | 阻塞（直至对端 recv） |
| 6 | `MapPages` | 将物理页按权限位映射到当前 VSpace 的虚拟地址 | `args[0]`=物理页基址，`args[1]`=虚拟地址，`args[2]`=页数，`args[3]`=权限位 | `Ok(0)` 映射成功 | `InvalidArgument`（地址未按页对齐）、`OutOfMemory`（页表配额耗尽）、`PermissionDenied`（权限位超出能力许可） | 非阻塞 |
| 7 | `UnmapPages` | 解除当前 VSpace 指定虚拟地址区间的页映射 | `args[0]`=虚拟地址，`args[1]`=页数 | `Ok(0)` 解除成功 | `InvalidArgument`（地址未按页对齐或区间未映射） | 非阻塞 |
| 8 | `CapInsert` | 将内核对象能力插入当前 CSpace 指定槽位 | `args[0]`=目标槽位索引，`args[1]`=对象句柄，`args[2]`=权限位 | `Ok(0)` 插入成功 | `InvalidArgument`（槽位越界或已占用）、`InvalidCapability`（对象句柄无效）、`PermissionDenied`（权限位超出派生许可） | 非阻塞 |
| 9 | `CapDelete` | 删除当前 CSpace 指定槽位上的能力并回收槽位 | `args[0]`=目标槽位索引 | `Ok(0)` 删除成功 | `InvalidCapability`（槽位为空或无效） | 非阻塞 |
| 10 | `CapMove` | 将能力从源槽位移动到目标槽位，源槽位移除 | `args[0]`=源 CSpace/槽位编码，`args[1]`=目标 CSpace/槽位编码 | `Ok(0)` 移动成功 | `InvalidCapability`（源槽位无效）、`InvalidArgument`（目标槽位已占用）、`PermissionDenied`（无移动权限） | 非阻塞 |
| 11 | `Notify` | 向通知对象投递通知信号（信号量语义）：有等待者则唤醒，否则按位或累积到通知字 | `args[0]`=通知对象能力槽，`args[1]`=通知位掩码 | `Ok(0)` 投递成功 | `InvalidCapability`、`PermissionDenied`（无通知权限） | 非阻塞 |
| 12 | `WaitFor` | 阻塞等待通知位非零，随后取走并清零通知字 | `args[0]`=通知对象能力槽 | `Ok(bits)` 取走的通知位掩码 | `InvalidCapability`、`Timeout`（带超时变体时限内未等到） | 阻塞 |
| 13 | `Schedule` | 请求调度器立即重新评估就绪队列并选择下一运行线程 | 无参数 | `Ok(0)` 调度完成 | `BadState`（调度器未初始化） | 非阻塞 |
| 14 | `TcbConfigure` | 修改指定线程 TCB 的调度属性（优先级 / 核亲和性掩码） | `args[0]`=目标线程标识号，`args[1]`=优先级，`args[2]`=亲和性掩码 | `Ok(0)` 配置成功 | `InvalidArgument`（线程不存在或优先级越界）、`PermissionDenied`（无配置权限） | 非阻塞 |
| 15 | `VSpaceMap` | 将物理页映射到指定（非当前）VSpace 的虚拟地址，用于跨地址空间页表管理 | `args[0]`=目标 VSpace 能力槽，`args[1]`=物理页基址，`args[2]`=虚拟地址，`args[3]`=页数，`args[4]`=权限位 | `Ok(0)` 映射成功 | `InvalidCapability`（VSpace 能力无效）、`InvalidArgument`（地址未按页对齐）、`OutOfMemory`（页表配额耗尽）、`PermissionDenied`（权限位超出能力许可） | 非阻塞 |

### 4.1 IpcCall 契约详述（蓝图 §4.2 原文示例）

`IpcCall` 为蓝图 §4.2 给出的完整契约示例，原文如下：

```rust
// IpcCall：同步发送并等待回复
// 参数：cap = 目标 endpoint 能力槽
//       msg = 消息寄存器数组（最多 8 个 word）
// 返回：recv 到的字节数；错误码
// 约定：阻塞当前线程直到对端 recv
fn ipc_call(cap: CapSlot, msg: &[usize; 8]) -> Result<usize, KernelError>;
```

骨架接口中该契约经统一入口签名表达：`cap` 位于 `args[0]`，`msg` 为 `args[1]` 起的消息寄存器数组（最多 8 个 word）；返回 `Ok(n)` 为 `recv` 到的字节数；阻塞语义为"阻塞当前线程直到对端 recv 并完成回复"。其余 15 个 syscall 为骨架预定义契约，实现期可增补细化。

---

## 5. 错误码总表

统一错误码 [`KernelError`](../../crates/kernel/microkernel/src/error.rs)（`#[repr(u16)]`，判别值 1~8 为稳定 ABI；新增错误码必须追加，禁止插入或重排）：

| 判别值 | 变体 | 语义 |
|--------|------|------|
| 1 | `InvalidArgument` | 非法参数：trampoline 层参数校验失败时返回 |
| 2 | `OutOfMemory` | 内存不足：内核堆或分区内存配额耗尽 |
| 3 | `InvalidCapability` | 无效能力：Capability 引用不存在或已被撤销 |
| 4 | `PermissionDenied` | 权限不足：能力存在但不包含所请求的操作权限位 |
| 5 | `WouldBlock` | 非阻塞调用暂不可完成：资源暂不可用，调用方可重试 |
| 6 | `Timeout` | 超时：阻塞调用在指定时限内未完成 |
| 7 | `BadState` | 状态错误：对象当前状态不允许该操作（如重复初始化） |
| 8 | `NotSupported` | 不支持：当前平台或配置不提供该功能 |

---

## 6. 扩展规则

1. **追加编号**：新增 syscall 编号从 16 起顺序追加，禁止插入或重排既有 0~15 编号；
2. **预留区间**：`0x0010`~`0x00FF` 为预留扩展区间（蓝图原注"预留至 0x00FF"）；`0x0100` 及以上暂不启用，留待 ABI 主版本演进；
3. **错误码扩展**：新增 `KernelError` 变体判别值从 9 起追加，同样禁止插入或重排；
4. **用户态兼容**：用户态 trampoline 按 `u16` 数值匹配编号与错误码，不依赖枚举声明顺序，旧版用户态遇到未知编号/错误码应按 `NotSupported` / 未知错误处理。
