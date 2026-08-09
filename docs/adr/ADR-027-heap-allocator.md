# ADR-027 — 可回收堆分配器选型：linked_list_allocator（22 W1.4）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W1.4 预设裁决：vetted 优先，不自研）
- **关联**：ADR-025（alloc_error_handler）、`蓝图/22` §2.W1.4

## 背景

22 §2.W1.4：`services/allocator.rs` 与 `root-task/allocator.rs` 的 bump
分配器 `dealloc` 空实现（64 KiB/512 KiB 堆永不回收）——长跑服务堆占用
单调增长，必然 OOM-abort（W1.1 之前是 abort，现在走 alloc_error_handler
挂起，但堆不可用仍是功能失效）。要求替换为可回收分配器 + 堆大小配置注入。

## 决策

1. **引入 `linked_list_allocator` v0.10**（预设裁决确认）：vetted（bootloader
   生态标准分配器）、no_std、内部自旋锁并发安全、`used()` 指标、
   GlobalAlloc 完整实现。不自研 free-list（自研分配器是 bug 重灾区，
   违反"vetted 优先"）。
2. **API**：`HeapAllocator` 包装 `LockedHeap`（静态 `UnsafeCell<[u8; HEAP_SIZE]>`
   存储 + `unsafe fn init()` 首分配前调用）。bin `_start` / `rust_entry`
   在第一个分配点之前 `init()`（`println`/`seL4_put_char`/`boot_init`
   均无分配，安全）。
3. **堆大小配置注入**：编译期 `ENEROS_HEAP_SIZE_BYTES` 环境变量（const fn
   十进制解析；非法/过小回退默认：services 64 KiB、root-task 512 KiB）。
   真机按板载 RAM 构建时注入，不改代码。
4. **BumpAllocator 保留**：供 loom 并发模型验证（`feature = "loom"`）与
   参考；生产 `#[global_allocator]` 全部切换 `HeapAllocator`。
5. **可执行证据**：
   - 长跑测试：1000 轮分配/释放后 `used() ≤ base + 1KiB`（bump 必失败）；
     200 轮交错大小（16B-4KiB）后 `used < capacity`；
   - 全仓 test / clippy / fmt / aarch64 target 构建绿。

## 影响

- 服务与 root-task 长跑堆占用稳定（回收真实）；OOM 从"必然"变"异常路径"
  （配合 W1.1 的 handler 挂起 + W1.2 复位）。
- 新依赖 `linked_list_allocator 0.10` + `spinning_top 0.2`（transitive，
  spin 已有先例）——已过 deny.toml 审查（无安全 advisory；`cargo deny`
  在 CI 全绿后确认）。

## 备选方案

- **自研 free-list/buddy**：预设裁决否决（bug 重灾区）。
- **bump + 定期重置**：服务生命周期内无法安全重置（指针泄漏语义），否决。
- **talc / embedded-alloc 等**：功能相当但生态成熟度低于
  linked_list_allocator（bootloader 事实标准），不引入额外选择。
