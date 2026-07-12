# EnerOS 代码规范

> 版本：v0.2.0  
> 适用范围：EnerOS 全 workspace 所有 Rust 代码  
> 蓝图依据：`蓝图/Power_Native_Agent_OS_Blueprint.md` §43.1、`记忆.md` §四

---

## no_std 规范（蓝图 §43.1）

### 适用范围

**全项目所有目标侧（target）Rust 代码必须 no_std**，禁止 `use std::*`。该规则覆盖三层架构：

| 层级 | 范围 | no_std | alloc | 说明 |
|------|------|--------|-------|------|
| ① 内核态 / RTOS 态 | Phase 0~3（kernel） | ✅ 必须 | 视场景 | 无堆用 `heapless`，有堆用 `alloc` |
| ② Agent Runtime | Phase 1 起（runtime） | ✅ 必须 | ✅ 可用 | 依赖 v0.11.0 用户堆；网络用 `smoltcp` |
| ③ LLM Rust 封装层 | v0.59.0 起 | ✅ 必须 | ✅ 可用 | C 底层（llama.cpp）例外，可有自己的运行时 |

### 禁止与替代方案对照表

| 禁止（目标侧） | 替代方案 | 说明 |
|----------------|----------|------|
| `std::collections::HashMap` | `alloc::collections::BTreeMap` | 需 alloc + 用户堆 |
| `std::time::Duration` | `core::time::Duration` | 无堆可用 |
| `std::sync::Mutex` | `spin::Mutex` | no_std 自旋锁 |
| `std::net::TcpStream` | `smoltcp` | no_std 网络栈 |
| `std::vec::Vec` | `alloc::vec::Vec` | 需 alloc + 用户堆 |
| `std::string::String` | `alloc::string::String` | 需 alloc + 用户堆 |
| `std::format!` | `alloc::format!` / `core::write!` | 视场景 |
| `std::fs` | 不适用（嵌入式无文件系统，v0.24.0 起自建） | — |
| `std::process::Command` | 不适用（目标侧无进程） | — |

### 例外：ci/ crate

`ci/` crate（eneros-ci）是 **host-side 开发工具**，在 CI runner / 开发者 PC 上运行，非嵌入式目标。它需要 `std::process::Command` 执行 cargo 子命令、`std::fs` 读取文件、`std::time` 计时，因此作为例外使用 std：

- **不纳入** no_std 合规范围
- **不参与** `aarch64-unknown-none` 交叉编译（CI 用 `-p eneros-kernel` / `-p eneros-runtime` 精确指定）
- 仅在 host 上以 `cargo run -p eneros-ci` 运行

除 `ci/` crate 外，所有 Rust 代码必须 no_std。

---

## 命名规范

遵循标准 Rust 命名约定（RFC 430）：

| 元素 | 规范 | 示例 |
|------|------|------|
| 函数 / 方法 | `snake_case` | `alloc_frame()` |
| 变量 / 局部 | `snake_case` | `page_count` |
| 模块 / crate | `snake_case` | `mm`, `device_drivers` |
| 类型 / Struct / Enum | `PascalCase` | `BuddyAllocator` |
| Trait | `PascalCase` | `QualityGate` |
| 常量 / 静态 | `SCREAMING_SNAKE_CASE` | `MAX_FRAME_SIZE` |
| 泛型参数 | 单大写或 PascalCase | `T`, `GateResult` |

- 禁止中文标识符
- 生命周期用小写：`'a`, `'ctx`

---

## 模块组织

### 目录与嵌套

- 每个 crate 一个顶层目录（`kernel/` `runtime/` `ai/` 等）
- 嵌套**不超过 3 层**：`kernel/src/mm/heap.rs` 合理；`kernel/src/mm/heap/impl/buddy/core.rs` 过深
- 目录名全小写 + 下划线（`device_drivers`），禁止驼峰
- 禁止中文目录名（`蓝图/` 文档目录例外）

### 文件组织

- 单文件建议不超过 500 行，过长应拆分子模块
- 每个公开 trait / struct 应有文档注释（`///`）
- `mod.rs` 仅用于模块声明与重导出，不放业务逻辑

---

## clippy.toml 配置说明

`clippy.toml` 位于仓库根目录，设定 clippy lint 严格度，确保 `-D warnings` 下的规则一致性。主要配置项：

| 配置项 | 含义 |
|--------|------|
| `type-complexity-threshold` | 类型复杂度阈值，超过则警告（鼓励拆分 type alias） |
| `too-many-arguments-threshold` | 函数参数数量上限，超过则警告 |
| `cognitive-complexity-threshold` | 认知复杂度阈值，超过则警告（鼓励拆分函数） |
| `enum-variant-name-threshold` | 枚举变体命名一致性阈值 |
| `single-char-binding-names-threshold` | 允许的单字符变量绑定数量 |

> 修改 `clippy.toml` 后需全 workspace 重新跑 `cargo clippy` 确认无新增 warning。

---

## rustfmt.toml 配置说明

`rustfmt.toml` 位于仓库根目录，统一代码格式化风格。CI 中 `cargo fmt --all -- --check` 依据此配置。主要配置项：

| 配置项 | 含义 |
|--------|------|
| `max_width` | 单行最大宽度（默认 100） |
| `hard_tabs` | 是否使用制表符缩进（false 用空格） |
| `tab_spaces` | 缩进空格数（通常 4） |
| `edition` | Rust edition（2021） |
| `reorder_imports` | 是否自动重排 use 语句 |
| `reorder_modules` | 是否自动重排 mod 声明 |
| `use_field_init_shorthand` | 是否使用字段初始化简写 `Foo { x }` |
| `newline_style` | 换行风格（Unix / Windows / Auto） |

> 提交前务必运行 `cargo fmt --all` 自动格式化，避免 CI 格式检查失败。

---

## 提交前检查清单

每次提交前，确保以下检查全部通过：

```bash
# 1. 格式化
cargo fmt --all

# 2. 格式检查（应无输出）
cargo fmt --all -- --check

# 3. Clippy lint（应无 warning）
cargo clippy --all-targets -- -D warnings

# 4. 安全与许可证检查
cargo deny check advisories licenses bans sources

# 5. 单元测试
cargo test --all

# 6. 交叉编译验证（目标侧代码）
cargo build -p eneros-kernel --target aarch64-unknown-none -Z build-std=core,alloc
cargo build -p eneros-runtime --target aarch64-unknown-none -Z build-std=core,alloc
```

或一键运行本地质量门禁：

```bash
make ci-local
```

### no_std 合规自查

提交目标侧代码（kernel / runtime）前，确认：

- [ ] `Cargo.toml` 中无 `std` 依赖（或仅 dev-dependencies 用 std）
- [ ] 源码无 `use std::*`
- [ ] 使用 `alloc::*` / `core::*` / `heapless::*` / `spin::*` 替代
- [ ] 交叉编译 `aarch64-unknown-none` 通过
