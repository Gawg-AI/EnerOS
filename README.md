# EnerOS / Power Native Agent OS

> **Power Native Agent OS** — Energy-internet-native Agent OS built on seL4 microkernel + Rust.
>
> **Tech Stack**: Rust nightly (`no_std`) + seL4 14.0.0 + ARM64 (`aarch64-unknown-none`)

[![CI](https://github.com/Gawg-AI/EnerOS/actions/workflows/ci.yml/badge.svg)](https://github.com/Gawg-AI/EnerOS/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

**[中文](#中文) | [English](#english)**

---

## 中文

### 项目简介

EnerOS（Power Native Agent OS）是一个面向能源互联网场景的 **Power Native + Agent Native + AI Native** 专用操作系统。基于 seL4 微内核形式化验证基座与 Rust `no_std` 运行时构建，采用混合关键性架构（Mixed Criticality），将**实时控制（RTOS 分区）**与**AI Agent（用户态分区）**隔离运行在同一 ARM64 硬件上。

**当前版本**：v0.22.0 — Phase 0（内核地基）已完成，477 个单元测试通过，准备进入 Phase 1（单机 MVP）。

#### 核心架构

```
┌─────────────────────────────────────────────────────────┐
│              慢平面（管理信息大区）                        │
│   Agent Runtime │ AI 双脑（LLM + Solver）│ 可观测性       │
├──────────────── Control Bus ─────────────────────────────┤
│   (Lock-free Ring Buffer + TTL + 约束包)                 │
├─────────────────────────────────────────────────────────┤
│              快平面（生产控制大区）                        │
│   RTOS 控制引擎（10ms 周期/抖动 <1ms）│ 设备协议栈        │
├─────────────────────────────────────────────────────────┤
│              seL4 微内核 + HAL                            │
│   分区调度器(ARINC 653) │ Capability │ 虚拟内存隔离       │
│   IPC │ SMP 多核 │ 时钟/看门狗/Panic                      │
└─────────────────────────────────────────────────────────┘
        7 层纵深安全 │ 国密 SM2/SM3/SM4 │ 横向隔离合规
```

#### 版本路线图

| Phase | 版本区间 | 版本数 | 目标 | 状态 |
|-------|---------|--------|------|------|
| Phase 0 | v0.1.0 ~ v0.22.0 | 25（+4 子版本） | 内核地基（seL4 + Rust + ARM64） | ✅ 完成 |
| Phase 1 | v0.23.0 ~ v0.74.0 | 60（+8 子版本） | 单机 MVP（Edge Box 端到端） | ⏳ 下一步 |
| Phase 2 | v0.75.0 ~ v0.126.0 | 53（+1 子版本） | 多机联邦（VPP/微电网） | — |
| Phase 3 | v0.127.0 ~ v0.170.0 | ~20（裁剪后） | seL4 深度定制 + 国产化 + 信创认证 | — |
| Phase 4 | v0.171.0 ~ v0.191.0 | 8（产品主路径） | 平台化（市场接入 + SDK 生态） | — |
| 商用版 | v1.0.0 | 1 | 最小可商用集合（MVP 联邦 + 合规 + SDK） | — |

> 主路径共 205 个开发版本（含 13 个刚性子版本），ADR-0001/0002 裁剪后实际约 167 版。

#### 关键架构决策（ADR）

| ADR | 决策 | 要点 |
|-----|------|------|
| ADR-0001 | Phase 3 采用 seL4 深度定制（方案 A） | 保留 seL4 内核，不自研；复用形式化证明；BSP 收敛为飞腾+鲲鹏+QEMU |
| ADR-0002 | Phase 4 研究线拆分 | P2P 撮合/区块链/MARL/RL 后置为研究附录；产品主路径仅 8 版 |
| ADR-0003 | 横向隔离合规 Go/No-Go 闸门 | Phase 0 硬出口条件：横向隔离合规结论必须通过 |
| ADR-0004 | v1.0.0 重定义为最小可商用集合 | 从"205 版全集总装"降维为"MVP 联邦+合规+SDK" |

#### 双脑 AI 架构

| 路径 | 内容 | MVP 可验收 | 实时性 |
|------|------|-----------|--------|
| **L1 主路径** | Solver-only（LP/MILP via HiGHS） | ✅ 是 | < 500ms，不依赖 LLM |
| **L2 增强路径** | LLM（llama.cpp 7B INT4）+ Solver | ❌ 否 | < 2s，离线复杂规划 |

---

### 目录结构

```
eneros/
├── Cargo.toml                 # Workspace 根配置
├── Cargo.lock                 # 依赖锁定（二进制 crate 入仓）
├── rust-toolchain.toml        # Rust nightly 版本锁定（nightly-2026-04-04）
├── .cargo/config.toml         # 交叉编译配置
├── Makefile                   # 统一构建入口
├── deny.toml                  # cargo-deny 许可证/安全配置
│
├── crates/                    # ★ 所有 Rust crate（按子系统分组）
│   ├── kernel/                #   内核子系统
│   │   ├── kernel/            #     eneros-kernel（启动/串口/内核入口）
│   │   ├── mm/                #     eneros-mm（内存管理/页表）
│   │   ├── heap/              #     eneros-heap（buddy 堆分配器）
│   │   ├── sched/             #     eneros-sched（调度器/分区调度/线程）
│   │   ├── smp/               #     eneros-smp（多核启动/IPI）
│   │   ├── ipc/               #     eneros-ipc（IPC 同步消息/通知/RingBuffer）
│   │   ├── controlbus/        #     eneros-controlbus（控制总线/TTL/约束包）
│   │   └── panic/             #     eneros-panic（Panic 框架）
│   ├── hal/                   #   硬件抽象层
│   │   ├── hal/               #     eneros-hal（HAL trait + arm64 实现）
│   │   └── board/             #     eneros-board（板级支持）
│   ├── runtime/               #   用户态运行时
│   │   ├── runtime/           #     eneros-runtime
│   │   ├── sel4-sys/          #     eneros-sel4-sys（seL4 syscall 绑定）
│   │   ├── hello/             #     eneros-hello（首个用户态样例）
│   │   └── user/heap/         #     eneros-user-heap（用户态堆）
│   └── drivers/               #   设备驱动
│       ├── time/              #     eneros-time（RTC/高精度定时器）
│       ├── watchdog/          #     eneros-watchdog（硬件看门狗）
│       ├── storage/           #     eneros-storage（块设备/文件系统）
│       └── power/             #     eneros-power（电源管理）
│
├── ci/                        # CI 工具 crate（eneros-ci）
├── configs/                   # 配置文件（设备树 .dts 等）
├── docs/                      # 工程文档（按方向分类）
│   ├── hal/ kernel/ runtime/ drivers/ smp/ boot/ conventions/ ci/
├── tools/                     # 构建/测试脚本
├── tests/                     # 集成测试
└── .github/workflows/ci.yml   # CI 流水线
```

> 蓝图文档位于 `蓝图/` 目录（不入仓，仅本地参考）。

---

### 环境要求

#### 开发主机

- **操作系统**：Ubuntu 22.04+ 或 WSL2
- **架构**：x86_64
- **内存**：≥ 4GB ｜ **磁盘**：≥ 10GB

#### 工具链版本

| 工具 | 版本 | 用途 |
|------|------|------|
| Rust nightly | 2026-04-04（锁定） | 编译器 |
| aarch64-linux-gnu-gcc | ≥ 11 | seL4 kernel C 编译 |
| qemu-system-aarch64 | ≥ 7.0 | ARM64 模拟验证 |
| cmake + ninja | ≥ 3.16 | seL4 构建 |
| dtc | 最新 | DTS → DTB 编译 |
| cargo-deny | 最新 | 许可证/供应链扫描 |
| cargo-audit | 最新 | 安全漏洞扫描 |

---

### 快速开始

```bash
# 1. 安装工具链（WSL2/Linux）
chmod +x tools/setup-toolchain.sh && ./tools/setup-toolchain.sh

# 2. 验证工具链
make check-tools

# 3. 构建（seL4 kernel + Rust runtime + 设备树）
make build

# 4. QEMU 运行验证
make run

# 5. GDB 调试（可选）
make gdb   # 终端 1：QEMU + GDB server
gdb-multiarch -x .gdbinit   # 终端 2：GDB 调试器
```

预期输出包含 `EnerOS boot: v0.22.0 (seL4 integrated)`。按 `Ctrl+A` 然后 `X` 退出 QEMU。

#### 常用 Make 目标

| 命令 | 说明 |
|------|------|
| `make build` | 全量构建（seL4 + Rust + DTB + 镜像） |
| `make run` | 构建并在 QEMU 中运行 |
| `make test` | 运行全部单元测试 |
| `make ci-local` | 本地质量门禁（fmt + clippy + deny + test） |
| `make clean` | 清理所有构建产物 |

---

### CI/CD

每次 PR 或 push 到 `main`/`develop` 分支时，CI 自动运行：

| 检查项 | 命令 | 说明 |
|--------|------|------|
| 代码格式 | `cargo fmt --all -- --check` | 格式一致 |
| Clippy lint | `cargo clippy --all-targets -- -D warnings` | 零 warning 容忍 |
| 安全与许可证 | `cargo deny check advisories licenses bans sources` | 漏洞 + 许可证双查 |
| 单元测试 | `cargo test --workspace` | 所有测试通过 |
| 交叉编译 | `cargo build --target aarch64-unknown-none` | ARM64 镜像可编译 |

CI 全流程目标 < 10 分钟。

#### 提交规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

```
<type>(<scope>): <subject>
```

允许的 type：`feat` `fix` `docs` `style` `refactor` `test` `chore` `ci` `perf` `build` `revert`

示例：`feat(kernel/heap): v0.10.0 实现内核态 buddy 堆分配器`

---

### 技术规范

#### no_std 合规（全项目强制）

```rust
// ✅ 正确
#![no_std]
use alloc::collections::BTreeMap;
use core::time::Duration;
use spin::Mutex;

// ❌ 禁止
use std::collections::HashMap;
use std::net::TcpStream;
```

#### 版本锁定

| 组件 | 版本 |
|------|------|
| Rust nightly | nightly-2026-04-04 |
| seL4 | 14.0.0 |
| rust-sel4 | v3.0.0 |
| 目标三元组 | aarch64-unknown-none |

#### 默认集成清单（防重复造轮子）

| 子系统 | 默认集成 | 自研范围 |
|--------|---------|---------|
| 文件系统 | littlefs2 | 能源特有点表存储接口封装 |
| TCP/IP 栈 | smoltcp（no_std） | 无 |
| DDS 总线 | Cyclone DDS | Rust 封装层 |
| MILP/LP Solver | HiGHS | Rust FFI 封装 + 能源建模层 |
| LLM 推理 | llama.cpp（C API） | Rust 封装层 |
| 密码学 | 国密 SM2/SM3/SM4 硬件加速 | 协议层适配 |

---

### 开发流程

1. 创建功能分支：`git checkout -b feature/v0.XX.0-xxx`
2. 编码实现（遵循 `no_std` 规范）
3. 本地测试：`cargo test` + QEMU 验证
4. 代码质量：`cargo fmt && cargo clippy && cargo deny check`
5. 提交代码（Conventional Commits 规范）
6. 推送并创建 PR
7. CI 全绿后合并

---

## English

### Overview

EnerOS (Power Native Agent OS) is an energy-internet-native Agent OS built on the seL4 microkernel and Rust `no_std` runtime. It employs a Mixed Criticality architecture, isolating **real-time control (RTOS partition)** and **AI Agent (user-space partition)** on the same ARM64 hardware.

**Current Version**: v0.22.0 — Phase 0 (Kernel Foundation) complete, 477 unit tests passing, ready for Phase 1 (Single-node MVP).

#### Core Architecture

```
┌─────────────────────────────────────────────────────────┐
│           Slow Plane (Management Zone)                   │
│   Agent Runtime │ AI Dual-Brain (LLM + Solver) │ Obs.    │
├──────────────── Control Bus ─────────────────────────────┤
│   (Lock-free Ring Buffer + TTL + Constraint Pkg)         │
├─────────────────────────────────────────────────────────┤
│           Fast Plane (Production Control Zone)           │
│   RTOS Engine (10ms cycle/jitter <1ms) │ Protocol Stack  │
├─────────────────────────────────────────────────────────┤
│           seL4 Microkernel + HAL                          │
│   Partition Sched (ARINC 653) │ Capability │ VSpace      │
│   IPC │ SMP │ Clock/Watchdog/Panic                        │
└─────────────────────────────────────────────────────────┘
     7-Layer Defense │ SM2/SM3/SM4 │ Lateral Isolation
```

#### Version Roadmap

| Phase | Versions | Count | Goal | Status |
|-------|---------|-------|------|--------|
| Phase 0 | v0.1.0 ~ v0.22.0 | 25 (+4 sub) | Kernel Foundation (seL4 + Rust + ARM64) | ✅ Done |
| Phase 1 | v0.23.0 ~ v0.74.0 | 60 (+8 sub) | Single-node MVP (Edge Box end-to-end) | ⏳ Next |
| Phase 2 | v0.75.0 ~ v0.126.0 | 53 (+1 sub) | Multi-node Federation (VPP/Microgrid) | — |
| Phase 3 | v0.127.0 ~ v0.170.0 | ~20 (trimmed) | seL4 Deep Customization + Localization | — |
| Phase 4 | v0.171.0 ~ v0.191.0 | 8 (product) | Platformization (Market Access + SDK) | — |
| Commercial | v1.0.0 | 1 | Minimal Commercial Set (MVP + Compliance + SDK) | — |

> 205 main-path dev versions total (incl. 13 rigid sub-versions); ~167 after ADR-0001/0002 trimming.

#### Key Architecture Decisions (ADR)

| ADR | Decision | Summary |
|-----|----------|---------|
| ADR-0001 | Phase 3: seL4 deep customization (Option A) | Keep seL4 kernel (no self-built kernel); reuse formal proofs; BSP converged to Phytium+Kunpeng+QEMU |
| ADR-0002 | Phase 4: Research line split | P2P matching/blockchain/MARL/RL deferred to research appendix; product main path = 8 versions |
| ADR-0003 | Lateral isolation compliance gate | Phase 0 hard exit: lateral isolation compliance must pass |
| ADR-0004 | v1.0.0 redefined as minimal commercial set | Down-scoped from "205-version full assembly" to "MVP federation + compliance + SDK" |

#### Dual-Brain AI Architecture

| Path | Content | MVP Verifiable | Latency |
|------|---------|---------------|---------|
| **L1 Main** | Solver-only (LP/MILP via HiGHS) | ✅ Yes | < 500ms, no LLM dependency |
| **L2 Enhanced** | LLM (llama.cpp 7B INT4) + Solver | ❌ No | < 2s, offline complex planning |

---

### Repository Structure

```
eneros/
├── Cargo.toml                 # Workspace root
├── Cargo.lock                 # Dependency lock (committed for binary crate)
├── rust-toolchain.toml        # Rust nightly pinned (nightly-2026-04-04)
├── .cargo/config.toml         # Cross-compilation config
├── Makefile                   # Unified build entry
├── deny.toml                  # cargo-deny license/safety config
│
├── crates/                    # ★ All Rust crates (grouped by subsystem)
│   ├── kernel/                #   Kernel subsystem
│   │   ├── kernel/            #     eneros-kernel (boot/serial/entry)
│   │   ├── mm/                #     eneros-mm (memory management/page tables)
│   │   ├── heap/              #     eneros-heap (buddy allocator)
│   │   ├── sched/             #     eneros-sched (scheduler/partition sched/threads)
│   │   ├── smp/               #     eneros-smp (multi-core boot/IPI)
│   │   ├── ipc/               #     eneros-ipc (IPC sync messaging/notifications/RingBuffer)
│   │   ├── controlbus/        #     eneros-controlbus (control bus/TTL/constraints)
│   │   └── panic/             #     eneros-panic (panic framework)
│   ├── hal/                   #   Hardware Abstraction Layer
│   │   ├── hal/               #     eneros-hal (HAL traits + arm64 impl)
│   │   └── board/             #     eneros-board (BSP)
│   ├── runtime/               #   User-space runtime
│   │   ├── runtime/           #     eneros-runtime
│   │   ├── sel4-sys/          #     eneros-sel4-sys (seL4 syscall bindings)
│   │   ├── hello/             #     eneros-hello (first user-space sample)
│   │   └── user/heap/         #     eneros-user-heap (user-space heap)
│   └── drivers/               #   Device drivers
│       ├── time/              #     eneros-time (RTC/high-res timers)
│       ├── watchdog/          #     eneros-watchdog (hardware watchdog)
│       ├── storage/           #     eneros-storage (block device/filesystem)
│       └── power/             #     eneros-power (power management)
│
├── ci/                        # CI tooling crate (eneros-ci)
├── configs/                   # Config files (device tree .dts, etc.)
├── docs/                      # Engineering docs (by topic)
├── tools/                     # Build/test scripts
├── tests/                     # Integration tests
└── .github/workflows/ci.yml   # CI pipeline
```

---

### Requirements

#### Dev Host

- **OS**: Ubuntu 22.04+ or WSL2 ｜ **Arch**: x86_64 ｜ **RAM**: ≥ 4GB ｜ **Disk**: ≥ 10GB

#### Toolchain

| Tool | Version | Purpose |
|------|---------|---------|
| Rust nightly | 2026-04-04 (pinned) | Compiler |
| aarch64-linux-gnu-gcc | ≥ 11 | seL4 kernel C compilation |
| qemu-system-aarch64 | ≥ 7.0 | ARM64 emulation |
| cmake + ninja | ≥ 3.16 | seL4 build |
| dtc | latest | DTS → DTB |
| cargo-deny | latest | License/supply-chain scan |
| cargo-audit | latest | Security vulnerability scan |

---

### Quick Start

```bash
# 1. Install toolchain (WSL2/Linux)
chmod +x tools/setup-toolchain.sh && ./tools/setup-toolchain.sh

# 2. Verify toolchain
make check-tools

# 3. Build (seL4 kernel + Rust runtime + device tree)
make build

# 4. Run in QEMU
make run

# 5. GDB debug (optional)
make gdb   # Terminal 1: QEMU + GDB server
gdb-multiarch -x .gdbinit   # Terminal 2: GDB debugger
```

Expected output includes `EnerOS boot: v0.22.0 (seL4 integrated)`. Press `Ctrl+A` then `X` to exit QEMU.

#### Common Make Targets

| Command | Description |
|---------|-------------|
| `make build` | Full build (seL4 + Rust + DTB + image) |
| `make run` | Build and run in QEMU |
| `make test` | Run all unit tests |
| `make ci-local` | Local quality gate (fmt + clippy + deny + test) |
| `make clean` | Clean all build artifacts |

---

### CI/CD

CI runs automatically on PR or push to `main`/`develop`:

| Check | Command | Description |
|-------|---------|-------------|
| Format | `cargo fmt --all -- --check` | Consistent formatting |
| Clippy | `cargo clippy --all-targets -- -D warnings` | Zero warning tolerance |
| Safety & License | `cargo deny check advisories licenses bans sources` | Vuln + license check |
| Unit Tests | `cargo test --workspace` | All tests pass |
| Cross-compile | `cargo build --target aarch64-unknown-none` | ARM64 image compiles |

Target: full pipeline < 10 minutes.

#### Commit Convention

Follows [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>
```

Allowed types: `feat` `fix` `docs` `style` `refactor` `test` `chore` `ci` `perf` `build` `revert`

Example: `feat(kernel/heap): v0.10.0 implement kernel buddy allocator`

---

### Technical Specs

#### no_std Compliance (Project-Wide Mandatory)

```rust
// ✅ Correct
#![no_std]
use alloc::collections::BTreeMap;
use core::time::Duration;
use spin::Mutex;

// ❌ Forbidden
use std::collections::HashMap;
use std::net::TcpStream;
```

#### Version Pins

| Component | Version |
|-----------|---------|
| Rust nightly | nightly-2026-04-04 |
| seL4 | 14.0.0 |
| rust-sel4 | v3.0.0 |
| Target triple | aarch64-unknown-none |

#### Default Integration (No Reinventing the Wheel)

| Subsystem | Default | Custom Scope |
|-----------|---------|-------------|
| Filesystem | littlefs2 | Energy-specific point table storage API |
| TCP/IP stack | smoltcp (no_std) | None |
| DDS bus | Cyclone DDS | Rust wrapper layer |
| MILP/LP Solver | HiGHS | Rust FFI + energy modeling layer |
| LLM inference | llama.cpp (C API) | Rust wrapper layer |
| Cryptography | SM2/SM3/SM4 hardware accel | Protocol adaptation |

---

### Development Workflow

1. Create feature branch: `git checkout -b feature/v0.XX.0-xxx`
2. Implement (follow `no_std` spec)
3. Local test: `cargo test` + QEMU verification
4. Quality check: `cargo fmt && cargo clippy && cargo deny check`
5. Commit (Conventional Commits)
6. Push and create PR
7. Merge after CI green

---

## License

MIT OR Apache-2.0

---

## Links

- **Repository**: [https://github.com/Gawg-AI/EnerOS](https://github.com/Gawg-AI/EnerOS)
- **CI Pipeline**: [GitHub Actions](https://github.com/Gawg-AI/EnerOS/actions)
- **Blueprint Docs**: `蓝图/` directory (not committed, local reference only)
- **Dev Rules**: `.trae/rules/记忆.md`
