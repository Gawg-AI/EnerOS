//! WP-37.4：静态 PIE 构建参数注入（仅本 crate）。
//! - `cargo:rustc-flags=-C relocation-model=pic`：PIC 代码生成
//!   （-pie 链接对非 PIC 代码的 ABS64 局部符号重定位会拒绝，
//!   QEMU 实测 rust-lld error）。
//! - `cargo:rustc-link-arg-bin=svc-pie=-pie`：仅 svc-pie bin 链接为
//!   ET_DYN（其余服务保持 ET_EXEC，root-task 亦不受影响）。

fn main() {
    // bin 级链接参数（Cargo 1.77+）：仅 svc-pie 链接为 ET_DYN。
    // PIC 代码生成由 .cargo/config.toml [target.aarch64-unknown-none]
    // rustflags 全局提供（WP-37.4 linker contract 冻结：全 aarch64 代码
    // PIC 生成、链接保持 EXEC，仅 svc-pie 加 -pie；root-task 必须 ET_EXEC
    // 供 elfloader 装载）。
    println!("cargo:rustc-link-arg-bin=svc-pie=-pie");
    println!("cargo:rerun-if-changed=build.rs");
}
