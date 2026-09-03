# EnerOS / Power Native Agent OS

> **Power Native Agent OS** — Energy-internet-native Agent OS built on seL4 microkernel + Rust.
>
> **Tech Stack**: Rust nightly (`no_std`) + seL4 14.0.0 + ARM64 (`aarch64-unknown-none`)

[![CI](https://github.com/Gawg-AI/EnerOS/actions/workflows/ci.yml/badge.svg)](https://github.com/Gawg-AI/EnerOS/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSES.md)

**[中文](#中文) | [English](#english)**

---

## 中文

### 项目简介

EnerOS（Power Native Agent OS）是一个面向能源互联网场景的 **Power Native + Agent Native + AI Native** 专用操作系统。基于 seL4 微内核形式化验证基座与 Rust `no_std` 运行时构建，采用混合关键性架构（Mixed Criticality），将**实时控制（RTOS 分区）**与**AI Agent（用户态分区）**隔离运行在同一 ARM64 硬件上。

**当前版本**：workspace v0.317.x（精确值以根 `Cargo.toml` 为准）— 交付冲刺 44–59 卷完成（候选冻结 tag `v1.0.0-rc.1`；终验矩阵与缺口清单见 `evidence/52/`）；蓝图 61 Phase 0–7 全出口后，蓝图 62/63（单源收口）与蓝图 64（调用方最后一截收口，卷 64.1 协议键表单源 + goose vlan_id 活缺陷修复）推进中；测试基线计数随卷出口证据更新（52 终验以来各卷 Exit 均全绿）；生产发布（v1.0.0）未授权——当前裁决 NO-GO（`docs/v1.0.0-go-no-go.md`）。

> **唯一内核路线 = seL4**（ADR-0023 用户决策，ADR-0030 执行确认，KRN-001）：自研 microkernel 已淘汰（G2 验证缺陷），产品镜像内核/引导为 seL4（`third_party/sel4-build.sh`）；EnerOS root-task 消费 BootInfo 并 spawn EL0 服务（KRN-002~004）。

#### 核心架构

```
┌─────────────────────────────────────────────────────────┐
│              慢平面（管理信息大区）                        │
│   Agent Runtime │ AI 双脑（LLM + Solver）│ 可观测性       │
├──────────────── Control Bus ─────────────────────────────┤
│   (Lock-free Ring Buffer + TTL + 约束包)                 │
├─────────────────────────────────────────────────────────┤
│              快平面（生产控制大区）                        │
│   RTOS 控制引擎（10ms 周期/抖动 <1ms，设计目标——真机实测待 bring-up B9）│ 设备协议栈        │
├─────────────────────────────────────────────────────────┤
│              seL4 微内核 + HAL                            │
│   分区调度器(ARINC 653) │ Capability │ 虚拟内存隔离       │
│   IPC │ SMP 多核 │ 时钟/看门狗/Panic                      │
└─────────────────────────────────────────────────────────┘
        7 层纵深安全 │ Ed25519/SHA-256/TLS1.3（国密 SM2/3/4=实验性，D4）│ 横向隔离
```

#### 版本路线图

| Phase | 版本区间 | 版本数 | 目标 | 状态 |
|-------|---------|--------|------|------|
| Phase 0 | v0.1.0 ~ v0.22.0 | 25（+4 子版本） | 内核地基（seL4 + Rust + ARM64） | ✅ 完成 |
| Phase 1 | v0.23.0 ~ v0.74.0 | 60（+8 子版本） | 单机 MVP（Edge Box 端到端） | ⏳ 下一步 |
| Phase 2 | v0.75.0 ~ v0.126.0 | 53（+1 子版本） | 多机联邦（VPP/微电网） | — |
| Phase 3 | v0.127.0 ~ v0.170.0 | ~20（裁剪后） | seL4 深度定制 + 国产化 + 信创认证 | — |
| Phase 4 | v0.171.0 ~ v0.191.0 | 8（产品主路径） | 平台化（市场接入 + SDK 生态） | ✅ 完成 |
| 商用版 | v1.0.0 | 1 | 最小可商用集合（MVP 联邦 + 合规 + SDK） | —（未授权） |

> 主路径共 205 个开发版本（含 13 个刚性子版本），ADR-0001/0002 裁剪后实际约 167 版。
>
> **注意（HYG-002，2026-09-03 刷新）**：workspace 版本已推进至 **v0.316.x**（交付冲刺
> 44–59 卷 + 蓝图 61/62/63 深化收口），远超本路线图表的 v0.191.0 规划；当前版本一律以根 `Cargo.toml`
> （version.workspace）与 `git` 为准——本表仅保留历史规划语义。

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
| **L1 主路径** | Solver-only（LP/MILP，HiGHS 求解器**预留未接线**：feature 零激活，AI-004 具名落地前保持停用；现役 L1 决策=静态启发式后端） | ⚠️ 求解器未接线 | < 500ms，不依赖 LLM |
| **L2 增强路径** | 站端 LLM peer 经 MQTT 远程供给（ADR-0045 方案 B 主线）；本地 llama.cpp 推理 spike=NO-GO（内存/移植面证据见 `docs/ai/local-inference-spike.md`，重启条件三条） | ⚠️ 需配套站端服务 | < 2s（受网络 RTT 支配） |

---

### 历史数据闭环（SPEC-C · D-TSDB-2）

| 能力 | 说明 |
|------|------|
| 遥测历史落盘 | **target 侧显式停用（E-DEF-01，2026-08-28 win4 实测）**：磁盘 TSDB 接缝（EIPC 块命令面 → littlefs2 + 列存 chunk + Snappy）在 svc-telemetry 装配中置 `None`（littlefs2 小分片往返成本使 TEL_INGEST deadline 物理不可满足，实测登记见 E-DEF-01 / A-history-contract F-A12）；**内存环接替**——采集线走基线词形 `TSDB-COMMIT batch=<seq> items=<n>`，不伪造持久化。修复方向（burst 块协议 / PAYLOAD_CAP 扩容 / eneros-fs 持久 mount）归 C 卷 owner 裁决 |
| 北向历史取回 | SCADA 经 MQTT 管理 RPC `history-query`（viewer 角色）取任意点位最近 N 条（≤512 钳制+提示），二进制分页行区经 mgmt_response 回传，**不复用 OBSF 遥测帧语义**（broker transcript 对账基线保护） |
| 判定行三态 | `TSDB-DISK-COMMIT batch=<seq> items=<n>` / `TSDB-DISK-DROPPED n=` / `TSDB-DISK-REJECTED n=`——**TSDB-DISK-\* 为磁盘介质词形，当前 target 装配不可达**（E-DEF-01 停用态，见上）；内存环降级沿用基线词形 `TSDB-COMMIT`（两词形区分 = transcript 连续性资产，A-history-contract F-A13） |
| 能力边界 | 仅最后 N 条快照窗口、非全量导出、无订阅式推送/跨节点聚合/SQL 查询面；retention 策略调参仍 OPEN（OQ-3）。未授予存储边或打开失败时显式降级为进程内环（不伪造持久化）。**磁盘落盘路径当前整体不可用（E-DEF-01 显式停用，见「遥测历史落盘」行）** |

内存与区段审计：`docs/delivery/wp-c-memory-audit-spec-c-20260827.md`。

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
│   ├── （内核源码不在 crates/ 下：seL4 14.0.0 微内核位于 third_party/
│   │    sel4-*（sel4-build / sel4-top / sel4-root / sel4-patches 等），
│   │    经 sel4-build-rust.sh 构建；crates/runtime/root-task 消费
│   │    BootInfo 拉起 EL0 服务）
│   ├── hal/                   #   硬件抽象层
│   │   ├── hal/               #     eneros-hal（HAL trait + arm64 实现）
│   │   └── board/             #     eneros-board（板级支持）
│   ├── runtime/               #   用户态运行时
│   │   ├── root-task/         #     eneros-root-task（EL0 首进程/服务编排）
│   │   ├── sel4-sys/          #     eneros-sel4-sys（seL4 syscall 安全绑定）
│   │   ├── runtime/           #     eneros-runtime
│   │   ├── config-bundle/     #     eneros-config-bundle（签名配置束）
│   │   ├── durable-log/       #     eneros-durable-log（掉电一致日志）
│   │   ├── durable-state/     #     eneros-durable-state（状态持久化）
│   │   └── hello/             #     eneros-hello（首个用户态样例）
│   ├── drivers/               #   设备驱动
│   │   ├── time/              #     eneros-time（RTC/高精度定时器）
│   │   ├── watchdog/          #     eneros-watchdog（硬件看门狗）
│   │   ├── storage/           #     eneros-storage（块设备/文件系统）
│   │   └── power/             #     eneros-power（电源管理）
│   ├── protocols/             #   协议栈（modbus/iec104/iec61850/mqtt/cj188…）
│   ├── security/              #   安全域（crypto/tls/secure-boot/ota/attestation/audit…）
│   ├── agents/                #   智能体域（device-agent/市场·联邦·孪生·运维…）
│   ├── ai/                    #   AI 组件（solver 族/intent/dual-brain/safety/gguf-loader…）
│   └── observability/         #   可观测性（metrics/logging/tracing/dashboard/mgmt-api…）
│
├── product/                   # 产品策略/profile/冻结册（装配与策略单一事实源）
├── xtask/                     # 形式化构建入口（cargo xtask …）
├── fuzz/                      # libFuzzer 模糊测试靶（独立 workspace）
├── ci/                        # CI 工具 crate（eneros-ci）
├── configs/                   # 配置文件（设备树 .dts 等）
├── docs/                      # 工程文档（按方向分类）
│   ├── hal/ kernel/ runtime/ drivers/ smp/ boot/ conventions/ ci/
├── tools/                     # 构建/测试脚本
├── tests/                     # 集成测试
└── .github/workflows/ci.yml   # CI 流水线
```

> 蓝图文档位于 `蓝图/` 目录（本地保留、不入 git——政策见 `.gitignore` 末块；盘外备份 `~/文档/eneros-local-backups/`）。

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

# 2. 构建（seL4 kernel + Rust runtime + DTB + 镜像）——唯一正式构建入口
cargo xtask build all

# 3. QEMU 运行验证
cargo xtask run qemu-prod

# 4. 全量单元测试
cargo test --workspace
```

预期输出包含 UEFI 交接行 `ENEROS stage=uefi-entry profile=qemu-prod secure=required`，随后是 boot-evidence JSON 行（`"result":"PASS"`，串口同流输出；真值取自 `evidence/62/vol62-3/normal.qemu-serial.log` 实弹串口日志）。按 `Ctrl+A` 然后 `X` 退出 QEMU。

#### 常用入口

> 旧 `make <target>` 目标已全部停用并重定向到 `cargo xtask`。

| 命令 | 说明 |
|------|------|
| `cargo xtask build all` | 全量构建（seL4 + Rust + DTB + 镜像） |
| `cargo xtask image raw\|iso\|recovery\|vars\|update` | 产物镜像制作 |
| `cargo xtask run qemu-prod` | 构建并在 QEMU 中运行生产 profile |
| `cargo test --workspace` | 运行全部单元测试 |
| `cargo xtask evidence verify` | 证据链校验 |

---

### CI/CD

**完整清单以 `.github/workflows/ci.yml` 为唯一事实源**（5 个 job：`g0-fail-closed` 首 job 45min 超时 / `evidence42-portable-tools` 矩阵 / `catalog-gate-43` / `g1-fuzz-gates` / `gkernel-qemu-spawn-ipc`；触发 `branches: ['*']`——PR 与全部分支 push）。要点（非全量）：

| 检查项 | 命令 | 说明 |
|--------|------|------|
| 代码格式 | `cargo fmt --all -- --check` | 格式一致 |
| Clippy lint | `cargo clippy --all-targets -- -D warnings` | 零 warning 容忍 |
| loom 并发模型 | account/p2p-match/metrics/telemetry-queue 等 5 组（C-243/C-247 gate） | 并发模型全跑 |
| 安全与许可证 | `cargo deny check advisories licenses bans sources` | 漏洞 + 许可证双查（advisories 全量） |
| 单元测试 | `cargo test --workspace` | 所有测试通过 |
| QEMU 启动证据 | seL4 kernel + root image + QEMU boot（fail-closed） | 引导链实弹 |

CI 全流程目标 < 10 分钟（现矩阵含 QEMU/fuzz job，实际更长——以 ci.yml 超时配置为准）。

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

#### 第三方集成状态清单（现役 / 预留 / 已撤线——防重复造轮子）

| 子系统 | 默认集成 | 自研范围 |
|--------|---------|---------|
| 文件系统 | littlefs2 | 能源特有点表存储接口封装 |
| TCP/IP 栈 | smoltcp（no_std） | 无 |
| DDS 总线 | —（已撤线：ADR-021 out-of-v1.0，Agent 总线由 MQTT/IPC 承担） | 无 |
| MILP/LP Solver | HiGHS **预留未接线**（`highs-ffi` feature 零激活；素材在库——build.rs + third_party/highs-1.7.2；AI-004 具名落地前保持停用；现役=静态启发式后端） | Rust FFI 封装 + 能源建模层（在库未接） |
| LLM 推理 | 本地 llama.cpp spike=NO-GO（`docs/ai/local-inference-spike.md`，与 L2 路径行同口径）；`llama-cpp` feature 预留（默认关） | Rust 封装层（未接线） |
| 密码学 | Ed25519/SHA-256/rustls(P-256)（产品线）；SM2/SM3/SM4 手卷为实验性（无硬件加速宣称，D4） | 协议层适配 |

#### 管理面动作接线现状（RBAC/CLI 放行面 > 服务接立面）

管理 RPC 面共 22 个动作（`MgmtAction`）；svc-mgmt 服务侧**接实 6 个**：status / agent-restart / config-get / log-query / support-bundle / history-query。RBAC（mgmt-api `ApiAction::permission()` 逐动作权限映射）与 mgmt CLI 按全量动作面放行，未接实动作由服务侧 fail-closed 拒绝（不空成功）：

- **OTA 三动作**（update-check / update-apply / update-rollback）→ `ota-channel-unbound:mgmt→ota-grant-missing`（mgmt→ota EIPC 授予边未建立；升格接通属独立侦察项，蓝图 60 §5 默认不做）；
- **其余 13 个**（slo / agent-list / agent-start / agent-stop / config-set / crash-symbolize / maintenance-enter / maintenance-exit / backup-create / restore / revoke / factory-reset / secure-erase）→ 裸词形 `action-not-wired:<action>`。

对未接实动作收到上述 Failed 词形是**预期行为**（明确原因词形），不是 bug；接线面以 svc-mgmt executor 白名单为单一事实源（蓝图 60 WP-60.2 登记，计数有单测钉住）。

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

**Current Version**: workspace v0.316.x — delivery-sprint volumes 44–59 complete, followed by blueprint 61 phases 0–7 and deepening blueprints 62/63 (single-source close-out) (candidate freeze tag `v1.0.0-rc.1`; final verdict NO-GO, gap ledger in `evidence/52/`). The live version is whatever the root `Cargo.toml` declares.

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
     7-Layer Defense │ Ed25519/SHA-256/TLS1.3 │ SM2/SM3/SM4 (experimental) │ Lat. Isolation
```

#### Version Roadmap

| Phase | Versions | Count | Goal | Status |
|-------|---------|-------|------|--------|
| Phase 0 | v0.1.0 ~ v0.22.0 | 25 (+4 sub) | Kernel Foundation (seL4 + Rust + ARM64) | ✅ Done |
| Phase 1 | v0.23.0 ~ v0.74.0 | 60 (+8 sub) | Single-node MVP (Edge Box end-to-end) | ⏳ Next |
| Phase 2 | v0.75.0 ~ v0.126.0 | 53 (+1 sub) | Multi-node Federation (VPP/Microgrid) | — |
| Phase 3 | v0.127.0 ~ v0.170.0 | ~20 (trimmed) | seL4 Deep Customization + Localization | — |
| Phase 4 | v0.171.0 ~ v0.191.0 | 8 (product) | Platformization (Market Access + SDK) | ✅ Done |
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
| **L1 Main** | Solver-only (LP/MILP; HiGHS solver **reserved, unwired**: feature never activated, stays disabled until AI-004 lands; live L1 decision = static heuristic backend) | ⚠️ solver unwired | < 500ms, no LLM dependency |
| **L2 Enhanced** | Station-side LLM peer via MQTT (ADR-0045 plan B mainline); local llama.cpp inference spike = NO-GO (`docs/ai/local-inference-spike.md`) | ⚠️ requires station service | < 2s (network RTT bound) |

---

### History Data Loop (SPEC-C / D-TSDB-2)

| Capability | Notes |
|------------|-------|
| Telemetry history persistence | **Explicitly disabled on target (E-DEF-01, live-probed in the 2026-08-28 win4 window)**: the disk-TSDB seam (EIPC block-command face → littlefs2 + columnar chunks + Snappy) is wired to `None` in the svc-telemetry assembly (littlefs2 small-fragment round-trip cost makes the TEL_INGEST deadline physically unsatisfiable; measurements registered under E-DEF-01 / A-history-contract F-A12). **In-memory ring takes over** — the ingest line emits the baseline word form `TSDB-COMMIT batch=<seq> items=<n>`; no fake persistence. Fix directions (burst block protocol / PAYLOAD_CAP headroom / eneros-fs persistent mount) are deferred to the C-volume owner. |
| Northbound history query | SCADA calls management RPC `history-query` (viewer role) over MQTT for the last N points of any metric (clamped at 512 with a hint flag). Paged binary rows return over the mgmt_response path — **OBSF telemetry-frame semantics are not reused**, protecting the broker-transcript reconciliation baseline. |
| Judgment lines | `TSDB-DISK-COMMIT batch=<seq> items=<n>` / `TSDB-DISK-DROPPED n=` / `TSDB-DISK-REJECTED n=` — **TSDB-DISK-\* are disk-medium word forms, unreachable in the current target assembly** (E-DEF-01 disabled state, see above); the in-memory ring fallback keeps the baseline word form `TSDB-COMMIT` (the two word forms are a transcript-continuity asset, A-history-contract F-A13). |
| Capability boundary | Last-N snapshot window only — no full export, no subscription push, no cross-node aggregation, no SQL face. Retention tuning stays OPEN (OQ-3). Explicit fallback to an in-process ring when the storage grant is absent or the open fails (no fake persistence). **The disk persistence path is currently unavailable end-to-end (E-DEF-01, see "Telemetry history persistence").** |

Memory & region audit: `docs/delivery/wp-c-memory-audit-spec-c-20260827.md`.

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
│   ├── (kernel sources are not under crates/: the seL4 14.0.0
│   │    microkernel lives in third_party/sel4-* (sel4-build /
│   │    sel4-top / sel4-root / sel4-patches …), built via
│   │    sel4-build-rust.sh; crates/runtime/root-task consumes
│   │    BootInfo and spawns EL0 services)
│   ├── hal/                   #   Hardware Abstraction Layer
│   │   ├── hal/               #     eneros-hal (HAL traits + arm64 impl)
│   │   └── board/             #     eneros-board (BSP)
│   ├── runtime/               #   User-space runtime
│   │   ├── root-task/         #     eneros-root-task (first EL0 process / service orchestration)
│   │   ├── sel4-sys/          #     eneros-sel4-sys (safe seL4 syscall bindings)
│   │   ├── runtime/           #     eneros-runtime
│   │   ├── config-bundle/     #     eneros-config-bundle (signed config bundle)
│   │   ├── durable-log/       #     eneros-durable-log (power-fail-consistent log)
│   │   ├── durable-state/     #     eneros-durable-state (persistent state)
│   │   └── hello/             #     eneros-hello (first user-space sample)
│   ├── drivers/               #   Device drivers
│   │   ├── time/              #     eneros-time (RTC/high-res timers)
│   │   ├── watchdog/          #     eneros-watchdog (hardware watchdog)
│   │   ├── storage/           #     eneros-storage (block device/filesystem)
│   │   └── power/             #     eneros-power (power management)
│   ├── protocols/             #   Protocol stacks (modbus/iec104/iec61850/mqtt…)
│   ├── security/              #   Security domain (crypto/tls/secure-boot/ota/audit…)
│   ├── agents/                #   Agent domain (device-agent/market/federation/twin…)
│   ├── ai/                    #   AI components (solver family/intent/dual-brain/safety…)
│   └── observability/         #   Observability (metrics/logging/tracing/mgmt-api…)
│
├── product/                   # Product policy/profiles/frozen register (assembly source of truth)
├── xtask/                     # Formal build entry (`cargo xtask …`)
├── fuzz/                      # libFuzzer targets (standalone workspace)
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

# 2. Build (seL4 kernel + Rust runtime + DTB + image) — the single formal entry
cargo xtask build all

# 3. Run in QEMU
cargo xtask run qemu-prod

# 4. Full unit tests
cargo test --workspace
```

Expected output includes the UEFI handoff line `ENEROS stage=uefi-entry profile=qemu-prod secure=required`, followed by a boot-evidence JSON line with `"result":"PASS"` (ground truth: `evidence/62/vol62-3/normal.qemu-serial.log` live serial log). Press `Ctrl+A` then `X` to exit QEMU.

#### Common Entry Points

> Legacy `make <target>` goals are retired and redirect to `cargo xtask`.

| Command | Description |
|---------|-------------|
| `cargo xtask build all` | Full build (seL4 + Rust + DTB + image) |
| `cargo xtask image raw\|iso\|recovery\|vars\|update` | Artifact image production |
| `cargo xtask run qemu-prod` | Build and run the production QEMU profile |
| `cargo test --workspace` | Run all unit tests |
| `cargo xtask evidence verify` | Evidence-chain verification |

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

#### Third-party Integration Status (wired / reserved / withdrawn — no reinventing the wheel)

| Subsystem | Default | Custom Scope |
|-----------|---------|-------------|
| Filesystem | littlefs2 | Energy-specific point table storage API |
| TCP/IP stack | smoltcp (no_std) | None |
| DDS bus | — (withdrawn: ADR-021 out-of-v1.0; agent bus carried by MQTT/IPC) | None |
| MILP/LP Solver | HiGHS **reserved, unwired** (`highs-ffi` feature never activated; materials in-tree — build.rs + third_party/highs-1.7.2; stays disabled until AI-004 lands; live decision = static heuristic backend) | Rust FFI + energy modeling layer (in-tree, unwired) |
| LLM inference | local llama.cpp spike = NO-GO (`docs/ai/local-inference-spike.md`, same verdict as the L2-path row); `llama-cpp` feature reserved (off by default) | Rust wrapper layer (unwired) |
| Cryptography | Ed25519/SHA-256/rustls(P-256) (product line); SM2/SM3/SM4 hand-rolled, experimental — no hardware-acceleration claims (D4) | Protocol adaptation |

> **Management-plane action wiring**: only 6 of 22 mgmt RPC actions are service-wired; the rest fail closed with the word form `ota-channel-unbound:mgmt→ota-grant-missing` (OTA triple — the mgmt→ota EIPC grant edge is absent) or bare `action-not-wired:<action>`. Receiving those Failed word forms is expected behavior, not a bug — see the Chinese section 「管理面动作接线现状」 for the full register.

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
- **Blueprint Docs**: `蓝图/` directory (kept locally, not tracked in git — policy in the `.gitignore` trailer; off-machine backup `~/文档/eneros-local-backups/`)
- **Dev Rules**: `.trae/rules/记忆.md`
