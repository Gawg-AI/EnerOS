//! build.rs — 构建/链接 HiGHS 静态库（feature = "highs-ffi"，AI-004）.
//!
//! 当 `highs-ffi` feature 启用时：
//! 1. 检查 `third_party/highs-1.7.2/build/lib/libhighs.a` 是否存在
//! 2. 若不存在，用 cmake + make 构建（需要 cmake、g++、make）
//! 3. 告知 cargo 链接 libhighs.a + libstdc++ + libm
//!
//! # 前置条件
//!
//! - `third_party/highs-1.7.2/` 目录存在（由 `tools/fetch-highs.sh` 获取）
//! - cmake >= 3.18、g++ >= 11、make 已安装
//!
//! # 引用
//!
//! - HiGHS v1.7.2, commit 5ce7a27531a7f4166ee5a8343169a1014febb41a
//! - License: MIT (见 third_party/highs-1.7.2/LICENSE.txt)

// Build scripts legitimately `panic!` on environment misconfiguration
// to fail the build fast with a clear message.
#![allow(clippy::disallowed_macros)]

use std::path::PathBuf;
use std::process::Command;

const HIGHS_VERSION: &str = "1.7.2";
const HIGHS_COMMIT: &str = "5ce7a27531a7f4166ee5a8343169a1014febb41a";

fn main() {
    // 仅在 highs-ffi feature 启用时运行
    let enabled = std::env::var("CARGO_FEATURE_HIGHS_FFI").is_ok();
    if !enabled {
        return;
    }

    // 定位 third_party 目录（相对于 crate 根的 ../../../third_party）
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let third_party = manifest_dir
        .join("..")
        .join("..")
        .join("..")
        .join("third_party");
    let highs_src = third_party.join(format!("highs-{}", HIGHS_VERSION));
    let highs_build = highs_src.join("build");
    let highs_lib = highs_build.join("lib").join("libhighs.a");

    // 检查源码目录
    if !highs_src.exists() {
        panic!(
            "HiGHS source not found at {}\n\
             Run: tools/fetch-highs.sh\n\
             Expected HiGHS v{} (commit {})",
            highs_src.display(),
            HIGHS_VERSION,
            HIGHS_COMMIT
        );
    }

    // 如果静态库不存在，构建它
    if !highs_lib.exists() {
        println!(
            "cargo:warning=Building HiGHS v{} from source (this may take a few minutes)...",
            HIGHS_VERSION
        );

        // 创建构建目录
        std::fs::create_dir_all(&highs_build).expect("failed to create HiGHS build dir");

        // cmake configure
        let cmake_status = Command::new("cmake")
            .arg(&highs_src)
            .arg("-DBUILD_SHARED_LIBS=OFF")
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg("-DFAST_BUILD=ON")
            .current_dir(&highs_build)
            .status()
            .expect("cmake not found — install cmake to build HiGHS");

        if !cmake_status.success() {
            panic!("cmake configure failed for HiGHS");
        }

        // make
        let nproc = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let make_status = Command::new("make")
            .arg(format!("-j{}", nproc))
            .arg("highs")
            .current_dir(&highs_build)
            .status()
            .expect("make not found — install build-essential");

        if !make_status.success() {
            panic!("make build failed for HiGHS");
        }

        if !highs_lib.exists() {
            panic!(
                "HiGHS build completed but libhighs.a not found at {}",
                highs_lib.display()
            );
        }

        println!("cargo:warning=HiGHS v{} built successfully", HIGHS_VERSION);
    }

    // 告知 cargo 链接
    let lib_dir = highs_build.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=highs");

    // HiGHS 是 C++ 库，需要链接 C++ 运行时 + zlib（HiGHS 内部用 zstr 做压缩 I/O）
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=z");

    // 重新运行 build.rs 如果源码变化
    println!(
        "cargo:rerun-if-changed={}",
        highs_src.join("CMakeLists.txt").display()
    );
    println!("cargo:rerun-if-changed={}", highs_lib.display());

    // 记录 pin 信息
    println!(
        "cargo:warning=HiGHS pinned: v{} commit={}",
        HIGHS_VERSION, HIGHS_COMMIT
    );
}
