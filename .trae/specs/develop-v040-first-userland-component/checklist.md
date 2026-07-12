# Checklist — EnerOS v0.4.0 第一个 Rust 用户态组件

## sel4-sys crate
- [x] `sel4-sys/Cargo.toml` 存在，crate 名为 `eneros-sel4-sys`，version.workspace = true
- [x] `sel4-sys/src/lib.rs` 包含 `#![no_std]`
- [x] `Endpoint` 结构体包含 `cap: u64` 字段，derive Clone/Copy/Debug
- [x] `seL4_put_char(c: u8) -> isize` 在 aarch64 目标用 inline asm（`svc #0`）
- [x] `seL4_put_char` 在非 aarch64 目标返回 0（stub）
- [x] `seL4_send(ep: Endpoint, msg: u64) -> isize` 实现双实现
- [x] `seL4_recv(ep: Endpoint) -> u64` 实现双实现
- [x] 单元测试覆盖 Endpoint 构造与 host stub 返回值
- [x] `#[cfg(target_arch = "aarch64")]` 守卫正确隔离两套实现

## runtime crate 重构
- [x] `runtime/Cargo.toml` 删除 `sel4` git 依赖
- [x] `runtime/Cargo.toml` 添加 `eneros-sel4-sys = { path = "../sel4-sys" }`
- [x] `runtime/src/main.rs` 已删除
- [x] `runtime/src/lib.rs` 包含 `#![no_std]`，无 `#![no_main]`
- [x] `runtime/src/lib.rs` 无 `#[panic_handler]`，无 `#[lang = "eh_personality"]`
- [x] `runtime/src/lib.rs` 声明 `pub mod serial;` 与 `pub mod console;`
- [x] `runtime/src/serial.rs` 定义 `SeL4Serial` 实现 `SerialOut` trait
- [x] `serial::SeL4Serial::putc` 调用 `eneros_sel4_sys::seL4_put_char`
- [x] `serial::SeL4Serial::puts` 对 `\n` 自动补 `\r`
- [x] `runtime/src/console.rs` 定义 `ConsoleWriter` 实现 `core::fmt::Write`
- [x] `console::print!` 宏基于 `core::fmt::write`
- [x] `console::println!` 宏在 print 后追加换行
- [x] `console::init()` 函数存在（no-op）
- [x] 单元测试覆盖 println 格式化与 serial newline 转换

## hello 二进制 crate
- [x] `hello/Cargo.toml` 存在，crate 名为 `eneros-hello`，version.workspace = true
- [x] `hello/Cargo.toml` 依赖 `eneros-runtime = { path = "../runtime" }`
- [x] `hello/src/main.rs` 包含 `#![no_std]` + `#![no_main]`
- [x] `_start` 入口调用 `eneros_runtime::init()`
- [x] 输出包含 "Hello from Rust on seL4!"
- [x] 输出包含 "EnerOS Phase 0 - first userland component."
- [x] 输出包含 "== Userland component alive =="
- [x] 输出包含 "Target: aarch64-unknown-none"
- [x] `#[panic_handler]` 输出 `[PANIC]` 前缀
- [x] `#[lang = "eh_personality"]` 存在
- [x] 主循环为 `loop { core::hint::spin_loop(); }`

## workspace Cargo.toml
- [x] members 包含 `"sel4-sys"` 与 `"hello"`
- [x] workspace.package.version 为 `"0.4.0"`

## CI 质量门禁
- [x] `ci/src/gate.rs` clippy 排除列表移除 `eneros-runtime`
- [x] `ci/src/gate.rs` clippy 排除列表新增 `eneros-hello`
- [x] `ci/src/gate.rs` test 排除列表移除 `eneros-runtime`
- [x] `ci/src/gate.rs` test 排除列表新增 `eneros-hello`
- [x] `ci/src/gate.rs` 保留 `--exclude eneros-kernel`
- [x] `.github/workflows/ci.yml` 版本标识更新为 v0.4.0
- [x] `.github/workflows/ci.yml` 交叉编译步骤包含 `cargo build -p eneros-sel4-sys`
- [x] `.github/workflows/ci.yml` 交叉编译步骤包含 `cargo build -p eneros-hello`

## Makefile
- [x] Makefile VERSION 为 `0.4.0`
- [x] Makefile 包含 `hello-build` 目标
- [x] `make hello-build` 运行 `cargo build -p eneros-hello --target aarch64-unknown-none -Z build-std=core,alloc`
- [x] Makefile `runtime-build` 改为构建库（`cargo build -p eneros-runtime`）
- [x] Makefile help 包含 hello-build 说明

## 文档
- [x] `docs/userland-runtime-design.md` 存在且包含架构说明
- [x] `docs/userland-runtime-design.md` 包含 print!/println! 宏机制说明
- [x] `docs/userland-runtime-design.md` 包含与 sel4-sys 的关系
- [x] `docs/sel4-api-bindings.md` 存在且包含 syscall 编号
- [x] `docs/sel4-api-bindings.md` 包含调用约定（x0-x7 寄存器）
- [x] `docs/sel4-api-bindings.md` 包含 Endpoint 类型说明
- [x] `docs/sel4-api-bindings.md` 包含已绑定接口列表

## 验证
- [x] `cargo fmt --all -- --check` 无差异
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 无 warning
- [x] `cargo test -p eneros-sel4-sys` 全部通过
- [x] `cargo test -p eneros-runtime` 全部通过
- [x] `cargo run -p eneros-ci` 全绿
- [x] `cargo deny check advisories licenses bans sources` 通过
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
