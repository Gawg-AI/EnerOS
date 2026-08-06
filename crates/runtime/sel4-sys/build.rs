//! build.rs for eneros-sel4-sys (G2 closure / ADR-0023).
//!
//! Build scripts legitimately `panic!` on environment misconfiguration
//! (missing seL4 sources/build dir) to fail the build fast with a clear
//! message. The `disallowed_macros` lint targets library/runtime code;
//! build scripts are exempt.
//!
//! Two modes:
//!
//! 1. **Target build** (`target_os = "none"`, i.e. `aarch64-unknown-none`):
//!    - Locate the pre-built seL4 build output (libsel4.a + generated headers)
//!      produced by `third_party/sel4-build.sh`.
//!    - Compile `shim/sel4_shim.c` against those headers with the same
//!      aarch64 cross-compiler the seL4 build uses.
//!    - Archive the shim into `libeneros_sel4_shim.a`.
//!    - Instruct rustc to link: `eneros_sel4_shim` → `sel4` → `gcc`.
//!    - Set `cfg(sel4_target)` so the Rust side picks the real FFI module.
//!
//! 2. **Host build** (anything else, e.g. `cargo test` on x86_64):
//!    - Do nothing. The Rust side picks the host-stub module. No C compiler
//!      is invoked, no libsel4 is required. This is what lets the workspace
//!      build and test on Windows/Linux without a seL4 toolchain.
//!
//! The seL4 build dir is resolved in this order:
//!    (a) `SEL4_BUILD_DIR` env var (absolute path to `third_party/sel4-build/top`)
//!    (b) `<workspace_root>/third_party/sel4-build/top`
//!    (c) `<crate>/../../../third_party/sel4-build/top`
//!
//! The seL4 source dir (for libsel4 headers) is resolved:
//!    (a) `SEL4_SRC` env var
//!    (b) `$HOME/.cache/sel4-src` (matches sel4-build.sh default)
//!    (c) `<workspace_root>/third_party/sel4-src` (if vendored)
//!
//! If the target build cannot find libsel4.a, it fails with a precise
//! message telling the user to run `third_party/sel4-build.sh` first.

// Build scripts legitimately `panic!` on environment misconfiguration
// to fail the build fast with a clear message. The `disallowed_macros`
// lint targets library/runtime code; build scripts are exempt.
#![allow(clippy::disallowed_macros)]

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // Declare the cfg flags this build.rs sets, so rustc's check-cfg lint
    // doesn't warn about them as "unexpected_cfgs".
    println!("cargo::rustc-check-cfg=cfg(host_build)");
    println!("cargo::rustc-check-cfg=cfg(sel4_target)");

    // Only do real work when targeting a seL4 userland target.
    // `target_os = "none"` is what `aarch64-unknown-none` reports.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_os != "none" {
        // Host build: no linking, no C compiler. Rust picks host stubs.
        println!("cargo:rustc-cfg=host_build");
        println!("cargo:rerun-if-changed=shim/sel4_shim.c");
        println!("cargo:rerun-if-changed=shim/sel4_shim.h");
        return;
    }

    // ---- target build: aarch64-unknown-none (seL4 userland) ----
    if target_arch != "aarch64" {
        panic!(
            "eneros-sel4-sys: target_os=none but target_arch={target_arch}; \
             only aarch64 is supported for seL4 userland (ADR-0023)"
        );
    }

    println!("cargo:rustc-cfg=sel4_target");

    let ws = workspace_root();
    let build_dir = resolve_build_dir(&ws);
    let sel4_src = resolve_sel4_src(&ws);

    // 1. Verify libsel4.a exists.
    let libsel4 = build_dir.join("libsel4/libsel4.a");
    if !libsel4.exists() {
        panic!(
            "eneros-sel4-sys: libsel4.a not found at {}\n\
             → Run `third_party/sel4-build.sh` first (in WSL2/Linux) to build seL4 + libsel4.\n\
             → Or set SEL4_BUILD_DIR to point at an existing build output dir.\n\
             See docs/adr/ADR-0023-sel4-kernel-baseline.md for the build procedure.",
            libsel4.display()
        );
    }

    // 2. Collect libsel4 + generated header include dirs (mirror sel4-top/CMakeLists.txt).
    let includes: Vec<PathBuf> = vec![
        // libsel4 source headers
        sel4_src.join("libsel4/include"),
        sel4_src.join("libsel4/arch_include/arm"),
        sel4_src.join("libsel4/sel4_arch_include/aarch64"),
        sel4_src.join("libsel4/sel4_plat_include/qemu-arm-virt"),
        sel4_src.join("libsel4/mode_include/64"),
        // generated headers (kernel + libsel4)
        build_dir.join("kernel/gen_headers"),
        build_dir.join("kernel/gen_config"),
        build_dir.join("kernel/autoconf"),
        build_dir.join("libsel4/gen_config"),
        build_dir.join("libsel4/autoconf"),
        build_dir.join("libsel4/include"),
        // libsel4 CMake 在 build 目录下生成 sel4_arch_include / arch_include
        // （types_gen.h、invocation.h 等）。C root task 通过 target_link_libraries(sel4)
        // 获得这些目录的传播；本 build.rs 必须显式列出，否则 shim 编译找不到
        // sel4/sel4_arch/types_gen.h。
        build_dir.join("libsel4/sel4_arch_include/aarch64"),
        build_dir.join("libsel4/arch_include/arm"),
    ];

    // 3. Locate the aarch64 cross-compiler.
    //    sel4-build.sh uses xpack-aarch64-none-elf-gcc; honor CROSS_COMPILE
    //    / AARCH64_NONE_ELF_GCC env, else fall back to the common name.
    let cross_prefix = env::var("CROSS_COMPILE")
        .or_else(|_| env::var("AARCH64_NONE_ELF_GCC"))
        .unwrap_or_else(|_| "aarch64-none-elf-".to_string());
    let cc_path = which_cc(&cross_prefix);

    // 4. Compile the shim into a static archive.
    let mut build = cc::Build::new();
    build
        .compiler(&cc_path)
        .file("shim/sel4_shim.c")
        .include("shim")
        .flag("-ffreestanding")
        .flag("-fno-builtin")
        .flag("-fno-pic")
        .flag("-fno-pie")
        .flag("-O2")
        .flag("-nostdlib")
        .flag("-march=armv8-a")
        .flag("-Wno-unused-parameter")
        // Put each function/data in its own section so the linker's --gc-sections
        // can discard unused shim wrappers (e.g. IPC functions when the root task
        // only calls DebugPutChar). Without this, the single shim .o pulls in
        // __sel4_ipc_buffer references even when IPC is never used.
        .flag("-ffunction-sections")
        .flag("-fdata-sections");
    for inc in &includes {
        build.include(inc);
    }
    // Compile to a named archive so the link order is deterministic.
    build.compile("eneros_sel4_shim");

    // 5. Link libsel4.a and libgcc.a AFTER the shim.
    //    The shim calls libsel4 inline functions → shim depends on libsel4
    //    symbols (e.g. seL4_GetBootInfo, seL4_DebugPutChar) and libgcc
    //    (compiler builtins like __divdi3 on aarch64).
    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("libsel4").display()
    );
    println!("cargo:rustc-link-lib=static=sel4");

    // libgcc: ask the cross-compiler where its libgcc.a lives.
    if let Some(libgcc_dir) = find_libgcc(&cc_path) {
        println!("cargo:rustc-link-search=native={}", libgcc_dir.display());
        println!("cargo:rustc-link-lib=static=gcc");
    } else {
        // libgcc may already be in the default linker search path; warn but
        // don't fail — the link step will surface a clear error if a builtin
        // is missing.
        println!(
            "cargo:warning=eneros-sel4-sys: could not locate libgcc.a via `{}`; \
                  relying on default linker search path",
            cc_path
        );
    }

    // 6. Re-run if any input changes.
    println!("cargo:rerun-if-changed=shim/sel4_shim.c");
    println!("cargo:rerun-if-changed=shim/sel4_shim.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SEL4_BUILD_DIR");
    println!("cargo:rerun-if-env-changed=SEL4_SRC");
    println!("cargo:rerun-if-env-changed=CROSS_COMPILE");
    println!("cargo:rerun-if-env-changed=AARCH64_NONE_ELF_GCC");
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/runtime/sel4-sys
    // workspace root = 3 levels up
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest.clone())
}

fn resolve_build_dir(ws: &Path) -> PathBuf {
    if let Ok(dir) = env::var("SEL4_BUILD_DIR") {
        return PathBuf::from(dir);
    }
    let candidate = ws.join("third_party/sel4-build/top");
    if candidate.exists() {
        return candidate;
    }
    // Fall back to a relative path from the crate.
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rel = manifest.join("../../../third_party/sel4-build/top");
    if rel.exists() {
        return rel;
    }
    panic!(
        "eneros-sel4-sys: seL4 build dir not found.\n\
         Set SEL4_BUILD_DIR or run third_party/sel4-build.sh first.\n\
         Looked for: {} and {}",
        candidate.display(),
        rel.display()
    )
}

fn resolve_sel4_src(ws: &Path) -> PathBuf {
    if let Ok(dir) = env::var("SEL4_SRC") {
        return PathBuf::from(dir);
    }
    // Match sel4-build.sh default cache location.
    if let Ok(home) = env::var("HOME") {
        let cached = PathBuf::from(home).join(".cache/sel4-src");
        if cached.exists() {
            return cached;
        }
    }
    let vendored = ws.join("third_party/sel4-src");
    if vendored.exists() {
        return vendored;
    }
    panic!(
        "eneros-sel4-sys: seL4 source not found.\n\
         Set SEL4_SRC to the seL4 kernel source root (the dir containing libsel4/).\n\
         sel4-build.sh expects it at $HOME/.cache/sel4-src by default."
    )
}

fn which_cc(prefix: &str) -> String {
    // Try `which <prefix>gcc` via std; if not found, return the prefixed name
    // and let cc::Build surface the error.
    let candidate = format!("{prefix}gcc");
    if which(&candidate).is_some() {
        return candidate;
    }
    // Fall back: maybe the user has `aarch64-linux-gnu-gcc` on PATH.
    for alt in ["aarch64-linux-gnu-gcc", "aarch64-elf-gcc", "clang"] {
        if which(alt).is_some() {
            return alt.to_string();
        }
    }
    // Last resort: return the preferred name; cc::Build will fail with a
    // clear "compiler not found" message.
    candidate
}

fn which(prog: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let full = dir.join(prog);
        if full.is_file() {
            return Some(full);
        }
        // Windows: try .exe
        let full_exe = dir.join(format!("{prog}.exe"));
        if full_exe.is_file() {
            return Some(full_exe);
        }
    }
    None
}

fn find_libgcc(cc: &str) -> Option<PathBuf> {
    // `aarch64-none-elf-gcc -print-libgcc-file-name` prints the full path.
    let output = std::process::Command::new(cc)
        .arg("-print-libgcc-file-name")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path_str = String::from_utf8_lossy(&output.stdout);
    let path_str = path_str.trim();
    if path_str.is_empty() {
        return None;
    }
    let libgcc = PathBuf::from(path_str);
    if libgcc.is_file() {
        libgcc.parent().map(Path::to_path_buf)
    } else {
        None
    }
}
