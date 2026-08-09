//! build.rs — 编译 gmssl_helpers.c 并链接系统 OpenSSL 3 libcrypto。
//!
//! 依赖 pkg-config 发现 `libcrypto.pc`（`libssl-dev` 提供）。
//! 失败即构建失败（fail-closed）：binding 必须链接 vetted 系统库，
//! 不允许静默退化为无算法可用或自包 C 源码。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/gmssl_helpers.c");
    pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("libcrypto")
        .expect("libcrypto >= 3.0 required (OpenSSL 3 SM2/SM3/SM4; install libssl-dev)");
    cc::Build::new()
        .file("src/gmssl_helpers.c")
        .warnings(true)
        .compile("gmssl_helpers");
}
