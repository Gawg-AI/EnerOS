# Checklist — EnerOS v0.5.0 HAL 接口规范设计

> **变更ID**：develop-v050-hal-interface-spec
> **用途**：逐项验证 v0.5.0 交付物是否符合 spec.md 与蓝图要求
> **蓝图依据**：`蓝图/phase0.md` §v0.5.0（第 792–1038 行）
> **验证状态**：全部通过（2026-07-12）

---

## 一、Crate 骨架与 workspace 集成

- [x] `hal/Cargo.toml` 存在，package name = `eneros-hal`
- [x] `hal/Cargo.toml` 使用 `version.workspace = true` / `edition.workspace = true` 等 workspace 继承
- [x] `hal/Cargo.toml` 含 `[features] default = []` 和 `mock = []`
- [x] `hal/Cargo.toml` 无外部依赖（仅 core）
- [x] `hal/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`（支持 host 测试的 no_std 模式）
- [x] `hal/src/lib.rs` 含 `pub mod types;` 和 `pub use types::*;`
- [x] `hal/src/lib.rs` 含 `#[cfg(feature = "mock")] pub mod mock;`
- [x] workspace 根 `Cargo.toml` members 含 `"hal"`
- [x] workspace 根 `Cargo.toml` version = `"0.5.0"`
- [x] `cargo build -p eneros-hal` 成功

## 二、公共类型 types.rs

- [x] `MemFlags` 定义，含 readable/writable/executable/device/cacheable 字段，派生 Clone/Copy/Debug
- [x] `MemFlags` 有便捷构造方法（`device()` / `normal()` / `code()`）
- [x] `IrqTrigger` 定义，含 Edge/Level 变体，派生 Clone/Copy/Debug/PartialEq/Eq
- [x] `HalError` 定义，含 InvalidParam/OutOfResource/NotSupported/HardwareFault/PermissionDenied 变体，派生 Debug
- [x] `HalError` 实现 `Display`（core::fmt::Display）
- [x] `GpioDir` 定义，含 Input/Output 变体，派生 Clone/Copy/Debug/PartialEq/Eq
- [x] `PullMode` 定义，含 None/Up/Down 变体，派生 Clone/Copy/Debug/PartialEq/Eq
- [x] `GpioConfig` 定义，含 pin/dir/pull 字段，派生 Clone/Copy
- [x] `IrqAction` 定义，含 Handled/WakeThread/Disabled 变体，派生 Debug/PartialEq/Eq
- [x] `IrqHandler` 定义为 `pub type IrqHandler = fn(irq: u32) -> IrqAction;`
- [x] 所有类型有文档注释

## 三、6 个 HAL trait 定义

- [x] `HalCpu` trait 定义，含 enable_irq/disable_irq/current_core/core_count/halt/wfi
- [x] `HalMem` trait 定义，含 map/unmap/translate/set_domain
- [x] `HalIrq` trait 定义，含 register/unregister/enable/disable/eoi
- [x] `HalClock` trait 定义，含 now_ns/frequency_hz/set_deadline
- [x] `HalSerial` trait 定义，含 write/read/flush
- [x] `HalGpio` trait 定义，含 set_dir/set/get/toggle
- [x] 所有 trait 方法有文档注释（契约说明）
- [x] 所有 trait 方法无泛型参数（dyn 安全）
- [x] 所有 trait 方法无 `Self` 返回类型（dyn 安全）
- [x] 所有 trait 方法非 `async fn`（蓝图 §8.5）
- [x] `HalCpu::halt(&self) -> !` 返回发散类型
- [x] `HalMem::map` 签名为 `fn map(&self, pa: u64, va: u64, flags: MemFlags) -> Result<(), HalError>`
- [x] `HalIrq::register` 签名为 `fn register(&self, irq: u32, trigger: IrqTrigger, handler: IrqHandler) -> Result<(), HalError>`

## 四、HalProvider 注册器模式

- [x] `HalProvider` trait 定义，含 cpu/mem/irq/clock/serial/gpio 六个方法
- [x] 每个 `HalProvider` 方法返回 `&'static dyn HalXxx`
- [x] `static mut HAL: Option<&'static dyn HalProvider> = None;` 定义存在
- [x] `init_hal(provider: &'static dyn HalProvider)` 函数实现
- [x] `hal() -> &'static dyn HalProvider` 函数实现
- [x] `hal()` 在未初始化时 panic 并提示 `"HAL not initialized"`
- [x] unsafe 块限于 init_hal/hal 两处，有安全说明注释
- [x] `#[allow(static_mut_refs)]` 已添加

## 五、Mock 实现 mock.rs

- [x] `MockHal` 结构体定义
- [x] `MockHal` 实现 `HalCpu`
- [x] `MockHal` 实现 `HalMem`
- [x] `MockHal` 实现 `HalIrq`
- [x] `MockHal` 实现 `HalClock`
- [x] `MockHal` 实现 `HalSerial`
- [x] `MockHal` 实现 `HalGpio`
- [x] `MockHalProvider` 实现 `HalProvider`
- [x] `cargo build -p eneros-hal --features mock` 成功
- [x] mock 实现符合蓝图 §4.5 代码（current_core==0, core_count==1, halt 用 loop{}+spin_loop）

## 六、单元测试

- [x] types.rs 含 `#[cfg(test)] mod tests`
- [x] 测试覆盖 MemFlags 构造与字段访问（device/normal/code/custom）
- [x] 测试覆盖 IrqTrigger 变体匹配
- [x] 测试覆盖 HalError 变体匹配（用 matches! 宏）
- [x] 测试覆盖 HalError Display 实现
- [x] 测试覆盖 GpioConfig 构造
- [x] 测试覆盖 IrqAction 变体
- [x] mock.rs 含 `#[cfg(test)] mod tests`
- [x] 测试 MockHal 各 trait 方法返回预期值
- [x] 测试 MockHalProvider 注入后 hal().cpu().current_core() == 0
- [x] 测试 IrqHandler 函数指针可赋值与调用
- [x] `cargo test -p eneros-hal --features mock` 全部通过
- [x] 测试数量 ≥ 10 个（实际 23 个：types 11 + mock 12）

## 七、no_std 合规

- [x] crate 根 `#![cfg_attr(not(test), no_std)]` 存在（正式构建 no_std，测试构建链接 std）
- [x] 无 `use std::*` 任何导入
- [x] 仅使用 `core::*`
- [x] 交叉编译 `cargo build -p eneros-hal --target aarch64-unknown-none` 成功

## 八、CI / Makefile / 质量门禁

- [x] `.github/workflows/ci.yml` 版本标识为 v0.5.0
- [x] `.github/workflows/ci.yml` cross-build 含 `Build hal crate` 步骤
- [x] `.github/workflows/ci.yml` test 任务含 `Run eneros-hal mock tests` 步骤
- [x] `Makefile` VERSION = 0.5.0
- [x] `Makefile` 含 `hal-build` 目标
- [x] `Makefile` 含 `hal-test` 目标
- [x] `Makefile` help 文本含 hal-build / hal-test 说明
- [x] `ci/src/gate.rs` clippy 排除项仅 eneros-kernel/eneros-hello（hal 作为库可参与）
- [x] `ci/src/gate.rs` 注释说明 hal 为库 crate 可 host 测试
- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（40 个测试）
- [x] `cargo test -p eneros-hal --features mock` 通过（23 个测试）
- [x] `cargo deny check advisories licenses bans sources` 通过

## 九、交叉编译验证

- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-board --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hal --target aarch64-unknown-none` 成功

## 十、文档交付

- [x] `docs/hal-interface-spec.md` 存在（461 行）
- [x] 《HAL 接口规范》覆盖 6 个 trait 的所有方法契约
- [x] 《HAL 接口规范》含 HalProvider 注册器模式说明
- [x] 《HAL 接口规范》含公共类型表
- [x] 《HAL 接口规范》含与 v0.6.0/v0.7.0 对接说明
- [x] `docs/hal-design-whitepaper.md` 存在（294 行）
- [x] 《HAL 设计白皮书》含 trait 抽象选型理由
- [x] 《HAL 设计白皮书》含 dyn 安全性分析
- [x] 《HAL 设计白皮书》含与 seL4 libplatsupport/Linux HAL/embedded-hal 对比
- [x] 《HAL 设计白皮书》含扩展路径（RISC-V BSP）

## 十一、工作区整洁

- [x] `git status` 无 target/ 被追踪
- [x] `git status` 无 build/ 被追踪
- [x] `git status` 无 *.elf/*.bin/*.img 被追踪
- [x] `git status` 无 *.dtb 被追踪
- [x] `git status` 无 IDE 缓存（.idea/.vscode/.trae/cache）被追踪
- [x] `.gitignore` 覆盖所有新产生的文件类型

## 十二、蓝图合规性

- [x] 蓝图 §43.1：所有 Rust 代码 no_std（`cfg_attr(not(test), no_std)` 模式）
- [x] 蓝图 §43.2：非瓶颈版本，trait/struct 签名可编译
- [x] 蓝图 §8.5：无 async fn in traits
- [x] 蓝图 §9.5：trait 文档化每个方法契约
- [x] 蓝图 §7.1：6 个 trait 定义完整
- [x] 蓝图 §7.2：mock 实现可编译运行
- [x] 蓝图 §7.3：文档齐全
- [x] 蓝图 §7.4：HAL 不暴露特权操作给非特权调用者（trait 文档说明调用约束）
- [x] 蓝图 §7.5：规范就绪（出口判定）

---

## 验证总结

| 验证项 | 结果 | 详情 |
|--------|------|------|
| cargo fmt | ✅ 通过 | 无格式问题 |
| cargo clippy (workspace) | ✅ 通过 | 无 warning |
| cargo clippy (hal mock) | ✅ 通过 | 无 warning |
| cargo test (workspace) | ✅ 通过 | 40 个测试全部通过 |
| cargo test (hal mock) | ✅ 通过 | 23 个测试全部通过 |
| cargo deny | ✅ 通过 | advisories/bans/licenses/sources 全 ok |
| 交叉编译 (6 crate) | ✅ 通过 | 全部成功构建到 aarch64-unknown-none |
| git status | ✅ 通过 | 无垃圾文件被追踪 |

**总计：63 个单元测试通过，6 个 crate 交叉编译成功，全部检查项通过。**
