//! 业务/运维服务构建配置（蓝图 41）：
//! 1. ELF 链接：强制 4 KiB 页对齐（spawn 前提——与 drv-svc/control-services
//!    同参；rust-lld 默认 64 KiB max-page-size 使段 vaddr 非页对齐且
//!    .bss 尾落在映射页外，QEMU 实测 8 服务全部静默死于首指令）；
//! 2. 嵌入签名配置 bundle（xtask `config compile` 产物）与验签公钥。
//!    产物缺失时服务 fail-closed（无签名 bundle 不提供服务）。
//!
//! 注意：bundle/pubkey 位于包目录之外（仓库根 product/），cargo 对包外
//! 路径的 rerun-if-changed 静默忽略 → 不能依赖它触发重跑；因此**不**
//! 声明任何 rerun-if-changed：每次构建都重跑本脚本读当前产物。

use std::{env, fs, path::Path};

fn main() {
    if env::var("TARGET")
        .map(|t| t == "aarch64-unknown-none")
        .unwrap_or(false)
    {
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=0x1000");
        // separate-code：让代码/数据段分别页对齐（vaddr % 4096 == 0）。
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=separate-code");
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/agents/business-services → 仓库根。
    let root = manifest.join("../../..");
    let bundle = root.join("product/config-bundle.bin");
    let pubkey = root.join("product/config-bundle-pubkey.hex");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("config_bundle.rs");

    // 真实语义：cargo 只在 build 脚本输出变化/其输入变化时重跑脚本——包外
    // 的 bundle/pubkey 若不声明依赖（rerun-if-changed 支持绝对路径，cargo 会
    // 跟踪），改 bundle 后镜像仍嵌旧配置（QEMU 采集实弹实测：旧 points 表
    // 导致点表驱动读 7 个旧点而非 6 个新点 1-6）。显式声明后：
    // bundle/pubkey 变化 → build 脚本重跑 → svc-config 重新内嵌。
    println!("cargo:rerun-if-changed={}", bundle.display());
    println!("cargo:rerun-if-changed={}", pubkey.display());

    let mut body = String::new();
    // 70.5（WP-70.5.6）：station principal 公钥嵌入（SEC1 未压缩点 65B
    // 原始字节——公钥非机密；轮换 = 重构建/换包）。缺文件 = 空常量 =
    // 显式开发模式（旧 48B 无签会话保留；v2 签名面拒绝 NoStationKey）。
    let station = root.join("product/station/station-pubkey.sec1");
    println!("cargo:rerun-if-changed={}", station.display());
    let station_bytes = fs::read(&station).unwrap_or_default();
    body.push_str(&format!(
        "pub const STATION_PUBKEY_SEC1: &[u8] = &{:?};\n",
        station_bytes
    ));
    if bundle.is_file() {
        let bytes = fs::read(&bundle).unwrap_or_default();
        body.push_str(&format!("pub const CONFIG_BUNDLE: &[u8] = &{:?};\n", bytes));
    } else {
        body.push_str("pub const CONFIG_BUNDLE: &[u8] = &[];\n");
    }
    if pubkey.is_file() {
        let hex = fs::read_to_string(&pubkey).unwrap_or_default();
        let hex = hex.trim();
        body.push_str(&format!(
            "pub const CONFIG_BUNDLE_PUBKEY_HEX: &str = \"{hex}\";\n"
        ));
    } else {
        body.push_str("pub const CONFIG_BUNDLE_PUBKEY_HEX: &str = \"\";\n");
    }
    fs::write(&out, body).expect("write config_bundle.rs");
}
