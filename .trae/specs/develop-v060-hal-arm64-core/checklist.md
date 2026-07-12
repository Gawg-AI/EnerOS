# Checklist — EnerOS v0.6.0 HAL ARM64 核心实现

> **变更ID**：develop-v060-hal-arm64-core
> **蓝图依据**：`蓝图/phase0.md` §v0.6.0（第 1040–1276 行）

---

## 一、模块骨架与 cfg 门控

- [x] `hal/src/arm64/mod.rs` 存在，含 `pub mod cpu/gicv3/timer/provider`
- [x] `hal/src/lib.rs` 含 `#[cfg(target_arch = "aarch64")] pub mod arm64;`
- [x] Host 构建（x86_64）arm64 模块被 cfg 排除，`cargo build -p eneros-hal` 成功
- [x] v0.5.0 的 mock 模块不受影响，`cargo build -p eneros-hal --features mock` 成功

## 二、Arm64Cpu 实现（cpu.rs）

- [x] `Arm64Cpu` 结构体定义
- [x] `CORE_COUNT` 编译期常量（默认 4）
- [x] `enable_irq()` 使用 `msr daifclr, #0xf`
- [x] `disable_irq()` 使用 `msr daifset, #0xf`
- [x] `current_core()` 读 `mpidr_el1` 返回 Aff0（低 8 位）
- [x] `core_count()` 返回 CORE_COUNT
- [x] `halt()` 循环调用 `wfi()` 永不返回（`-> !`）
- [x] `wfi()` 使用 `wfi` 指令
- [x] `static ARM64_CPU` 单例 + `pub fn cpu() -> &'static dyn HalCpu`
- [x] 所有 inline asm 使用 `core::arch::asm!` + `options(nostack, preserves_flags)` 如适用

## 三、Arm64Gic 实现（gicv3.rs）

### 寄存器常量
- [x] GICD 偏移常量：GICD_CTLR=0x00, GICD_ISENABLER=0x100, GICD_ICENABLER=0x180, GICD_ICPENDR=0x280, GICD_PRI=0x400
- [x] GICR 偏移常量：GICR_CTLR=0x00, GICR_WAKER=0x14, GICR_TYPER=0x08, GICR_IGROUPR0=0x100
- [x] WAKER 位常量：PROCESSING_SLEEP=2, CHILDREN_ASLEEP=4
- [x] `MAX_IRQ` = 256

### 结构体与状态
- [x] `Arm64Gic` 结构体（gicd_base/gicr_base 字段）
- [x] `static mut IRQ_HANDLERS: [Option<IrqHandler>; MAX_IRQ] = [None; MAX_IRQ];`
- [x] MMIO 辅助 `unsafe fn w32/r32`

### 初始化
- [x] `init()` 使能 GICD_CTLR（ARE_NS=1, EnableGrp1=1）
- [x] `init_redistributor()` 清除 WAKER.ProcessorSleep
- [x] `init_redistributor()` 轮询 WAKER.ChildrenAsleep 清零
- [x] CPU interface 通过 ICC_IGRPEN1_EL1 使能（系统寄存器模式，非 GICC 内存映射）

### HalIrq 实现
- [x] `register()` 校验 irq < MAX_IRQ，写入 IRQ_HANDLERS（unsafe）
- [x] `register()` 超范围返回 `Err(HalError::InvalidParam)`
- [x] `unregister()` 清除 IRQ_HANDLERS[slot]（unsafe）
- [x] `enable()` 写 GICD_ISENABLER（1<<(irq%32)）
- [x] `disable()` 写 GICD_ICENABLER（1<<(irq%32)）
- [x] `eoi()` 使用 `msr icc_eoir1_el1, irq`（GICv3 系统寄存器）
- [x] `dispatch_irq()` 读 ICC_IAR1_EL1 获取 IRQ ID
- [x] `dispatch_irq()` 查 handler 表，调用 handler，EOI
- [x] `dispatch_irq()` 未知中断号打印告警并 EOI 丢弃
- [x] `static ARM64_GIC` 单例 + `pub fn irq() -> &'static dyn HalIrq`

### GICv3 合规性（蓝图 §43.2 修复）
- [x] 使用 ICC_*_EL1 系统寄存器（非 GICv2 的 GICC_* 内存映射）
- [x] 包含 GICR Redistributor 初始化（非 v1.0 遗漏）
- [x] GICD_CTLR.ARE_NS 置位（启用亲和性路由）

## 四、Arm64Timer 实现（timer.rs）

- [x] `Arm64Timer` 结构体定义
- [x] `now_ns()` 读 `cntpct_el0`，乘 1e9 / frequency 转纳秒
- [x] `frequency_hz()` 读 `cntfrq_el0`
- [x] `set_deadline(ns)` 计算 tick 数，写 CNTP_TVAL_EL0，使能 CNTP_CTL_EL1
- [x] `static ARM64_TIMER` 单例 + `pub fn clock() -> &'static dyn HalClock`

## 五、Arm64HalCoreProvider 实现（provider.rs）

- [x] `Arm64HalCoreProvider` 结构体定义
- [x] `HalProvider::cpu()` 返回 `&ARM64_CPU`
- [x] `HalProvider::irq()` 返回 `&ARM64_GIC`
- [x] `HalProvider::clock()` 返回 `&ARM64_TIMER`
- [x] `HalProvider::mem()/serial()/gpio()` panic 或返回 NotSupported
- [x] `static ARM64_HAL_CORE` 单例 + `pub fn core_provider() -> &'static dyn HalProvider`

## 六、单元测试

- [x] gicv3.rs `#[cfg(test)]` 测试：GICD/GICR 寄存器偏移常量正确性
- [x] gicv3.rs 测试：WAKER 位常量正确性
- [x] gicv3.rs 测试：MAX_IRQ == 256
- [x] cpu.rs `#[cfg(test)]` 测试：CORE_COUNT 常量
- [x] `cargo test -p eneros-hal` 通过（默认 feature，host 仅 types 测试，arm64 模块被 cfg 排除）
- [x] `cargo test -p eneros-hal --features mock` 通过（v0.5.0 mock 回归，23 个测试）

## 七、no_std 合规

- [x] arm64 模块代码不使用 `std::*`
- [x] inline asm 使用 `core::arch::asm!`
- [x] MMIO 使用 `core::ptr::read_volatile`/`write_volatile`
- [x] 交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 成功

## 八、CI / Makefile

- [x] `.github/workflows/ci.yml` 版本标识为 v0.6.0
- [x] `Makefile` VERSION = 0.6.0
- [x] `ci/src/gate.rs` 注释更新
- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（40 个测试）
- [x] `cargo deny check licenses bans sources` 通过（advisories 因网络问题跳过，非代码问题）

## 九、交叉编译验证

- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-board --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hal --target aarch64-unknown-none` 成功（含 arm64 模块）

## 十、文档交付

- [x] `docs/gicv3-driver-guide.md` 存在（553 行）
- [x] 《GICv3 驱动说明》含 GICD/GICR/ICC 寄存器表
- [x] 《GICv3 驱动说明》含初始化序列
- [x] 《GICv3 驱动说明》含中断分发流程
- [x] 《GICv3 驱动说明》含与 GICv2 差异
- [x] `docs/arm-generic-timer-usage.md` 存在（448 行）
- [x] 《ARM Generic Timer 使用》含寄存器表
- [x] 《ARM Generic Timer 使用》含纳秒转换公式
- [x] 《ARM Generic Timer 使用》含定时器中断配置

## 十一、工作区整洁

- [x] `git status` 无 target/ build/ *.elf *.bin *.img *.dtb 被追踪
- [x] 无 IDE 缓存被追踪

## 十二、蓝图合规性

- [x] 蓝图 §43.1：所有 Rust 代码 no_std
- [x] 蓝图 §43.2：非瓶颈版本，签名可编译
- [x] 蓝图 §43.2 修复：GICv3 使用 ICC 系统寄存器（非 GICv2 GICC 内存映射）
- [x] 蓝图 §43.2 修复：包含 GICR Redistributor 初始化
- [x] 蓝图 §7.1：定时器中断可配置（set_deadline）
- [x] 蓝图 §7.2：now_ns() 单调递增（cntpct_el0 物理计数器）
- [x] 蓝图 §7.4：文档齐全
- [x] 蓝图 §7.5：HAL 核心就绪（出口判定）
- [x] 蓝图 §8.5（v0.5.0）：无 async fn
- [x] v0.5.0 mock 回归兼容
