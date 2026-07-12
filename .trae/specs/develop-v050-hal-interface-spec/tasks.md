# Tasks — EnerOS v0.5.0 HAL 接口规范设计

> **变更ID**：develop-v050-hal-interface-spec
> **蓝图依据**：`蓝图/phase0.md` §v0.5.0（第 792–1038 行）
> **原则**：纯设计版本，trait/struct 签名必须可编译（蓝图 §43.2 非瓶颈版本要求）

---

# Task 1: 创建 eneros-hal crate 骨架

创建 `hal/` 目录与 crate 基础结构，接入 workspace。

- [x] SubTask 1.1: 创建 `hal/Cargo.toml`
  - package name = `eneros-hal`，version.workspace / edition.workspace / authors.workspace / license.workspace
  - description = "EnerOS Hardware Abstraction Layer trait specifications (no_std)"
  - `[features] default = []` + `mock = []`
  - 无外部依赖（仅 core）
- [x] SubTask 1.2: 创建 `hal/src/lib.rs` 骨架
  - `#![no_std]`（实际用 `#![cfg_attr(not(test), no_std)]` 支持 host 测试）
  - `pub mod types;`
  - `pub use types::*;`
  - `pub mod mock;`（仅 `#[cfg(feature = "mock")]`）
  - 模块级文档注释说明 crate 用途
- [x] SubTask 1.3: 修改 workspace 根 `Cargo.toml`
  - members 增加 `"hal"`
  - version `0.4.0` → `0.5.0`
- [x] SubTask 1.4: 验证 `cargo build -p eneros-hal` 成功（空骨架可编译）

---

# Task 2: 实现公共类型 `hal/src/types.rs`

按蓝图 §4.1 定义所有公共类型。

- [x] SubTask 2.1: 定义 `MemFlags`（readable/writable/executable/device/cacheable，派生 Clone/Copy/Debug）
- [x] SubTask 2.2: 定义 `IrqTrigger`（Edge/Level，派生 Clone/Copy/Debug/PartialEq/Eq）
- [x] SubTask 2.3: 定义 `HalError`（InvalidParam/OutOfResource/NotSupported/HardwareFault/PermissionDenied，仅派生 Debug）+ `impl Display`
- [x] SubTask 2.4: 定义 `GpioDir`（Input/Output，派生 Clone/Copy/Debug/PartialEq/Eq）
- [x] SubTask 2.5: 定义 `PullMode`（None/Up/Down，派生 Clone/Copy/Debug/PartialEq/Eq）
- [x] SubTask 2.6: 定义 `GpioConfig`（pin: u32, dir: GpioDir, pull: PullMode，派生 Clone/Copy）
- [x] SubTask 2.7: 定义 `IrqAction`（Handled/WakeThread/Disabled，派生 Debug/PartialEq/Eq）
- [x] SubTask 2.8: 定义 `IrqHandler` 类型别名 `pub type IrqHandler = fn(irq: u32) -> IrqAction;`
- [x] SubTask 2.9: 为 `MemFlags` 添加便捷构造方法（`MemFlags::device()` / `MemFlags::normal()` / `MemFlags::code()`）
- [x] SubTask 2.10: 验证 `cargo build -p eneros-hal` 成功

---

# Task 3: 实现 6 个 HAL trait `hal/src/lib.rs`

按蓝图 §4.2 定义 6 个核心 trait，确保 dyn 安全。

- [x] SubTask 3.1: 定义 `HalCpu` trait（enable_irq/disable_irq/current_core/core_count/halt/wfi），每个方法带文档注释
- [x] SubTask 3.2: 定义 `HalMem` trait（map/unmap/translate/set_domain）
- [x] SubTask 3.3: 定义 `HalIrq` trait（register/unregister/enable/disable/eoi）
- [x] SubTask 3.4: 定义 `HalClock` trait（now_ns/frequency_hz/set_deadline）
- [x] SubTask 3.5: 定义 `HalSerial` trait（write/read/flush）
- [x] SubTask 3.6: 定义 `HalGpio` trait（set_dir/set/get/toggle）
- [x] SubTask 3.7: 验证所有 trait 方法无泛型参数、无 `Self` 返回（dyn 安全）
- [x] SubTask 3.8: 验证 `cargo build -p eneros-hal` 成功

---

# Task 4: 实现 HalProvider 注册器模式

按蓝图 §4.5 实现 BSP 注入点。

- [x] SubTask 4.1: 定义 `HalProvider` trait（cpu/mem/irq/clock/serial/gpio 六个方法返回 `&'static dyn HalXxx`）
- [x] SubTask 4.2: 定义 `static mut HAL: Option<&'static dyn HalProvider> = None;`
- [x] SubTask 4.3: 实现 `init_hal(provider: &'static dyn HalProvider)`（unsafe 块，赋值 HAL）
- [x] SubTask 4.4: 实现 `hal() -> &'static dyn HalProvider`（unsafe 块，expect "HAL not initialized"）
- [x] SubTask 4.5: 为 `static mut HAL` 添加 `#[allow(static_mut_refs)]`
- [x] SubTask 4.6: 验证 `cargo build -p eneros-hal` 成功

---

# Task 5: 实现 mock `hal/src/mock.rs`

按蓝图 §4.5 实现 mock，覆盖全部 6 个 trait，用于编译验证与单元测试。

- [x] SubTask 5.1: 定义 `MockHal` 结构体（无字段）
- [x] SubTask 5.2: 为 `MockHal` 实现 `HalCpu`（current_core 返回 0，core_count 返回 1，halt 用 `loop {}` + spin_loop，其余 no-op）
- [x] SubTask 5.3: 为 `MockHal` 实现 `HalMem`（map/unmap/set_domain 返回 Ok，translate 返回 None）
- [x] SubTask 5.4: 为 `MockHal` 实现 `HalIrq`（register/unregister 返回 Ok，enable/disable/eoi no-op）
- [x] SubTask 5.5: 为 `MockHal` 实现 `HalClock`（now_ns 返回 0，frequency_hz 返回 1000，set_deadline 返回 Ok）
- [x] SubTask 5.6: 为 `MockHal` 实现 `HalSerial`（write 返回 Ok(data.len())，read 返回 Ok(0)，flush 返回 Ok）
- [x] SubTask 5.7: 为 `MockHal` 实现 `HalGpio`（set_dir/set/toggle 返回 Ok，get 返回 Ok(false)）
- [x] SubTask 5.8: 实现 `MockHalProvider` 实现 `HalProvider`，返回 `&'static MockHal`
- [x] SubTask 5.9: 验证 `cargo build -p eneros-hal --features mock` 成功

---

# Task 6: 编写单元测试

在 `hal/src/types.rs` 和 `hal/src/mock.rs` 中添加 `#[cfg(test)] mod tests`。

- [x] SubTask 6.1: types.rs 测试：MemFlags 构造与字段访问、IrqTrigger 变体匹配、HalError 变体匹配（用 matches!）、GpioConfig 构造、IrqAction 变体（共 11 个测试）
- [x] SubTask 6.2: mock.rs 测试：MockHal 各 trait 方法调用返回预期值（current_core==0, core_count==1, now_ns==0, write 返回长度等）
- [x] SubTask 6.3: mock.rs 测试：MockHalProvider 通过 init_hal 注入后，hal().cpu().current_core() 返回 0
- [x] SubTask 6.4: mock.rs 测试：IrqHandler 函数指针类型可赋值与调用
- [x] SubTask 6.5: 验证 `cargo test -p eneros-hal --features mock` 全部通过（23 个测试）

---

# Task 7: 集成到 CI / Makefile / 质量门禁

更新构建系统与 CI 配置。

- [x] SubTask 7.1: 修改 `ci/src/gate.rs` —— 更新 clippy/test 注释说明 hal 为库 crate 可 host 测试
- [x] SubTask 7.2: 修改 `.github/workflows/ci.yml` —— 版本 v0.5.0 + cross-build hal 步骤 + test mock 步骤
- [x] SubTask 7.3: 修改 `Makefile` —— VERSION 0.5.0 + hal-build/hal-test 目标 + help 文本
- [x] SubTask 7.4: 验证本地 `cargo fmt --all -- --check` 通过
- [x] SubTask 7.5: 验证本地 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 7.6: 验证本地 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [x] SubTask 7.7: 验证交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 通过

---

# Task 8: 编写文档

交付两份技术文档。

- [x] SubTask 8.1: 创建 `docs/hal-interface-spec.md`《HAL 接口规范》（461 行，覆盖 8 章 + 全部 trait 方法契约）
- [x] SubTask 8.2: 创建 `docs/hal-design-whitepaper.md`《HAL 设计白皮书》（294 行，覆盖 9 章 + 对比分析）

---

# Task 9: 验证与收尾

全量验证并更新版本管理。

- [x] SubTask 9.1: 运行 `cargo fmt --all -- --check` 通过
- [x] SubTask 9.2: 运行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 9.3: 运行 `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] SubTask 9.4: 运行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（40 个测试）
- [x] SubTask 9.5: 运行 `cargo test -p eneros-hal --features mock` 通过（23 个测试）
- [x] SubTask 9.6: 运行 `cargo deny check advisories licenses bans sources` 通过
- [x] SubTask 9.7: 交叉编译全部 6 个 crate 到 aarch64-unknown-none 通过
- [x] SubTask 9.8: 确认 `git status` 无垃圾文件被追踪
- [x] SubTask 9.9: 更新 spec checklist.md 所有检查项

---

# Task Dependencies

- Task 2 依赖 Task 1（crate 骨架）
- Task 3 依赖 Task 2（类型定义）
- Task 4 依赖 Task 3（trait 定义）
- Task 5 依赖 Task 4（mock 实现 trait + provider）
- Task 6 依赖 Task 5（测试 mock）
- Task 7 依赖 Task 6（CI 集成需测试通过）
- Task 8 可与 Task 5/6/7 并行（文档独立于代码）
- Task 9 依赖全部前序任务
