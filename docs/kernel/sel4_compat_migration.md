# seL4 → EnerOS 用户态兼容性迁移指南

> **版本**：v0.156.0（P3-G 首版本）
> **ADR-0001 定位**：方案 A（seL4 深度定制），本版本为"seL4 深度定制配置变更后的用户态兼容性验证"
> **最后更新**：2026-07-29

---

## 1. 概述

v0.156.0 实现 seL4 用户态 API 兼容层，验证 seL4 深度定制配置变更后 Phase 1-2 用户态代码的兼容性。方案 A 下 seL4 内核保留，用户态无需"迁移"，兼容层确保定制配置不影响现有用户态运行。

### 1.1 业务价值

- 用户态代码资产零成本复用
- 避免重写 Agent Runtime、设备协议栈、LLM 引擎等大量已验证代码
- 为 v0.157.0 全功能回归奠定基础

### 1.2 架构设计

采用"垫片库（shim）+ syscall 翻译"双层方案：

```
用户态 seL4 调用
       ↓
seL4 Shim 拦截（编译时替换）
       ↓
SyscallTranslator 翻译（运行时映射）
       ↓
自研内核 syscall 执行
       ↓
结果翻译返回
       ↓
用户态无感知
```

---

## 2. API 映射表

### 2.1 IPC 类

| seL4 syscall | 编号 | 自研内核 SyscallNum | 说明 |
|---|---|---|---|
| `seL4_Send` | 2 | `IpcSend` (3) | IPC 发送 |
| `seL4_Recv` | 3 | `IpcRecv` (4) | IPC 接收 |
| `seL4_Call` | 6 | `IpcCall` (5) | 同步往返 IPC |
| `seL4_Reply` | 13 | `IpcSend` (3) | 回复（降级为 Send） |
| `seL4_ReplyRecv` | 14 | `IpcRecv` (4) | 回复并接收（降级为 Recv） |
| `seL4_NBSend` | 11 | `IpcSend` (3) | 非阻塞发送（降级） |
| `seL4_NBRecv` | 12 | `IpcRecv` (4) | 非阻塞接收（降级） |

### 2.2 通知类

| seL4 syscall | 编号 | 自研内核 SyscallNum | 说明 |
|---|---|---|---|
| `seL4_Wait` | 4 | `WaitFor` (12) | 通知等待 |
| `seL4_Signal` | 10 | `Notify` (11) | 通知发送 |

### 2.3 调度类

| seL4 syscall | 编号 | 自研内核 SyscallNum | 说明 |
|---|---|---|---|
| `seL4_Yield` | 5 | `Yield` (0) | 让出时间片 |

### 2.4 能力类

| seL4 syscall | 编号 | 自研内核 SyscallNum | 说明 |
|---|---|---|---|
| `seL4_Mint` | 1 | `CapInsert` (8) | 能力派生（badge + rights 缩减） |
| `seL4_Move` | 7 | `CapMove` (10) | 能力移动 |
| `seL4_Copy` | 8 | `CapInsert` (8) | 能力复制 |
| `seL4_Delete` | 9 | `CapDelete` (9) | 能力删除 |

### 2.5 线程类

| seL4 syscall | 编号 | 自研内核 SyscallNum | 说明 |
|---|---|---|---|
| `seL4_TCB_Configure` | 16 | `ThreadCreate` (1) | 聚合 Retype+Configure |
| `seL4_TCB_SetPriority` | 17 | `TcbConfigure` (14) | 优先级设置 |
| `seL4_TCB_Resume` | 18 | `ThreadCreate` (1) | 恢复（骨架降级） |
| `seL4_TCB_Suspend` | 19 | `ThreadDelete` (2) | 挂起（骨架降级） |

### 2.6 内存类（兼容层跳过）

| seL4 syscall | 编号 | 处理方式 | 说明 |
|---|---|---|---|
| `seL4_Retype` | 15 | 跳过（返回 Ok(0)） | 内核自动管理 |
| `seL4_Untyped_Retype` | 20 | 跳过（返回 Ok(0)） | 内核自动管理 |

---

## 3. 数据类型映射

### 3.1 MessageInfo

| seL4 字段 | 兼容层字段 | 类型 | 说明 |
|---|---|---|---|
| `label` | `label` | `u64` | 消息标签 |
| `capsUnwrapped` | `caps_unwrapped` | `u8` | 解包能力数 |
| `extraCaps` | `extra_caps` | `u8` | 额外能力数 |
| `length` | `length` | `u16` | 消息长度（字节） |

序列化格式（u64 寄存器传递）：`[63:32] label | [31:16] length | [15:8] extra_caps | [7:0] caps_unwrapped`

### 3.2 Rights 权限映射

| seL4 Rights | 值 | 自研内核 CapRights | 值 |
|---|---|---|---|
| `None` | 0 | 无权限 | 0 |
| `Read` | 1 | `READ` | 1 |
| `Write` | 2 | `WRITE` | 2 |
| `Grant` | 4 | `GRANT` | 8 |
| `ReadWrite` | 3 | `READ \| WRITE` | 3 |
| `All` | 7 | `READ \| WRITE \| GRANT` | 11 |

### 3.3 CSpace 映射

- **seL4**：树形 CSpace（CNode 多级寻址），64 位 CPtr
- **自研内核**：扁平能力表（分区内 4096 槽位），32 位 CapSlot
- **映射器**：`CSpaceMapper` 维护 CPtr → CapSlot 定长映射表（cap=256）

---

## 4. 错误码映射

| 兼容层错误 | 值 | 语义 | 用户态处理 |
|---|---|---|---|
| `UnsupportedSyscall` | 1 | ENOSYS | 降级/忽略 |
| `InvalidCapability` | 2 | EFAULT | 错误处理路径 |
| `TranslationFailed` | 3 | 内部错误 | 不应发生 |
| `Timeout` | 4 | ETIMEDOUT | 重试/降级 |

---

## 5. 模块结构

```
crates/kernel/microkernel/src/compat/
├── mod.rs          # 模块根：Sel4CompatLayer trait（v0.155.0 契约保留）+ 重导出
├── types.rs        # 数据类型：MessageInfo/CPtr/Rights/CompatError + sel4_syscall_nums
├── translation.rs  # SyscallTranslator：20 条翻译表 + 参数/返回值翻译
├── cspace.rs       # CSpaceMapper：CPtr ↔ CapSlot 定长 256 映射
└── shim.rs         # Sel4Compat 主结构体 + Sel4Shim trait + KernelSyscallOps 注入
```

---

## 6. 使用方式

### 6.1 Feature Flag

```toml
# Cargo.toml
[features]
sel4-compat = []
```

### 6.2 集成层接线

```rust
use eneros_microkernel::compat::{Sel4Compat, KernelSyscallOps};

// 实现 KernelSyscallOps trait（注入真实内核操作）
struct RealKernelOps { /* ... */ }
impl KernelSyscallOps for RealKernelOps { /* ... */ }

// 创建兼容层实例
let mut ops = RealKernelOps::new();
let mut compat = Sel4Compat::new(&mut ops);

// 预置 CSpace 映射（启动时从 seL4 CSpace 导入）
compat.cspace_mut().insert_at(sel4_cptr, kernel_cap_slot);

// 处理 seL4 风格 syscall
let result = compat.handle_syscall(sel4_syscall_num, &args);
```

---

## 7. 已知限制与坑点

1. **seL4 MR 机制**：seL4 使用消息寄存器（MR）传递数据，自研内核使用连续缓冲区。当前翻译层为直通模式，MR ↔ buffer 转换留待集成层完成。

2. **CNode Copy/Delete**：seL4 的 CNode Copy/Delete 在自研内核中无直接等价物，用 CapInsert/CapDelete 模拟。

3. **Untyped 重类型化**：seL4 的 Untyped Retype 在自研内核中由内核自动管理，兼容层跳过。

4. **非阻塞语义降级**：seL4 的 NBSend/NBRecv 降级为阻塞式 Send/Recv（骨架阶段）。

---

## 8. 测试覆盖

| 测试文件 | 测试数 | 覆盖范围 |
|---|---|---|
| `src/compat/types.rs` (内嵌) | 8 | MessageInfo 序列化/Rights 映射/CompatError |
| `src/compat/translation.rs` (内嵌) | 12 | 翻译表正确性/参数翻译/返回值映射 |
| `src/compat/cspace.rs` (内嵌) | 10 | 分配/解析/释放/表满/复用 |
| `src/compat/shim.rs` (内嵌) | 14 | 全 syscall handler/Sel4Shim trait/统计 |
| `src/compat/mod.rs` (内嵌) | 3 | Sel4CompatLayer 契约向后兼容 |
| `tests/compat_test.rs` (集成) | 12 | F1~F12 端到端验证 |
| **合计** | **59** | |

---

## 9. 性能指标

| 指标 | 目标 | 说明 |
|---|---|---|
| 兼容层开销 | < 1μs | 单次 syscall 翻译 |
| IPC 延迟增量 | < 2μs | 经兼容层的 IPC 往返 |

---

## 10. 后续版本

- **v0.157.0**：seL4 深度定制后全功能回归验证（★ P3-G 终版本）
- **v0.158.0**：SM2 签名接入（能力令牌签名验证）
