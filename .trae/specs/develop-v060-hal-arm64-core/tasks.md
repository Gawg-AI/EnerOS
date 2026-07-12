# Tasks — EnerOS v0.6.0 HAL ARM64 核心实现

> **变更ID**：develop-v060-hal-arm64-core
> **蓝图依据**：`蓝图/phase0.md` §v0.6.0（第 1040–1276 行）
> **原则**：非瓶颈版本，trait/struct 签名必须可编译（蓝图 §43.2）

---

# Task 1: 创建 arm64 模块骨架

在 eneros-hal crate 中创建 arm64 模块，cfg 门控。

- [x] SubTask 1.1: 创建 `hal/src/arm64/mod.rs`
  - 模块级文档注释
  - `pub mod cpu;`
  - `pub mod gicv3;`
  - `pub mod timer;`
  - `pub mod provider;`
- [x] SubTask 1.2: 修改 `hal/src/lib.rs`
  - 在 `pub use types::*;` 之后添加 `#[cfg(target_arch = "aarch64")] pub mod arm64;`
- [x] SubTask 1.3: 修改 workspace 根 `Cargo.toml`
  - version `0.5.0` → `0.6.0`
- [x] SubTask 1.4: 创建空文件 `hal/src/arm64/cpu.rs`、`gicv3.rs`、`timer.rs`、`provider.rs`（仅模块文档注释）
- [x] SubTask 1.5: 验证 `cargo build -p eneros-hal` 成功（host，arm64 被 cfg 排除）

---

# Task 2: 实现 Arm64Cpu（hal/src/arm64/cpu.rs）

实现 HalCpu trait，使用 ARM64 系统寄存器与指令。

- [x] SubTask 2.1: 定义 `Arm64Cpu` 结构体（无字段或含 `core_count: u32` 配置字段）
- [x] SubTask 2.2: 定义 `CORE_COUNT` 编译期常量（默认 4）
- [x] SubTask 2.3: 实现 `HalCpu::enable_irq()` — `msr daifclr, #0xf`
- [x] SubTask 2.4: 实现 `HalCpu::disable_irq()` — `msr daifset, #0xf`
- [x] SubTask 2.5: 实现 `HalCpu::current_core()` — 读 `mpidr_el1`，返回 `Aff0`（低 8 位）
- [x] SubTask 2.6: 实现 `HalCpu::core_count()` — 返回 CORE_COUNT
- [x] SubTask 2.7: 实现 `HalCpu::halt()` — `loop { self.wfi(); }`
- [x] SubTask 2.8: 实现 `HalCpu::wfi()` — `wfi` 指令
- [x] SubTask 2.9: 添加 `static ARM64_CPU: Arm64Cpu = Arm64Cpu;` 单例
- [x] SubTask 2.10: 添加 `pub fn cpu() -> &'static dyn HalCpu` 获取器

---

# Task 3: 实现 Arm64Gic（hal/src/arm64/gicv3.rs）

实现 HalIrq trait，基于 GICv3 架构（GICD + GICR + ICC 系统寄存器）。

- [x] SubTask 3.1: 定义 GICD 寄存器偏移常量（GICD_CTLR/GICD_ISENABLER/GICD_ICENABLER/GICD_ICPENDR/GICD_PRI）
- [x] SubTask 3.2: 定义 GICR 寄存器偏移常量（GICR_CTLR/GICR_WAKER/GICR_TYPER/GICR_IGROUPR0）
- [x] SubTask 3.3: 定义 GICR_WAKER 位常量（PROCESSING_SLEEP/CHILDREN_ASLEEP）
- [x] SubTask 3.4: 定义 `MAX_IRQ` 常量（256）
- [x] SubTask 3.5: 定义 `Arm64Gic` 结构体（gicd_base/gicr_base 字段）
- [x] SubTask 3.6: 定义 `static mut IRQ_HANDLERS: [Option<IrqHandler>; MAX_IRQ] = [None; MAX_IRQ];`
- [x] SubTask 3.7: 实现 `unsafe fn w32(base, off, v)` / `unsafe fn r32(base, off) -> u32` MMIO 辅助
- [x] SubTask 3.8: 实现 `Arm64Gic::init()` — GICD_CTLR 使能 + GICR 唤醒 + ICC 系统寄存器使能
- [x] SubTask 3.9: 实现 `Arm64Gic::init_redistributor()` — WAKER 清除 + 轮询 ChildrenAsleep
- [x] SubTask 3.10: 实现 `HalIrq::register()` — 校验 irq 范围，写入 IRQ_HANDLERS
- [x] SubTask 3.11: 实现 `HalIrq::unregister()` — 清除 IRQ_HANDLERS[slot]
- [x] SubTask 3.12: 实现 `HalIrq::enable()` — GICD_ISENABLER 写 1<<（irq%32）
- [x] SubTask 3.13: 实现 `HalIrq::disable()` — GICD_ICENABLER 写 1<<（irq%32）
- [x] SubTask 3.14: 实现 `HalIrq::eoi()` — `msr icc_eoir1_el1, irq`（系统寄存器模式）
- [x] SubTask 3.15: 实现 `Arm64Gic::dispatch_irq()` — 读 ICC_IAR1_EL1，查 handler 表，调用 handler，EOI
- [x] SubTask 3.16: 添加 `static ARM64_GIC: Arm64Gic` 单例 + `pub fn irq() -> &'static dyn HalIrq` 获取器

---

# Task 4: 实现 Arm64Timer（hal/src/arm64/timer.rs）

实现 HalClock trait，基于 ARMv8 Generic Timer。

- [x] SubTask 4.1: 定义 `Arm64Timer` 结构体（无字段或含 frequency 配置）
- [x] SubTask 4.2: 实现 `HalClock::now_ns()` — 读 `cntpct_el0`，乘 1e9 / frequency 转纳秒
- [x] SubTask 4.3: 实现 `HalClock::frequency_hz()` — 读 `cntfrq_el0`
- [x] SubTask 4.4: 实现 `HalClock::set_deadline(ns)` — 计算 tick 数，写 CNTP_TVAL_EL0，使能 CNTP_CTL_EL1
- [x] SubTask 4.5: 添加 `static ARM64_TIMER: Arm64Timer` 单例 + `pub fn clock() -> &'static dyn HalClock` 获取器

---

# Task 5: 实现 Arm64HalCoreProvider（hal/src/arm64/provider.rs）

部分实现 HalProvider，仅返回 cpu/irq/clock，其余返回 NotSupported 错误或 panic。

- [x] SubTask 5.1: 定义 `Arm64HalCoreProvider` 结构体
- [x] SubTask 5.2: 实现 `HalProvider::cpu()` 返回 `&ARM64_CPU`
- [x] SubTask 5.3: 实现 `HalProvider::irq()` 返回 `&ARM64_GIC`
- [x] SubTask 5.4: 实现 `HalProvider::clock()` 返回 `&ARM64_TIMER`
- [x] SubTask 5.5: 实现 `HalProvider::mem()/serial()/gpio()` — panic!("not implemented: v0.7.0")
- [x] SubTask 5.6: 添加 `static ARM64_HAL_CORE: Arm64HalCoreProvider` 单例
- [x] SubTask 5.7: 添加 `pub fn core_provider() -> &'static dyn HalProvider` 获取器

---

# Task 6: 编写单元测试

Host 端可测试的部分（寄存器常量、类型构造）；aarch64 代码通过交叉编译验证。

- [x] SubTask 6.1: 在 `gicv3.rs` 中添加 `#[cfg(test)] mod tests`（host 可测的常量验证）
  - GICD 寄存器偏移值正确性（GICD_CTLR==0x00, GICD_ISENABLER==0x100 等）
  - GICR 寄存器偏移值正确性
  - WAKER 位常量正确性
  - MAX_IRQ == 256
- [x] SubTask 6.2: 在 `cpu.rs` 中添加 `#[cfg(test)] mod tests`
  - CORE_COUNT 常量验证
- [x] SubTask 6.3: 验证 `cargo test -p eneros-hal --features mock` 通过（v0.5.0 mock 回归，23 个测试通过）
- [x] SubTask 6.4: 验证 `cargo test -p eneros-hal` 通过（默认 feature，host 常量测试在 arm64 模块内被 cfg 排除，host 仅运行 types 测试）

---

# Task 7: 集成到 CI / Makefile

更新版本号与构建配置。

- [x] SubTask 7.1: 修改 `.github/workflows/ci.yml` — 版本标识 v0.5.0 → v0.6.0
- [x] SubTask 7.2: 修改 `Makefile` — VERSION 0.5.0 → 0.6.0
- [x] SubTask 7.3: 修改 `ci/src/gate.rs` — 注释更新说明 v0.6.0 arm64 模块 cfg 门控
- [x] SubTask 7.4: 验证 `cargo fmt --all -- --check` 通过
- [x] SubTask 7.5: 验证 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 7.6: 验证 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（40 个测试）
- [x] SubTask 7.7: 验证交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 通过（含 arm64 模块）

---

# Task 8: 编写文档

交付两份技术文档。

- [x] SubTask 8.1: 创建 `docs/gicv3-driver-guide.md`《GICv3 驱动说明》（553 行）
  - GICv3 架构概述（GICD/GICR/CPU interface 三层）
  - GICD 寄存器表（CTLR/ISENABLER/ICENABLER/ICPENDR/PRI）
  - GICR 寄存器表（CTLR/WAKER/TYPER/IGROUPR0）
  - ICC 系统寄存器表（ICC_IAR1_EL1/ICC_EOIR1_EL1/ICC_IGRPEN1_EL1/ICC_PMR_EL1）
  - 初始化序列（GICD→GICR→CPU interface）
  - 中断分发流程（IAR 读取→handler 查找→EOI）
  - 与 GICv2 差异（Redistributor、系统寄存器、亲和性路由）
- [x] SubTask 8.2: 创建 `docs/arm-generic-timer-usage.md`《ARM Generic Timer 使用》（448 行）
  - Generic Timer 概述（物理/虚拟计时器）
  - 寄存器表（CNTFRQ_EL0/CNTPCT_EL0/CNTP_TVAL_EL0/CNTP_CVAL_EL0/CNTP_CTL_EL1）
  - 纳秒转换公式（ns = cntpct * 1e9 / cntfrq）
  - 定时器中断配置（TVAL/CVAL 写入 + CTL.ENABLE 使能）
  - QEMU virt 频率说明（默认 62.5MHz）

---

# Task 9: 验证与收尾

全量验证。

- [x] SubTask 9.1: `cargo fmt --all -- --check` — 通过
- [x] SubTask 9.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` — 通过
- [x] SubTask 9.3: `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` — 通过
- [x] SubTask 9.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` — 通过（40 个测试）
- [x] SubTask 9.5: `cargo test -p eneros-hal --features mock` — 通过（23 个测试）
- [x] SubTask 9.6: `cargo deny check advisories licenses bans sources` — licenses/bans/sources 通过；advisories 因网络无法连接 github.com 失败（环境问题，非代码问题）
- [x] SubTask 9.7: 交叉编译全部 crate 到 aarch64-unknown-none（kernel/runtime/board/sel4-sys/hello/hal）— 全部通过
- [x] SubTask 9.8: 确认 `git status` 无垃圾文件 — 通过（target/build/*.elf 等均被 .gitignore 覆盖）
- [x] SubTask 9.9: 更新 checklist.md

---

# Task Dependencies

- Task 2/3/4 依赖 Task 1（模块骨架）
- Task 5 依赖 Task 2/3/4（provider 引用 cpu/gic/timer 单例）
- Task 6 依赖 Task 5（测试）
- Task 7 依赖 Task 6（CI 集成）
- Task 8 可与 Task 5/6/7 并行（文档独立）
- Task 9 依赖全部前序
