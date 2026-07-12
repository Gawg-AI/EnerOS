# Checklist — v0.18.0 线程/任务抽象

## 代码实现

- [x] C1: `crates/kernel/sched/src/tcb.rs` 已创建，含 `ThreadState` 枚举（5 态）、`Tcb` 结构体（10 字段）、`transition()` 状态机、`init_stack_frame()` 栈帧初始化
- [x] C2: `crates/kernel/sched/src/switch.rs` 已创建，含 `context_switch` naked 函数（aarch64 内联汇编）与 `thread_switch` 包装
- [x] C3: `crates/kernel/sched/src/priority.rs` 已创建，含优先级选择 `select_next_by_priority` 或 `PriorityQueue`
- [x] C4: `Tid`/`CoreMask` 复用现有定义（v0.16.0 percore.rs/affinity.rs），未重定义
- [x] C5: `Tcb` 含裸指针（`stack`/`stack_top`），未自动 `impl Send/Sync`，访问通过 `Spinlock` 保护
- [x] C6: `lib.rs` 已添加 `extern crate alloc;`、`pub mod tcb/switch/priority`、`pub use` 导出
- [x] C7: 全局线程表 `THREAD_TABLE` 使用 `percore::Spinlock` 包裹 `[Option<Box<Tcb>>; MAX_THREADS]`，const 初始化

## API 完整性

- [x] C8: `thread_create(entry, stack_size, priority) -> Tid` 已实现，分配失败返回 `Tid(0)`
- [x] C9: `thread_destroy(tid)` 已实现，`Running` 线程拒绝销毁
- [x] C10: `thread_block(tid)` / `thread_resume(tid)` 已实现，调用 `transition()`
- [x] C11: `thread_exit(tid) -> !` 已实现，置 `Dead` 并切换
- [x] C12: `thread_yield()` 已实现，当前线程让出
- [x] C13: `thread_state(tid) -> ThreadState` 已实现

## ARM64 上下文切换正确性

- [x] C14: `context_switch` 使用 `#[unsafe(naked)]` + `extern "C"` ABI（蓝图 §8.5 强制 naked + extern "C"；nightly-2026-04-04 要求 `#[unsafe(naked)]` 替代 `#[naked]`）
- [x] C15: 保存/恢复 callee-saved 寄存器 x19-x30（蓝图 §4.5 代码）
- [x] C16: 栈帧大小 272 字节（31 寄存器 × 8 + spsr + elr）
- [x] C17: `init_stack_frame` 设置 `x30`/`elr_el1` 为入口地址，`spsr_el1 = 0x3C5`（启用 IRQ）
- [x] C18: aarch64 内联汇编用 `#[cfg(target_arch = "aarch64")]` gate，host 侧有 stub

## 状态机正确性

- [x] C19: 合法转换 `Ready↔Running`、`Running→Blocked`、`Blocked→Ready`、`Ready↔Suspended`、`Running→Dead`、`Ready→Dead` 返回 `Ok(())`
- [x] C20: 非法转换（如 `Dead→Running`、`Blocked→Running`）返回 `Err("invalid transition")`

## 测试覆盖

- [x] C21: `cargo test -p eneros-sched` 通过，新增 tcb/switch/priority 测试 ≥ 80% 覆盖（蓝图 §6.1）
- [x] C22: v0.16.0 原有 51 个测试不退化（回归通过）
- [x] C23: 状态机转换测试覆盖所有合法/非法路径
- [x] C24: 优先级调度测试覆盖：选最高优先级、同优先级 FIFO、空队列、单元素

## 构建与质量

- [x] C25: `crates/kernel/sched/Cargo.toml` 版本 `0.18.0`
- [x] C26: 根 `Cargo.toml` workspace 版本 `0.18.0`
- [x] C27: `Makefile` VERSION `0.18.0`
- [x] C28: `.github/workflows/ci.yml` 版本标识 v0.18.0
- [x] C29: `ci/src/gate.rs` 注释含 v0.18.0
- [x] C30: `cargo fmt --all -- --check` 通过
- [x] C31: `cargo clippy -p eneros-sched --all-targets -- -D warnings` 无 warning
- [x] C32: `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc` 通过
- [x] C33: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全通过（v0.17.0 一致性不退化）

## 文档与规范

- [x] C34: `docs/smp/thread-abstraction-design.md` 已创建（590 行，线程抽象设计）
- [x] C35: `docs/smp/context-switch-guide.md` 已创建（491 行，上下文切换说明）
- [x] C36: 文档放 `docs/smp/` 子目录，未平面化放 `docs/` 根（规则 §2.3.3）
- [x] C37: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C38: 新增 crate 源码在 `crates/kernel/sched/` 下（规则 §2.3.1）

## 验收标准（蓝图 §7）

- [x] C39: 线程可创建/切换/销毁（§7.1）
- [x] C40: 状态机转换正确（§7.2）
- [ ] C41: 单次切换 < 2μs（§7.3，QEMU 实机后验证，host 仅测 API 可用）— 延后至 QEMU 实机
- [x] C42: 文档齐全（§7.4）
- [x] C43: 出口判定：线程抽象就绪（§7.5）
