//! 构建冒烟测试（v0.128.0 Task 4，偏差 D7：纯静态契约检查）。
//!
//! 本文件从集成测试视角对"构建链路交付物"做静态契约断言：
//! 链接脚本导出符号、工具链锁定、交叉目标配置、版本信息注入。
//! 按 D7 决策仅做文件内容/常量断言，禁止嵌套 cargo 动态构建，
//! 避免 target 目录锁死锁。集成测试在 host 上编译运行，可使用 std。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::fs;
use std::path::{Path, PathBuf};

use eneros_microkernel::KERNEL_VERSION;

/// 定位仓库根：自 `CARGO_MANIFEST_DIR`（crates/kernel/microkernel）
/// 沿 ancestors 上溯，取首个含 `rust-toolchain.toml` 的目录。
fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|dir| dir.join("rust-toolchain.toml").is_file())
        .expect("未能在 CARGO_MANIFEST_DIR 祖先目录中找到仓库根 rust-toolchain.toml")
        .to_path_buf()
}

/// BSM1：链接脚本契约——`linker.ld` 存在，含 `ENTRY(_start)`、
/// 8 个导出符号与 .text/.rodata/.data/.bss/.stack 五节区关键字。
#[test]
fn bsm1_linker_script_contract() {
    let ld_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("linker.ld");
    let content = fs::read_to_string(&ld_path)
        .unwrap_or_else(|e| panic!("读取链接脚本 {:?} 失败：{}", ld_path, e));

    // 入口符号契约。
    assert!(
        content.contains("ENTRY(_start)"),
        "linker.ld 缺少 ENTRY(_start) 入口声明"
    );

    // 8 个导出符号契约。
    for sym in [
        "__kernel_start",
        "__kernel_end",
        "__text_start",
        "__rodata_start",
        "__data_start",
        "__bss_start",
        "__bss_end",
        "__stack_top",
    ] {
        assert!(content.contains(sym), "linker.ld 缺少导出符号 {}", sym);
    }

    // 五节区关键字契约。
    for section in [".text", ".rodata", ".data", ".bss", ".stack"] {
        assert!(content.contains(section), "linker.ld 缺少节区 {}", section);
    }
}

/// BSM2：工具链锁定——仓库根 `rust-toolchain.toml` 含
/// `nightly-2026-04-04` 与 `rust-src`（蓝图 §5.2 依赖锁定）。
#[test]
fn bsm2_toolchain_lock() {
    let toml_path = repo_root().join("rust-toolchain.toml");
    let content = fs::read_to_string(&toml_path)
        .unwrap_or_else(|e| panic!("读取工具链锁定 {:?} 失败：{}", toml_path, e));

    assert!(
        content.contains("nightly-2026-04-04"),
        "rust-toolchain.toml 未锁定 nightly-2026-04-04"
    );
    assert!(
        content.contains("rust-src"),
        "rust-toolchain.toml 缺少 rust-src 组件（build-std 必需）"
    );
}

/// BSM3：交叉目标配置——仓库根 `.cargo/config.toml` 含
/// `aarch64-unknown-none` 目标配置。
#[test]
fn bsm3_target_config() {
    let cfg_path = repo_root().join(".cargo").join("config.toml");
    let content = fs::read_to_string(&cfg_path)
        .unwrap_or_else(|e| panic!("读取 cargo 配置 {:?} 失败：{}", cfg_path, e));

    assert!(
        content.contains("aarch64-unknown-none"),
        ".cargo/config.toml 缺少 aarch64-unknown-none 目标配置"
    );
}

/// BSM4：内核版本集成断言（D8/D9 集成视角）——
/// `KERNEL_VERSION.minor == 155`，且 build.rs 注入的
/// `commit` / `build_time` 非空（未注入时回退 "unknown" 亦非空）。
#[test]
fn bsm4_kernel_version_integration() {
    assert_eq!(KERNEL_VERSION.major, 0);
    assert_eq!(KERNEL_VERSION.minor, 155);
    assert_eq!(KERNEL_VERSION.patch, 0);
    assert!(
        !KERNEL_VERSION.commit.is_empty(),
        "KERNEL_VERSION.commit 为空：build.rs GIT_HASH 注入/回退失效"
    );
    assert!(
        !KERNEL_VERSION.build_time.is_empty(),
        "KERNEL_VERSION.build_time 为空：build.rs BUILD_TIME 注入/回退失效"
    );
}
