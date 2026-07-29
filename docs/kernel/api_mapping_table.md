# seL4 → 自研内核 API 映射表

> **版本**：v0.156.0
> **用途**：seL4 syscall 与自研内核 syscall 的完整映射参考

---

## 1. Syscall 编号映射

| # | seL4 syscall | seL4 编号 | 自研内核 SyscallNum | 内核编号 | 翻译策略 |
|---|---|---|---|---|---|
| 1 | `seL4_Mint` | 1 | `CapInsert` | 8 | 派生新能力插入目标槽 |
| 2 | `seL4_Send` | 2 | `IpcSend` | 3 | 直通 |
| 3 | `seL4_Recv` | 3 | `IpcRecv` | 4 | 直通 |
| 4 | `seL4_Wait` | 4 | `WaitFor` | 12 | 直通 |
| 5 | `seL4_Yield` | 5 | `Yield` | 0 | 直通 |
| 6 | `seL4_Call` | 6 | `IpcCall` | 5 | 直通 |
| 7 | `seL4_Move` | 7 | `CapMove` | 10 | 直通 |
| 8 | `seL4_Copy` | 8 | `CapInsert` | 8 | 副本插入目标槽 |
| 9 | `seL4_Delete` | 9 | `CapDelete` | 9 | 直通 |
| 10 | `seL4_Signal` | 10 | `Notify` | 11 | 直通 |
| 11 | `seL4_NBSend` | 11 | `IpcSend` | 3 | 降级为阻塞式 |
| 12 | `seL4_NBRecv` | 12 | `IpcRecv` | 4 | 降级为阻塞式 |
| 13 | `seL4_Reply` | 13 | `IpcSend` | 3 | 降级为 Send |
| 14 | `seL4_ReplyRecv` | 14 | `IpcRecv` | 4 | 降级为 Recv |
| 15 | `seL4_Retype` | 15 | `MapPages` | 6 | 骨架映射（兼容层跳过） |
| 16 | `seL4_TCB_Configure` | 16 | `ThreadCreate` | 1 | 聚合 Retype+Configure |
| 17 | `seL4_TCB_SetPriority` | 17 | `TcbConfigure` | 14 | 直通 |
| 18 | `seL4_TCB_Resume` | 18 | `ThreadCreate` | 1 | 骨架降级 |
| 19 | `seL4_TCB_Suspend` | 19 | `ThreadDelete` | 2 | 骨架降级 |
| 20 | `seL4_Untyped_Retype` | 20 | `MapPages` | 6 | 骨架映射（兼容层跳过） |

---

## 2. 聚合语义说明

### 2.1 线程创建聚合

seL4 创建线程需要两步：
1. `seL4_Untyped_Retype`：从 Untyped 对象重类型化为 TCB
2. `seL4_TCB_Configure`：配置 TCB（入口/栈/优先级）

自研内核单 syscall 聚合：
- `ThreadCreate`：单 syscall 完成 TCB 分配 + 配置

兼容层将 seL4 两步调用序列翻译为单 `ThreadCreate` syscall。

### 2.2 Retype 跳过

seL4 的 `seL4_Retype` / `seL4_Untyped_Retype` 在自研内核中由内核自动管理内存对象生命周期，兼容层直接跳过（返回 Ok(0)）。

---

## 3. 能力空间映射

| seL4 概念 | 自研内核概念 | 映射方式 |
|---|---|---|
| CSpace（树形） | CapSpace（扁平） | CSpaceMapper 定长表 |
| CPtr（64 位路径） | CapSlot（32 位下标） | 双向映射 |
| CNode（容器节点） | 分区能力表 | 每分区 4096 槽 |
| CDT（派生树） | parent 反向链接 | mint/derive 建边 |

---

## 4. 权限位映射

| seL4 权限 | 位值 | 自研内核权限 | 位值 | 说明 |
|---|---|---|---|---|
| `seL4_NoRights` | 0 | 无 | 0 | |
| `seL4_CanRead` | 1 | `READ` | 1 | |
| `seL4_CanWrite` | 2 | `WRITE` | 2 | |
| `seL4_CanGrant` | 4 | `GRANT` | 8 | 位值不同 |
| — | — | `INVOKE` | 4 | seL4 无对应 |
| — | — | `DERIVE` | 16 | seL4 无对应 |

---

## 5. 错误码映射

| seL4 错误 | 兼容层错误 | 自研内核 KernelError | 说明 |
|---|---|---|---|
| `seL4_InvalidArgument` | `TranslationFailed` | `InvalidArgument` (1) | |
| `seL4_InvalidCapability` | `InvalidCapability` | `InvalidCapability` (3) | |
| `seL4_IllegalOperation` | `UnsupportedSyscall` | `NotSupported` (8) | |
| `seL4_DeleteFirst` | — | `BadState` (7) | |
| `seL4_FailedLookup` | `InvalidCapability` | `InvalidCapability` (3) | |
| `seL4_TruncatedMessage` | `TranslationFailed` | — | MR 截断 |
| — | `Timeout` | `Timeout` (6) | |

---

## 6. 未覆盖的 seL4 API（Phase 1-2 未使用）

以下 seL4 API 在 Phase 1-2 用户态代码中未使用，兼容层不覆盖：

- `seL4_IRQControl_Get`：中断控制
- `seL4_IRQHandler_Ack`：中断确认
- `seL4_IRQHandler_SetNotification`：中断绑定通知
- `seL4_ASIDPool_Assign`：ASID 池分配
- `seL4_Page_GetAddress`：页地址查询
- `seL4_PageTable_Map`：页表映射
- `seL4_PageDirectory_Map`：页目录映射
- `seL4_DebugPutChar`：调试输出
- `seL4_Benchmark*`：性能基准

如需扩展，在 `translation.rs` 的 `TRANSLATION_TABLE` 中追加条目即可。
