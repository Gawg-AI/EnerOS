//! 控制服务 ELF 链接配置（蓝图 40：svc-telemetry/device-agent/control-dispatch/ai-worker/energy-agent）：强制 4 KiB 页对齐（spawn 前提）。
//!
//! 与 eneros-drv-svc / eneros-services 同因：root-task 的
//! `configure_service` 按页映射 ELF 段（`seg.vaddr % PAGE_SIZE == 0`），
//! rust-lld 默认 64 KiB max-page-size 导致段 vaddr 非页对齐——39 收尾
//! 复验实测：7 个协议服务因此静默死于首指令（readiness 永不报告，
//! 全部超时降级）。与 drv-svc/build.rs 完全同参修复。
fn main() {
    if std::env::var("TARGET")
        .map(|t| t == "aarch64-unknown-none")
        .unwrap_or(false)
    {
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=0x1000");
        // separate-code：让代码/数据段分别页对齐（vaddr % 4096 == 0）。
        // 实测仅 max-page-size 不足以对齐段 vaddr（.text 起始偏移仍
        // 非页对齐）；separate-code 强制每个 PT_LOAD 页对齐。
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=separate-code");
    }
}
