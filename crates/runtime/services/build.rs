//! 服务 ELF 链接配置：强制 4 KiB 页对齐（spawn 前提）。
//!
//! 服务段（PT_LOAD）的 vaddr 必须页对齐：root-task 的 `configure_service`
//! 按页（4 KiB）映射 ELF 段（`seg.vaddr % PAGE_SIZE == 0` 否则
//! `SpawnError::PlanMismatch`，fail-closed）。rust-lld 默认
//! `max-page-size=0x10000`（64 KiB），段 vaddr 落在 0x...080/0x...690 等
//! 非页对齐地址 → spawn 失败（21-A 遗留缺陷，C2/BOOT-007 迭代修复）。
//! `-z max-page-size=0x1000` 让链接器把段对齐到 4 KiB 页粒度。
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
        // 注：服务 ELF 链接在 0x200000（aarch64-unknown-none 默认），
        // root-task 把服务映射到同一虚拟地址区域（VSPACE_BASE=0x200000，
        // 与链接一致）——ET_EXEC 固定地址代码的绝对引用才能解析。
    }
}
