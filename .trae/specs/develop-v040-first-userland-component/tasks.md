# Tasks — EnerOS v0.4.0 第一个 Rust 用户态组件

- [x] Task 1: 创建 sel4-sys crate（seL4 syscall FFI 绑定）
  - [ ] SubTask 1.1: 创建 `sel4-sys/Cargo.toml`（crate 名 `eneros-sel4-sys`，version.workspace，no_std，无外部依赖）
  - [ ] SubTask 1.2: 创建 `sel4-sys/src/lib.rs`：定义 `Endpoint` 结构体（`cap: u64`，derive Clone/Copy/Debug）
  - [ ] SubTask 1.3: 创建 `sel4-sys/src/lib.rs`：实现 `seL4_put_char(c: u8) -> isize`，aarch64 用 inline asm（`svc #0`，x0=字符，x7=syscall号），host 用 stub 返回 0
  - [ ] SubTask 1.4: 创建 `sel4-sys/src/lib.rs`：实现 `seL4_send(ep: Endpoint, msg: u64) -> isize` 与 `seL4_recv(ep: Endpoint) -> u64`，同样双实现
  - [ ] SubTask 1.5: 添加单元测试（Endpoint 构造与字段访问、host stub 返回值验证，≥ 80% 覆盖率）

- [x] Task 2: 重构 runtime crate（二进制 → 库）
  - [ ] SubTask 2.1: 修改 `runtime/Cargo.toml`：删除 `sel4` git 依赖，添加 `eneros-sel4-sys = { path = "../sel4-sys" }`；description 更新为 "EnerOS user-space runtime library (no_std)"
  - [ ] SubTask 2.2: 删除 `runtime/src/main.rs`，创建 `runtime/src/lib.rs`：`#![no_std]`，无 `#![no_main]`，无 `#[panic_handler]`，无 `#[lang = "eh_personality"]`
  - [ ] SubTask 2.3: 创建 `runtime/src/serial.rs`：定义 `SeL4Serial` 结构体实现 `SerialOut` trait（putc 调用 `eneros_sel4_sys::seL4_put_char`，puts 处理 `\n` → `\r\n`）
  - [ ] SubTask 2.4: 创建 `runtime/src/console.rs`：定义 `ConsoleWriter` 实现 `core::fmt::Write`；定义 `print!` / `println!` 宏（基于 `core::fmt::write`）；定义 `init()` 函数（no-op）
  - [ ] SubTask 2.5: 在 `runtime/src/lib.rs` 声明 `pub mod serial;` + `pub mod console;` + `pub use console::{print, println, init};`
  - [ ] SubTask 2.6: 添加单元测试（println 宏格式化、ConsoleWriter write_str、serial newline 转换，host 侧运行）

- [x] Task 3: 创建 hello 二进制 crate
  - [ ] SubTask 3.1: 创建 `hello/Cargo.toml`（crate 名 `eneros-hello`，version.workspace，no_std，依赖 `eneros-runtime = { path = "../runtime" }`）
  - [ ] SubTask 3.2: 创建 `hello/src/main.rs`：`#![no_std]` + `#![no_main]` + `#![feature(lang_items)]`
  - [ ] SubTask 3.3: 实现 `_start` 入口：调用 `eneros_runtime::init()`，用 `println!` 输出 4 行消息，进入 `loop { spin_loop() }`
  - [ ] SubTask 3.4: 实现 `#[lang = "eh_personality"]` 与 `#[panic_handler]`（panic 时用 `println!` 输出 `[PANIC]` 前缀，然后循环）

- [x] Task 4: 更新 workspace Cargo.toml
  - [ ] SubTask 4.1: 修改 `Cargo.toml`：members 增加 `"sel4-sys"` 与 `"hello"` → `["kernel", "runtime", "ci", "board", "sel4-sys", "hello"]`
  - [ ] SubTask 4.2: 修改 `Cargo.toml`：workspace.package.version 从 `"0.3.0"` 更新为 `"0.4.0"`

- [x] Task 5: 更新 CI 质量门禁
  - [ ] SubTask 5.1: 修改 `ci/src/gate.rs`：clippy 排除列表移除 `eneros-runtime`，新增 `--exclude eneros-hello`（保留 `--exclude eneros-kernel`）
  - [ ] SubTask 5.2: 修改 `ci/src/gate.rs`：test 排除列表移除 `eneros-runtime`，新增 `--exclude eneros-hello`（保留 `--exclude eneros-kernel`）
  - [ ] SubTask 5.3: 修改 `.github/workflows/ci.yml`：版本标识更新为 v0.4.0；交叉编译步骤新增 `cargo build -p eneros-sel4-sys` 与 `cargo build -p eneros-hello`

- [x] Task 6: 更新 Makefile
  - [ ] SubTask 6.1: 修改 `Makefile`：VERSION 从 `0.3.0` 更新为 `0.4.0`；头部注释更新版本
  - [ ] SubTask 6.2: 修改 `Makefile`：新增 `hello-build` 目标（`cargo build -p eneros-hello --target $(TARGET) -Z build-std=core,alloc`）
  - [ ] SubTask 6.3: 修改 `Makefile`：`runtime-build` 目标改为构建库（`cargo build -p eneros-runtime --target $(TARGET) -Z build-std=core,alloc`）；help 添加 hello-build 说明

- [x] Task 7: 创建文档
  - [ ] SubTask 7.1: 创建 `docs/userland-runtime-design.md`（《Rust 用户态运行时设计》：架构、模块划分、print!/println! 宏机制、与 sel4-sys 的关系、未来扩展）
  - [ ] SubTask 7.2: 创建 `docs/sel4-api-bindings.md》（《seL4 API 绑定说明》：syscall 编号、调用约定（x0-x7）、Endpoint 类型、当前已绑定接口列表、未来扩展计划）

- [x] Task 8: 验证与测试
  - [ ] SubTask 8.1: `cargo fmt --all -- --check` 无差异
  - [ ] SubTask 8.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 无 warning
  - [ ] SubTask 8.3: `cargo test -p eneros-sel4-sys` 单元测试通过
  - [ ] SubTask 8.4: `cargo test -p eneros-runtime` 单元测试通过
  - [ ] SubTask 8.5: `cargo run -p eneros-ci` 质量门禁全绿
  - [ ] SubTask 8.6: `cargo deny check advisories licenses bans sources` 通过
  - [ ] SubTask 8.7: `cargo build -p eneros-sel4-sys --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
  - [ ] SubTask 8.8: `cargo build -p eneros-runtime --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过
  - [ ] SubTask 8.9: `cargo build -p eneros-hello --target aarch64-unknown-none -Z build-std=core,alloc` 交叉编译通过

# Task Dependencies
- [Task 2] 依赖 [Task 1]（runtime 依赖 sel4-sys）
- [Task 3] 依赖 [Task 2]（hello 依赖 runtime）
- [Task 5] 依赖 [Task 3]（CI 排除列表需知道 hello 存在）
- [Task 6] 依赖 [Task 3]（Makefile 需知道 hello 存在）
- [Task 8] 依赖 [Task 1]~[Task 7] 全部完成
- [Task 4]、[Task 7] 可与 [Task 1]~[Task 3] 并行（无代码依赖）
