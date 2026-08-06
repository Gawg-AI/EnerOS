# 架构收敛 — 本地构建/验证命令（Phase 0 固化）

> 适用范围：feature/v0.165.0-protocol-driver-sdk 分支的 07 蓝图架构收敛工作
> 环境约束：sandbox 下 `/home/ahx` 只读（.rustup 无法写入），nightly-2026-04-04 装入 `/tmp/rustup`（tmpfs，session 内有效，重启即失；CI 不受影响）

## 1. host 基线（stable 1.97.1）

```bash
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/15/include"   # littlefs2-sys bindgen 缺 stdbool.h
export RUSTUP_TOOLCHAIN=stable

# 全部 lib（含 no_std lib 的 host 编译）
cargo build --workspace --lib

# 全部测试（裸机 bin 在 test 模式有 #[cfg(not(test))] 保护，可 host 编译）
cargo test --workspace

# clippy
cargo clippy --workspace --lib
```

注意：`cargo build --workspace`（含 bins）在 host 上会失败——`eneros-services`/`eneros-drv-svc` 的 10 个 bin 与 `boot-manager-uefi`/`installer-uefi` 是 aarch64 裸机/UEI 入口（自定义 `_start`），只能交叉编译；本阶段工作包全部为 host 可编译的库代码，不受影响。

## 2. aarch64 cross（nightly-2026-04-04，装于 /tmp）

```bash
export RUSTUP_HOME=/tmp/rustup
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/15/include"

# 抽查裸机 EL0 服务 bin
cargo build -p eneros-services -p eneros-drv-svc --target aarch64-unknown-none
```

## 3. 基线验证记录（2026-08-04）

| 项 | 命令 | 结果 |
|---|---|---|
| host lib | `cargo build --workspace --lib` | ✅ Finished dev |
| host test | `cargo test --workspace` | ✅ 全量 ok（无 FAILED；含既有 ignored） |
| aarch64 cross | `cargo build -p eneros-services -p eneros-drv-svc --target aarch64-unknown-none` | ✅ Finished dev |
| clippy 基线 | `cargo clippy --workspace --lib` | 见下方计数 |

## 4. 每工作包完成后的回归命令

```bash
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/15/include"
RUSTUP_TOOLCHAIN=stable cargo build --workspace --lib
RUSTUP_TOOLCHAIN=stable cargo test --workspace
RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --lib
```
