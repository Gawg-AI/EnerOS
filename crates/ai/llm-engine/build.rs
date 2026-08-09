//! build.rs — 构建/链接 llama.cpp 静态库（feature = "llama-cpp"，AI-001）.
//!
//! 当 `llama-cpp` feature 启用时：
//! 1. 检查 `third_party/llama.cpp/build/src/libllama.a` 是否存在
//! 2. 若不存在，用 cmake 构建（关闭 tests/examples/server，仅静态库）
//! 3. 告知 cargo 链接 libllama.a + libggml.a + pthread + m + dl
//!
//! # 前置条件
//!
//! - `third_party/llama.cpp/` 目录存在（pin 到 b4000）
//! - cmake >= 3.14、g++ >= 11、make 已安装
//!
//! # 引用
//!
//! - llama.cpp b4000 (commit c02e5ab2a675c8bc1abc8b1e4cb6a93b26bdcce7)
//! - License: MIT

// Build scripts legitimately `panic!` on environment misconfiguration
// to fail the build fast with a clear message.
#![allow(clippy::disallowed_macros)]

use std::path::PathBuf;
use std::process::Command;

const LLAMA_TAG: &str = "b4000";
const LLAMA_COMMIT: &str = "c02e5ab2a675c8bc1abc8b1e4cb6a93b26bdcce7";

fn main() {
    // 仅在 llama-cpp feature 启用时运行
    let enabled = std::env::var("CARGO_FEATURE_LLAMA_CPP").is_ok();
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
    let llama_src = third_party.join("llama.cpp");
    let llama_build = llama_src.join("build");
    // llama.cpp b4000 的 cmake 输出布局：
    //   build/src/libllama.a
    //   build/ggml/src/libggml.a       (基础后端)
    //   build/ggml/src/libggml-base.a
    //   build/ggml/src/libggml-cpu.a
    let llama_lib = llama_build.join("src").join("libllama.a");

    // 检查源码目录
    if !llama_src.exists() {
        panic!(
            "llama.cpp source not found at {}\n\
             Run: tools/fetch-llama.sh\n\
             Expected llama.cpp tag {} (commit {})",
            llama_src.display(),
            LLAMA_TAG,
            LLAMA_COMMIT,
        );
    }

    // 校验 pinned commit（防止误升级）
    let head = Command::new("git")
        .arg("-C")
        .arg(&llama_src)
        .arg("rev-parse")
        .arg("HEAD")
        .output();
    if let Ok(out) = head {
        if out.status.success() {
            let actual = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if actual != LLAMA_COMMIT {
                panic!(
                    "llama.cpp commit mismatch: expected {}, got {}\n\
                     Run: git -C {} checkout {}",
                    LLAMA_COMMIT,
                    actual,
                    llama_src.display(),
                    LLAMA_COMMIT,
                );
            }
        }
    }

    // 如果静态库不存在，构建它
    if !llama_lib.exists() {
        println!(
            "cargo:info=Building llama.cpp {} from source (this may take several minutes)...",
            LLAMA_TAG
        );

        std::fs::create_dir_all(&llama_build).expect("failed to create llama.cpp build dir");

        // cmake configure — 仅静态库，关闭所有可执行目标。
        // GGML_NATIVE=OFF（官方默认）：禁止 -march=native——否则产物携带
        // 构建机专属指令（如 AVX-VNNI 的 vpdpbusd），在无该特性的运行机
        // 上 SIGILL（21-B A1 实测：Zen3+ 机器跑 Zen4 编译的 libllama.a）。
        let cmake_status = Command::new("cmake")
            .arg(&llama_src)
            .arg("-DBUILD_SHARED_LIBS=OFF")
            .arg("-DLLAMA_BUILD_TESTS=OFF")
            .arg("-DLLAMA_BUILD_EXAMPLES=OFF")
            .arg("-DLLAMA_BUILD_SERVER=OFF")
            .arg("-DGGML_BUILD_NUMBER=0")
            .arg("-DGGML_NATIVE=OFF")
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .current_dir(&llama_build)
            .status()
            .expect("cmake not found — install cmake to build llama.cpp");

        if !cmake_status.success() {
            panic!("cmake configure failed for llama.cpp");
        }

        // make — 只构建 llama 静态库目标（会连带 ggml 依赖）
        let nproc = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let make_status = Command::new("make")
            .arg(format!("-j{}", nproc))
            .arg("llama")
            .current_dir(&llama_build)
            .status()
            .expect("make not found — install build-essential");

        if !make_status.success() {
            panic!("make build failed for llama.cpp");
        }

        if !llama_lib.exists() {
            panic!(
                "llama.cpp build completed but libllama.a not found at {}",
                llama_lib.display()
            );
        }

        println!("cargo:info=llama.cpp {} built successfully", LLAMA_TAG);
    }

    // 告知 cargo 链接搜索路径
    // b4000 布局：build/src/, build/ggml/src/
    println!(
        "cargo:rustc-link-search=native={}",
        llama_build.join("src").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        llama_build.join("ggml").join("src").display()
    );

    // 链接顺序：llama 依赖 ggml（b4000 整体打包为单个 libggml.a）
    println!("cargo:rustc-link-lib=static=llama");
    println!("cargo:rustc-link-lib=static=ggml");

    // llama.cpp 是 C++ 库，依赖：stdc++（C++ 运行时）、pthread（线程池）、m（数学）、dl（动态加载）、gomp（OpenMP，ggml 并行）
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=gomp");

    // 重新运行 build.rs 如果源码变化
    println!(
        "cargo:rerun-if-changed={}",
        llama_src.join("CMakeLists.txt").display()
    );
    println!("cargo:rerun-if-changed={}", llama_lib.display());

    // 记录 pin 信息
    println!(
        "cargo:info=llama.cpp pinned: tag {} commit={}",
        LLAMA_TAG, LLAMA_COMMIT
    );
}
