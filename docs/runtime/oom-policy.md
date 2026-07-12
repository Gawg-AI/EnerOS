# EnerOS 用户态堆 OOM 策略

> **版本**：v0.11.0
> **crate**：`eneros-user-heap`
> **蓝图依据**：`蓝图/phase0.md` §v0.11.0
> **最后更新**：2026-07-12

---

## 1. OOM 触发条件

用户态堆在以下两种情况下触发 OOM（Out-Of-Memory）：

### 1.1 配额超限（Quota Exceeded）

当分配请求的 `layout.size()` 会使 `quota.used` 超过 `quota.limit` 时：

```rust
// 当前 used = 8192，limit = 8192，请求 4096 字节
// 8192 + 4096 = 12288 > 8192 → OOM
quota.check(4096) == false
```

**注意**：配额超限不意味着底层 buddy 池已满——池可能有空间，但配额限制了使用。
这允许系统设计者通过 `set_quota()` 为不同用户进程分配不同的内存上限。

### 1.2 Buddy 池耗尽（Pool Exhausted）

当配额检查通过，但 `BuddyAllocator::alloc()` 返回 `null_mut` 时：

```rust
// 池大小 256KB，已分配 64 页（256KB），配额无限制
// 第 65 次单页分配 → buddy 无空闲页 → OOM
buddy.alloc(4096) == null_mut
```

这种情况发生在配额未设或配额 ≥ 池大小时，物理内存真正耗尽。

### 1.3 不会触发 OOM 的情况

- **堆未初始化**：`USER_HEAP` 为 `None` 时，`alloc` 返回 `null_mut`，**不触发 OOM**。
  这允许在堆初始化前的早期代码路径安全调用分配（返回 null 由调用方处理）。
- **dealloc**：`dealloc` 永远不触发 OOM，即使指针无效或堆未初始化（静默返回）。

---

## 2. OOM Handler 机制

### 2.1 类型定义

```rust
pub type OomHandler = Option<fn() -> !>;
```

- `Option`：`None` 表示使用默认行为（panic），`Some(f)` 表示调用 `f`
- `fn() -> !`：函数指针，返回类型 `!`（never type）表示必须发散
- `fn`（非 `Fn`）：只接受函数指针，不接受闭包，保证 `Sync` 安全

### 2.2 发散要求

`OomHandler` 的返回类型 `!` 强制 handler 必须以以下方式之一结束：

| 方式 | 示例 | 适用场景 |
|------|------|----------|
| `panic!` | `panic!("user heap OOM")` | 默认行为，开发/调试 |
| `loop {}` | `loop { core::hint::spin_loop(); }` | 嵌入式系统无 panic runtime |
| `core::intrinsics::abort()` | 直接终止 | 生产环境，立即停机 |
| 自定义终止 | 调用 seL4 syscall 终止线程 | 集成 OS 级错误恢复 |

**不能**返回 `()`——编译器会拒绝任何返回的 handler。

### 2.3 设置 Handler

```rust
pub fn set_oom_handler(handler: fn() -> !);
```

- 若堆未初始化，此调用是 no-op（handler 不会被保存）
- 再次调用会替换之前的 handler
- Handler 在 `USER_HEAP` 锁释放后调用，可在 handler 内安全访问 `used()` 等查询接口

---

## 3. 默认行为

未设置 handler 时（`oom_handler == None`），OOM 触发 `panic!`：

```rust
fn trigger_oom_handler(handler: OomHandler) {
    match handler {
        Some(f) => f(),
        None => panic!("user heap OOM"),
    }
}
```

### 3.1 Panic 语义

- **panic 消息**：`"user heap OOM"`
- **unwind 行为**：取决于 panic strategy（`panic = "unwind"` 或 `panic = "abort"`）
  - `unwind`：栈展开，`Drop` 实现 cleanup
  - `abort`：立即终止进程
- **`Mutex` 中毒**：由于 `trigger_oom_handler` 在 `drop(guard)` 后调用，
  `USER_HEAP` 的 `Mutex` **不会**因 panic 中毒

### 3.2 测试中的默认行为

集成测试使用 `std::panic::catch_unwind` 捕获 OOM panic：

```rust
let result = panic::catch_unwind(|| unsafe {
    GlobalAlloc::alloc(&heap, layout)
});
assert!(result.is_err(), "alloc beyond quota should trigger OOM panic");
```

---

## 4. 触发流程详解

### 4.1 alloc 中的 OOM 路径

```
UserHeap::alloc(layout)
│
├─ lock USER_HEAP
├─ inner = USER_HEAP.as_mut()
│
├─ if inner is None:
│   └─ return null_mut  // 不触发 OOM
│
├─ if !inner.quota.check(size):  // 配额超限
│   ├─ handler = inner.oom_handler
│   ├─ drop(guard)               // 释放锁
│   ├─ trigger_oom_handler(handler)
│   └─ return null_mut           // 若 handler 不发散（不应发生）
│
├─ ptr = inner.buddy.alloc(size)
│
├─ if ptr is null:               // buddy 池耗尽
│   ├─ handler = inner.oom_handler
│   ├─ drop(guard)
│   ├─ trigger_oom_handler(handler)
│   └─ return null_mut
│
├─ inner.quota.add_used(size)
├─ return ptr
```

### 4.2 手动触发

```rust
pub fn trigger_oom();
```

读取当前 `oom_handler` 并触发。用于：
- 测试验证 handler 设置
- 应用层主动触发清理（例如检测到内存压力时主动 OOM 以触发恢复流程）

---

## 5. 恢复策略

### 5.1 默认（无恢复）

默认 `panic!` 行为不提供恢复——进程终止或 unwind。适用于：
- 开发阶段：快速暴露内存泄漏
- 简单用户进程：失败即重启

### 5.2 自定义恢复示例

#### 示例 1：日志 + 终止

```rust
fn log_and_abort() -> ! {
    // 假设有串口输出接口
    println!("[FATAL] user heap OOM, aborting");
    loop { core::hint::spin_loop(); }  // 或调用 abort
}

eneros_user_heap::set_oom_handler(log_and_abort);
```

#### 示例 2：释放缓存后重试

```rust
// 注意：handler 必须发散，因此"重试"需在 handler 内 loop
fn reclaim_and_retry() -> ! {
    // 释放应用层缓存
    unsafe { reclaim_cache(); }
    // 此时重试分配可能成功，但 GlobalAlloc 不会自动重试
    // 因此仍需 panic 或 loop
    panic!("OOM after cache reclaim");
}
```

#### 示例 3：seL4 级终止

```rust
fn sel4_terminate() -> ! {
    // 调用 seL4 syscall 终止当前线程
    unsafe { sel4_sys::terminate_thread(); }
    loop {}  // 不可达
}
```

### 5.3 恢复策略选择指南

| 场景 | 推荐策略 | 理由 |
|------|----------|------|
| 开发/调试 | 默认 `panic!` | 快速暴露问题 |
| 关键服务 | `panic = "unwind"` + 重启 | 自动恢复 |
| 安全关键 | `abort` + 看门狗重启 | 确定性停机 |
| 容错系统 | 释放缓存 + 重试 | 延长运行时间 |

---

## 6. 与内核堆 OOM 的差异

| 特性 | 内核堆（v0.10.0） | 用户态堆（v0.11.0） |
|------|-------------------|---------------------|
| OOM 行为 | 返回 `null_mut`，调用方处理 | `OomHandler` 可定制 |
| 默认策略 | 无（调用方决定） | `panic!("user heap OOM")` |
| 可恢复性 | 取决于调用方 | 取决于 handler |
| 锁中毒风险 | 无（不在持锁时 panic） | 无（`drop(guard)` 后触发） |

**设计理由**：用户态进程通常无法像内核那样优雅处理 `null` 返回值（`Vec::push` 等
`alloc` crate API 不检查 null）。因此默认 `panic` 更安全，同时保留 handler 供
需要恢复的进程定制。

---

## 7. 测试覆盖

### 7.1 OOM 相关测试

| 测试 | 验证点 |
|------|--------|
| `test_user_heap_integration` 第 4 节 | 配额超限触发 panic |
| `test_user_heap_integration` 第 5 节 | buddy 耗尽触发 panic |
| `test_user_heap_integration` 第 6 节 | 自定义 handler 被调用且 panic 消息匹配 |
| `test_trigger_oom_default` | `trigger_oom()` 默认 panic |

### 7.2 测试方法

使用 `std::panic::catch_unwind` 捕获 panic：

```rust
let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
    GlobalAlloc::alloc(&heap, page)
}));
assert!(result.is_err());
```

验证自定义 handler：

```rust
let msg = result.as_ref().err().and_then(|e| {
    e.downcast_ref::<String>().map(|s| s.as_str())
        .or_else(|| e.downcast_ref::<&str>().copied())
});
assert_eq!(msg, Some("custom OOM handler invoked"));
```

---

## 8. 参考

- [用户态堆设计](user-heap-design.md) — 整体架构
- [内核堆设计](../kernel/kernel-heap-design.md) — v0.10.0 对比
- 蓝图 `phase0.md` §v0.11.0 — 版本规格
