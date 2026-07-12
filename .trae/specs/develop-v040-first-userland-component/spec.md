# EnerOS v0.4.0 — 第一个 Rust 用户态组件 Spec

## Why

v0.3.0 建立了硬件启动链（设备树、PL011 驱动、U-Boot 脚本），但 seL4 之上尚未运行任何 Rust 用户态程序。v0.4.0 是 Phase 0 的终点（P0-A），需在 seL4 用户态运行第一个 Rust 程序（Hello World），建立 seL4 API 的 Rust 绑定与最小 no_std 运行时，验证"Rust + seL4 用户态"技术路径可行，为后续所有用户态组件（HAL、调度器、IPC）铺路。

## What Changes

- **新增 `sel4-sys` crate**（`eneros-sel4-sys`）：seL4 最小 syscall FFI 绑定，提供 `seL4_put_char` / `seL4_send` / `seL4_recv` 与 `Endpoint` 类型；aarch64 目标用 inline asm 实现 svc 调用，host 目标提供 stub 便于单元测试
- **重构 `runtime` crate**（`eneros-runtime`）：从二进制（`main.rs`）转换为库（`lib.rs`），移除 `#![no_main]` / `#[panic_handler]` / `#[lang = "eh_personality"]`，提供 `print!` / `println!` 宏与 `init()` 入口；删除未使用的 `sel4` git 依赖，改用 `eneros-sel4-sys` 路径依赖
- **新增 `hello` crate**（`eneros-hello`）：no_std + no_main 二进制，定义 `_start` 入口与 `#[panic_handler]`，通过 runtime 库打印 "Hello from Rust on seL4!"
- **更新 workspace**：members 增加 `"sel4-sys"` 与 `"hello"`；版本号 `0.3.0` → `0.4.0`
- **更新 CI 质量门禁**：`eneros-runtime` 转为库后可参与 host 侧 clippy/test（移出排除列表）；新增 `eneros-hello` 排除（含 panic_handler/no_main，仅交叉编译验证）
- **更新 Makefile**：新增 `hello-build` 目标；`runtime-build` 改为构建库；版本号更新
- **新增文档**：《Rust 用户态运行时设计》、《seL4 API 绑定说明》

## Impact

- **Affected specs**: 
  - v0.3.0（board crate 提供的 `SerialOut` trait 与 `Pl011Serial` 驱动，v0.4.0 runtime 在 host 测试时用 sel4-sys stub，在目标板用 seL4 syscall）
  - v0.5.0（HAL 接口规范——v0.4.0 用户态运行时为 HAL 提供宿主环境）
- **Affected code**:
  - `Cargo.toml`（workspace 根：members、版本）
  - `runtime/Cargo.toml`（删除 sel4 git 依赖，添加 eneros-sel4-sys 路径依赖）
  - `runtime/src/main.rs` → 删除，替换为 `runtime/src/lib.rs`
  - `ci/src/gate.rs`（排除列表调整）
  - `.github/workflows/ci.yml`（交叉编译新增 sel4-sys、hello；版本标识）
  - `Makefile`（VERSION、hello-build 目标）
  - `deny.toml`（无影响——删除 sel4 git 依赖后 allow-git 条目保留但不再触发）

## ADDED Requirements

### Requirement: seL4 Syscall FFI 绑定（eneros-sel4-sys）

系统 SHALL 提供一个 no_std 库 crate `eneros-sel4-sys`，封装 seL4 最小 syscall 接口，供用户态程序调用。

#### Scenario: aarch64 目标调用 seL4_put_char
- **WHEN** 在 `aarch64-unknown-none` 目标上调用 `seL4_put_char(b'A')`
- **THEN** 通过 `svc #0` 指令触发 seL4 syscall，x0 传入字符，x7 传入 syscall 号 0（debug putchar）
- **AND** 返回 seL4 syscall 结果（isize）

#### Scenario: host 单元测试使用 stub
- **WHEN** 在 host（x86_64）目标上调用 `seL4_put_char(b'A')`
- **THEN** 返回 0（stub 实现），不触发任何 syscall
- **AND** 不产生编译错误（通过 `#[cfg(not(target_arch = "aarch64"))]` 守卫）

#### Scenario: Endpoint 类型传递
- **WHEN** 构造 `Endpoint { cap: 42 }` 并传递给 `seL4_send`
- **THEN** 函数接收 Endpoint 的 `cap` 字段作为 syscall 参数

### Requirement: 用户态运行时库（eneros-runtime）

系统 SHALL 提供一个 no_std 库 crate `eneros-runtime`，为用户态程序提供 `print!` / `println!` 宏与控制台初始化接口。

#### Scenario: println 宏输出字符串
- **WHEN** 调用 `println!("Hello from Rust on seL4!")`
- **THEN** 字符串通过 `eneros-sel4_sys::seL4_put_char` 逐字符输出到串口
- **AND** 末尾自动追加 `\r\n`（`\n` 前补 `\r`）

#### Scenario: 运行时初始化
- **WHEN** 调用 `eneros_runtime::init()`
- **THEN** 控制台进入就绪状态（本版本为 no-op，seL4 负责串口初始化）

#### Scenario: panic 信息输出
- **WHEN** hello 二进制触发 panic
- **THEN** `#[panic_handler]` 通过 `println!` 输出 `[PANIC]` 前缀与 panic 位置信息

### Requirement: Hello World 用户态组件（eneros-hello）

系统 SHALL 提供一个 no_std + no_main 二进制 crate `eneros-hello`，作为 seL4 用户态第一个 Rust 程序。

#### Scenario: 启动并输出 Hello
- **WHEN** seL4 加载并跳转到 `eneros-hello` 的 `_start` 入口
- **THEN** 程序调用 `eneros_runtime::init()` 初始化运行时
- **AND** 通过 `println!` 输出 "Hello from Rust on seL4!"
- **AND** 输出 "EnerOS Phase 0 - first userland component."
- **AND** 输出 "== Userland component alive ==" 与 "Target: aarch64-unknown-none"
- **AND** 进入无限循环（不返回）

#### Scenario: 交叉编译生成二进制
- **WHEN** 执行 `cargo build -p eneros-hello --target aarch64-unknown-none -Z build-std=core,alloc`
- **THEN** 生成 `target/aarch64-unknown-none/release/eneros-hello` ELF 文件
- **AND** 退出码为 0

## MODIFIED Requirements

### Requirement: runtime crate 角色

**变更前**（v0.1.0~v0.3.0）：`eneros-runtime` 为二进制 crate（`src/main.rs`），含 `_start` 入口、`#[panic_handler]`、`#[lang = "eh_personality"]`，通过 inline asm 直接调用 seL4 debug putchar，被 CI 排除出 host 侧 clippy/test。

**变更后**（v0.4.0）：`eneros-runtime` 为库 crate（`src/lib.rs`），无 `#![no_main]`、无 `#[panic_handler]`、无 `#[lang = "eh_personality"]`，提供 `print!` / `println!` 宏与 `init()` 函数，依赖 `eneros-sel4-sys`，可参与 host 侧 clippy/test。`_start` 入口与 panic_handler 职责转移至 `eneros-hello` 二进制。

## REMOVED Requirements

### Requirement: runtime 的 sel4 git 依赖

**Reason**: v0.1.0 引入的 `sel4 = { git = "https://github.com/seL4/rust-sel4.git", tag = "v3.0.0" }` 从未被实际使用（runtime/main.rs 用 inline asm 直接调用 syscall），且引入 git 依赖增加构建复杂度与 deny 配置负担。v0.4.0 改用自研 `eneros-sel4-sys` 最小绑定。

**Migration**: 删除 `runtime/Cargo.toml` 中的 `sel4` 依赖，替换为 `eneros-sel4-sys = { path = "../sel4-sys" }`。`deny.toml` 中的 `allow-git` 条目保留（无害，未来可能复用）。

---

## 设计决策

### D1: 目录结构——顶层 crate 而非 `user/` 子目录

蓝图 v0.4.0 §3 使用 `user/hello/`、`user/runtime/`、`user/sel4-sys/` 路径，但工作区规则（记忆.md §2.1）标准目录树将 `runtime/` 置于顶层。决策：遵循工作区规则，三个 crate 均置于顶层（`sel4-sys/`、`runtime/`、`hello/`），避免移动现有 `runtime/` 目录的破坏性变更。`user/` 前缀视为逻辑分组，非物理路径要求。

### D2: sel4-sys 双实现——aarch64 inline asm + host stub

`eneros-sel4-sys` 在 `aarch64` 目标用 `core::arch::asm!` 实现 `svc #0` syscall；在非 aarch64 目标（host）提供返回 0 的 stub。通过 `#[cfg(target_arch = "aarch64")]` 守卫，使 sel4-sys 与依赖它的 runtime 均可在 host 上编译与单元测试。

### D3: runtime 转为库后可参与 host 测试

`eneros-runtime` 移除 `#[panic_handler]` / `#[lang = "eh_personality"]` 后，与 `eneros-board` 一样可参与 host 侧 clippy/test（no_std 库的测试 harness 自动链接 std）。CI gate.rs 从 clippy/test 排除列表中移除 `eneros-runtime`，新增 `eneros-hello` 排除。

### D4: QEMU+seL4 实际启动验证为延后项

v0.3.0 建立了启动链基础设施（DTS、boot.txt、flash.sh），但未实际构建/运行 seL4 内核镜像（需 seL4 源码集成与 cmake 构建，工作量超出 v0.4.0 范围）。v0.4.0 验证以交叉编译通过 + host 单元测试通过为准；QEMU 实际启动 "Hello" 输出留待 seL4 构建集成完成后补充验证（非 v0.4.0 阻塞项）。

### D5: println! 宏基于 core::fmt::Write

实现 `core::fmt::Write` trait 的 `ConsoleWriter`，通过 `core::fmt::write()` 格式化输出。这是 no_std 环境标准做法，支持 `{}` / `{:?}` 等格式说明符，无需 alloc。
