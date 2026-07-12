# EnerOS v0.3.0 — seL4 在 ARM64 硬件启动 Spec

## Why
v0.1.0 仅在 QEMU 上验证了 seL4 + Rust 工具链可编译可启动；v0.3.0 需要建立"硬件启动链路"——从设备树描述、串口驱动到启动配置，使镜像能在真实 ARM64 硬件（或 QEMU virt 兜底）上启动并输出 seL4 boot log。这是 Phase 0 从"PC 上能编译"跨越到"真机上能启动"的里程碑。

## What Changes
- 新增 `board/` crate（no_std 库）：`BootInfo`、`BootStage`、`SerialOut` trait、`Pl011Serial` 驱动
- 新增 `board/qemu-virt/` 板级配置：`boot.txt`（U-Boot 启动脚本）、`dts`（设备树覆盖片段）
- 新增 `tools/flash.sh`：SD 卡烧录脚本（支持 QEMU 兜底模式）
- 新增文档：真机启动指南、串口调试手册、设备树说明
- 修改 `Cargo.toml`（workspace）：members 添加 `"board"`，版本更新为 `"0.3.0"`
- 修改 `Makefile`：VERSION 升级 `0.3.0`，添加 `board-build` 目标
- 修改 `.github/workflows/ci.yml`：交叉编译添加 board crate，版本标识更新
- 修改 `ci/src/gate.rs`：board crate 无 panic_handler，可纳入 host 侧 clippy/test

## Impact
- Affected specs: v0.1.0（工具链）、v0.2.0（CI/CD）
- Affected code: `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`、`kernel/src/lib.rs`（注释更新）
- 下游影响：v0.4.0（第一个 Rust 用户态组件）将依赖 board crate 的 `SerialOut` 与 `BootInfo`

## 设计决策

### D1: board crate 为 no_std 库，不定义 panic_handler
**理由**：board crate 是库而非二进制，panic_handler 应由最终二进制（runtime）定义。这样 board crate 可在 host 上运行单元测试（蓝图 §6.1 要求 BootInfo 字段解析覆盖率 ≥ 80%）。

### D2: QEMU virt 为首要目标，真机为扩展
**理由**：蓝图 §8.1 指出"硬件不可用（中/高）→ QEMU 兜底"。当前无真机硬件，先实现 QEMU virt 板级配置，board crate 通过 `BoardConfig` trait 支持多板扩展。

### D3: Pl011Serial 为最小串口驱动，不依赖外部 crate
**理由**：蓝图 §4.5 给出的 PL011 驱动仅用 `core::ptr::read_volatile/write_volatile`，无需嵌入 HAL 抽象层。保持最小依赖链，符合 Phase 0 精简原则。

### D4: v0.3.0 不是瓶颈版本（非 ★），代码可用伪代码但签名必须可编译
**理由**：蓝图 §43.2 + 记忆.md §4.4。但蓝图给出的代码示例本身是可编译的骨架代码，将按蓝图实现。

## ADDED Requirements

### Requirement: Board Crate
系统 SHALL 提供一个 `board` crate（no_std 库），包含硬件启动所需的类型定义和最小串口驱动。

#### Scenario: BootInfo 结构体定义
- **WHEN** 开发者引用 `board::BootInfo`
- **THEN** 结构体包含 `board_name: &'static str`、`ram_base: u64`、`ram_size: u64`、`serial_base: u64`、`cpu_count: u32`、`freq_mhz: u32` 六个字段

#### Scenario: BootStage 枚举定义
- **WHEN** 开发者引用 `board::BootStage`
- **THEN** 枚举包含 `RomInit`、`Bootloader`、`Sel4Loaded`、`Sel4Running` 四个变体

#### Scenario: SerialOut trait
- **WHEN** 类型实现 `board::SerialOut` trait
- **THEN** 需实现 `putc(&self, c: u8)`、`puts(&self, s: &str)`、`hex(&self, v: u64)` 三个方法

#### Scenario: Pl011Serial 驱动
- **WHEN** 开发者构造 `Pl011Serial::new(0x0900_0000)`
- **THEN** 返回的实例实现 `SerialOut` trait，`putc` 等待 TX FIFO 不满后写入数据寄存器

### Requirement: 板级配置文件
系统 SHALL 为 QEMU virt 板提供启动配置文件。

#### Scenario: boot.txt
- **WHEN** U-Boot 加载 `board/qemu-virt/boot.txt`
- **THEN** 脚本设置 `bootcmd` 加载 seL4 镜像到 RAM 并跳转入口地址

#### Scenario: DTS 覆盖片段
- **WHEN** 编译设备树时引用 `board/qemu-virt/dts`
- **THEN** 包含 PL011 UART、内存、CPU 节点定义（与 `configs/qemu-virt.dts` 一致）

### Requirement: 烧录脚本
系统 SHALL 提供 `tools/flash.sh` 脚本支持镜像烧录到 SD 卡。

#### Scenario: 烧录到 SD 卡
- **WHEN** 执行 `tools/flash.sh /dev/sdX`
- **THEN** 脚本将 `build/eneros-0.3.0.img` 写入指定设备

#### Scenario: QEMU 兜底模式
- **WHEN** 执行 `tools/flash.sh --qemu`
- **THEN** 脚本启动 QEMU virt 验证镜像可启动

### Requirement: 文档
系统 SHALL 提供三份文档：真机启动指南、串口调试手册、设备树说明。

### Requirement: 单元测试
系统 SHALL 为 `BootInfo` 字段解析提供单元测试，覆盖率 ≥ 80%。

## MODIFIED Requirements

### Requirement: Workspace 结构
workspace members 从 `["kernel", "runtime", "ci"]` 扩展为 `["kernel", "runtime", "ci", "board"]`，workspace.package.version 更新为 `"0.3.0"`。

### Requirement: CI 流水线
CI 交叉编译步骤 SHALL 新增 `cargo build -p eneros-board --target aarch64-unknown-none -Z build-std=core,alloc`。board crate 无 panic_handler，无需从 host 侧 clippy/test 中排除。

### Requirement: Makefile
Makefile VERSION 更新为 `0.3.0`，新增 `board-build` 目标构建 board crate。

### Requirement: 质量门禁（ci crate）
`ci/src/gate.rs` 的 clippy/test 命令无需排除 `eneros-board`（board crate 不含 panic_handler，可在 host 编译）。
