//! 驱动/存储服务 ELF 链接配置：强制 4 KiB 页对齐（spawn 前提）。
//!
//! 与 eneros-services 同因：root-task 的 `configure_service` 按页映射
//! ELF 段（`seg.vaddr % PAGE_SIZE == 0`），rust-lld 默认 64 KiB
//! max-page-size 导致段 vaddr 非页对齐 → `SpawnError::PlanMismatch`
//! （21-A 遗留缺陷，C2/BOOT-007 迭代修复）。`-z max-page-size=0x1000`
//! 让链接器把段对齐到 4 KiB 页粒度。
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
        // 注：与 eneros-services 相同，链接在 0x200000，root-task 映射
        // 到同一虚拟地址区域（VSPACE_BASE=0x200000）。
    }
}
