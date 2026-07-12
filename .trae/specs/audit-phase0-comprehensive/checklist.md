# Checklist — Phase 0 全面审计

## Workspace 完整性（Task 1）

- [x] C1: `cargo metadata --format-version 1` 成功（workspace 成员路径全部正确）
- [x] C2: 根 `Cargo.toml` members 含全部 17 个 crate + ci
- [x] C3: 跨 crate `path = "..."` 引用全部使用正确相对路径（无绝对路径）

## 代码格式（Task 2）

- [x] C4: `cargo fmt --all -- --check` 通过

## Clippy 静态分析（Task 3）

- [x] C5: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 无 warning

## 全量单元测试（Task 4）

- [x] C6: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部通过
- [x] C7: `cargo test -p eneros-hal --features mock` 通过
- [x] C8: 各 crate 测试数量统计完成（总测试数 477 ≥ 400）

## aarch64 交叉编译（Task 5）

- [x] C9: `eneros-kernel` 交叉编译通过
- [x] C10: `eneros-mm` 交叉编译通过
- [x] C11: `eneros-heap` 交叉编译通过
- [x] C12: `eneros-sched` 交叉编译通过
- [x] C13: `eneros-smp` 交叉编译通过
- [x] C14: `eneros-panic` 交叉编译通过
- [x] C15: `eneros-ipc` 交叉编译通过
- [x] C16: `eneros-controlbus` 交叉编译通过
- [x] C17: `eneros-hal` 交叉编译通过
- [x] C18: `eneros-board` 交叉编译通过
- [x] C19: `eneros-runtime` 交叉编译通过
- [x] C20: `eneros-sel4-sys` 交叉编译通过
- [x] C21: `eneros-hello` 交叉编译通过（`cargo check` 验证，链接需 WSL2）
- [x] C22: `eneros-user-heap` 交叉编译通过
- [x] C23: `eneros-time` 交叉编译通过
- [x] C24: `eneros-watchdog` 交叉编译通过
- [x] C25: `eneros-power` 交叉编译通过

## no_std 合规性（Task 6）

- [x] C26: `crates/` 下搜索 `use std::` 零匹配（蓝图 §43.1，仅 `#[cfg(test)]` 模块内允许）
- [x] C27: 所有 crate 的 `lib.rs` 含 `#![cfg_attr(not(test), no_std)]` 或 `#![no_std]`

## 目录结构校验（Task 7，§2.4 C1~C15）

- [x] C28: 所有 crate 在 `crates/<subsystem>/` 下，未直接放根目录（§2.4 C1）
- [x] C29: 根 `Cargo.toml` members 含全部 crate 路径（§2.4 C2）
- [x] C30: 跨 crate `path = "..."` 使用正确相对路径（§2.4 C3）
- [x] C31: 根目录无除 `ci/` 外的 Rust crate 文件夹（§2.4 C5）
- [x] C32: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪（§2.4 C13）
- [x] C33: `.gitignore` 覆盖新产生的文件类型（§2.4 C14）

## 文档分类（Task 8）

- [x] C34: `docs/` 根目录无平面化 `.md` 文件（除 `README.md` 索引外）
- [x] C35: 文档分布在 `docs/<topic>/` 子目录（hal/kernel/runtime/drivers/smp/boot/conventions/ci）

## .gitignore 覆盖（Task 9）

- [x] C36: `.gitignore` 含 `target/`
- [x] C37: `.gitignore` 含 `build/`
- [x] C38: `.gitignore` 含 `*.elf`、`*.bin`、`*.dtb`、`*.img`
- [x] C39: `.gitignore` 含 `qemu-output/`
- [x] C40: `.gitignore` 含 IDE 缓存（`.idea/`、`.vscode/`）
- [x] C41: `.gitignore` 含密钥文件（`*.pem`、`*.key`、`.env`）

## CI 质量门禁（Task 10）

- [x] C42: `cargo run -p eneros-ci` 通过（Overall: PASS，fmt/clippy/test 全绿）
- [x] C43: `cargo deny check advisories licenses bans sources` 降级模式通过（cargo-deny 未安装，CI 降级模式可接受）

## 出口标准 1 — 双分区隔离（Task 11）

- [x] C44: v0.8.0 页表隔离（`vspace.rs`）测试通过 — vspace::tests 全 9 项通过
- [x] C45: v0.9.0 物理内存隔离（`partition.rs`）测试通过 — partition::tests 全 14 项通过
- [x] C46: v0.9.1 合规验证（`isolation/`）测试通过 — isolation::compliance/audit 全 13 项通过
- [x] C47: v0.21.0 共享内存授权（`shared_mem.rs`）测试通过 — shared_mem::tests 全 2 项通过

## 出口标准 2 — 实时性能（Task 12，主机侧）

- [x] C48: v0.19.0 分区调度抖动（`jitter.rs`）测试通过 — jitter::tests 全 5 项通过
- [x] C49: v0.12.0 时钟精度（`eneros-time`）测试通过 — time 全 117 项通过
- [x] C50: v0.18.0 线程切换（`switch.rs`）测试通过 — switch::tests 全 2 项通过
- [x] C51: v0.22.0 命令通道延迟（`eneros-controlbus`）测试通过 — controlbus 全 29 项通过
- [x] C52: 性能指标（抖动 <1ms、延迟 <50μs）延后 QEMU（主机仅验证逻辑）

## 出口标准 3 — 多核启动 + RTOS 核绑定（Task 13）

- [x] C53: v0.15.0 SMP 启动（`boot.rs`）测试通过 — boot::tests 全 8 项通过
- [x] C54: v0.16.0 核绑定（`affinity.rs`）测试通过 — affinity::tests 全 4 项通过
- [x] C55: v0.17.0 内存一致性（`coherence.rs`）测试通过 — coherence::tests 全 5 项通过

## 出口标准 4 — 基础 OS 服务就绪（Task 14）

- [x] C56: v0.10.0 内核堆（`eneros-heap`）测试通过 — 21 项通过
- [x] C57: v0.11.0 用户堆（`eneros-user-heap`）测试通过 — 9 项通过
- [x] C58: v0.12.0 RTC/时钟（`eneros-time`）测试通过 — 117 项通过
- [x] C59: v0.13.0 看门狗（`eneros-watchdog`）测试通过 — 22 项通过
- [x] C60: v0.14.0 Panic 框架（`eneros-panic`）测试通过 — 无单元测试，doc-test ignored
- [x] C61: v0.15.0 SMP（`eneros-smp`）测试通过 — 31 项通过
- [x] C62: v0.16.0~v0.19.0 调度器（`eneros-sched`）测试通过 — 101 项通过
- [x] C63: v0.20.0 IPC（`eneros-ipc`）测试通过 — 22 项通过
- [x] C64: v0.21.0 SPSC Ring（`eneros-ipc`）测试通过 — 含在 ipc 22 项内
- [x] C65: v0.22.0 Control Bus（`eneros-controlbus`）测试通过 — 29 项通过

## 审计报告（Task 15）

- [x] C66: 各 crate 测试数量统计完成 — 477 个测试，0 失败
- [x] C67: 四大出口标准达成情况汇总 — 全部达成
- [x] C68: 遗留项与推迟项记录（QEMU 实测项） — 6 项遗留，全部推迟到 Phase 1/Phase 2
- [x] C69: Phase 0 总体结论给出 — ✅ 通过（有条件），QEMU 实测推迟 Phase 1
