# Tasks — v0.18.0 线程/任务抽象

- [x] Task 1: 升级 sched crate 版本号与构建配置
  - [x] SubTask 1.1: 修改 `crates/kernel/sched/Cargo.toml`，版本 `0.16.0` → `0.18.0`
  - [x] SubTask 1.2: 修改根 `Cargo.toml`，workspace `version` `0.17.0` → `0.18.0`
  - [x] SubTask 1.3: 修改 `Makefile`，`VERSION` `0.17.0` → `0.18.0`
  - [x] SubTask 1.4: 修改 `.github/workflows/ci.yml`，版本标识更新为 v0.18.0
  - [x] SubTask 1.5: 修改 `ci/src/gate.rs`，注释追加 v0.18.0 说明

- [x] Task 2: 实现 `crates/kernel/sched/src/tcb.rs`（TCB 与状态机）
  - [x] SubTask 2.1: 在 `lib.rs` 顶部添加 `extern crate alloc;`，声明 `pub mod tcb;`
  - [x] SubTask 2.2: 实现 `ThreadState` 枚举（`Ready`/`Running`/`Blocked`/`Suspended`/`Dead`），derive `Clone, Copy, PartialEq, Debug`
  - [x] SubTask 2.3: 实现 `Tcb` 结构体（`tid`/`state`/`priority`/`stack`/`stack_top`/`stack_size`/`sp`/`pc`/`entry`/`partition` 字段），复用现有 `Tid` 类型（不重定义）
  - [x] SubTask 2.4: 实现 `Tcb::new(tid, entry, stack, size, priority)` 构造函数，调用 `init_stack_frame` 初始化栈帧
  - [x] SubTask 2.5: 实现 `Tcb::transition(&mut self, next) -> Result<(), &'static str>` 状态机转换，合法转换集合：`Ready↔Running`、`Running→Blocked`、`Blocked→Ready`、`Ready↔Suspended`、`Running→Dead`、`Ready→Dead`
  - [x] SubTask 2.6: 实现 `init_stack_frame(stack_top, entry) -> u64`，aarch64 侧 272 字节栈帧（31 寄存器 + spsr + elr），host 侧 cfg gate stub
  - [x] SubTask 2.7: 编写单元测试：合法转换、非法转换、栈帧布局（aarch64 only）、构造函数初始化

- [x] Task 3: 实现 `crates/kernel/sched/src/switch.rs`（上下文切换）
  - [x] SubTask 3.1: 实现 `context_switch(from_sp, to_sp)` naked 函数，`#[cfg(target_arch = "aarch64")]` gate，保存/恢复 x19-x30 callee-saved 寄存器，`extern "C"` ABI（注：nightly-2026-04-04 需用 `#[unsafe(naked)]` + `naked_asm!`，非 `#[naked]` + `asm!`）
  - [x] SubTask 3.2: 提供 host 侧 `#[cfg(not(target_arch = "aarch64"))]` stub（panic 或 no-op）以保证 crate 在 host 编译通过
  - [x] SubTask 3.3: 实现 `thread_switch(from: &mut Tcb, to: &Tcb)` 包装函数，调用 `context_switch`
  - [x] SubTask 3.4: 在 `lib.rs` 添加 `pub mod switch;` 与 `pub use switch::thread_switch`

- [x] Task 4: 实现 `crates/kernel/sched/src/priority.rs`（优先级调度）
  - [x] SubTask 4.1: 实现 `select_next_by_priority(tids: &[Tid], get_prio: impl Fn(Tid) -> u8) -> Option<Tid>`，选 `priority` 最小者，同优先级 FIFO
  - [x] SubTask 4.2: 实现 `PriorityQueue` 简单结构：基于固定数组，`push(tid, prio)`/`pop() -> Option<Tid>`（使用 Tid(0) 哨兵标记空槽保持 FIFO）
  - [x] SubTask 4.3: 在 `lib.rs` 添加 `pub mod priority;` 与 `pub use`
  - [x] SubTask 4.4: 编写单元测试：选最高优先级、同优先级 FIFO、空队列、单元素

- [x] Task 5: 实现全局线程管理 API（在 `tcb.rs`）
  - [x] SubTask 5.1: 定义全局线程表 `static THREAD_TABLE: ThreadTable`（`Spinlock` + `UnsafeCell<[Option<Box<Tcb>>; MAX_THREADS]>` + `unsafe impl Sync`），复用 `percore::Spinlock`，const 初始化
  - [x] SubTask 5.2: 实现 `thread_create(entry, stack_size, priority) -> Tid`：分配 Box<Tcb> + 栈（`alloc::alloc::alloc`），找空槽插入，返回 Tid；分配失败返回 `Tid(0)`
  - [x] SubTask 5.3: 实现 `thread_destroy(tid)`：状态非 `Running` 时置 `Dead` 并回收栈；`Running` 时返回错误
  - [x] SubTask 5.4: 实现 `thread_block(tid)`：`transition(Blocked)`，从运行队列移除
  - [x] SubTask 5.5: 实现 `thread_resume(tid)`：`transition(Ready)`，加入运行队列
  - [x] SubTask 5.6: 实现 `thread_exit(tid) -> !`：置 `Dead`，回收栈，切换到下一个线程
  - [x] SubTask 5.7: 实现 `thread_yield()`：当前线程 `Ready`，调用 `thread_switch` 到下一个
  - [x] SubTask 5.8: 实现 `thread_state(tid) -> ThreadState`：查表返回状态
  - [x] SubTask 5.9: 编写单元测试：创建/销毁/阻塞/唤醒/状态查询，使用 `TEST_LOCK` 串行化保护全局表

- [x] Task 6: 创建文档（由并行 sub-agent 完成）
  - [x] SubTask 6.1: 创建 `docs/smp/thread-abstraction-design.md`（590 行，线程抽象设计）：TCB 结构、状态机、生命周期 API、与 v0.16.0 调度器的关系
  - [x] SubTask 6.2: 创建 `docs/smp/context-switch-guide.md`（491 行，上下文切换说明）：ARM64 naked 函数、栈帧布局、寄存器保存约定、eret 匹配、内存屏障（依赖 v0.17.0）

- [x] Task 7: 验证与回归
  - [x] SubTask 7.1: `cargo fmt --all -- --check`
  - [x] SubTask 7.2: `cargo clippy -p eneros-sched --all-targets -- -D warnings`
  - [x] SubTask 7.3: `cargo test -p eneros-sched`（含新增 tcb/switch/priority 测试 + v0.16.0 回归）
  - [x] SubTask 7.4: `cargo build -p eneros-sched --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`
  - [x] SubTask 7.5: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`（v0.17.0 一致性不退化）
  - [x] SubTask 7.6: `git status` 确认无垃圾文件（无 `target/`、`*.elf`、`*.bin`）

# Task Dependencies

- Task 2/3/4 可与 Task 1 并行（版本号独立）
- Task 5 依赖 Task 2（Tcb 类型）与 Task 3（thread_switch）
- Task 6 依赖 Task 2/3/4/5 完成（文档描述实现）
- Task 7 依赖所有前序任务完成

# Notes

- Task 6（文档）由并行 sub-agent 创建，已确认两份文档存在
- Task 3.1 实现细节：nightly-2026-04-04 要求 `#[unsafe(naked)]` 替代 `#[naked]`，`naked_asm!` 替代 `asm!`（不含寄存器操作数），直接引用 x0/x1 寄存器传递参数
- aarch64 限制：`str sp, [Xn]` 和 `ldr sp, [Xn]` 均非法，需通过中间寄存器（x2）中转
- 测试总数：72（v0.16.0 原有 52 + v0.18.0 新增 20：tcb 11 + switch 2 + priority 7）
- workspace 回归：296 个测试全通过
