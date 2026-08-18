//! root-task 链接配置：作为 seL4 rootserver，ELF 入口必须是 `_start`
//! （seL4 kernel 以 ELF entry 创建初始线程）。Rust 默认把 `main` 设为
//! 入口，这里用 `-e _start` 强制覆盖（rust-lld）。
//!
//! 同时强制 4 KiB 页对齐（`-z max-page-size=0x1000`）：plan_spawn 解析
//! root 模块（bundle 中的 root-task ELF）时要求段 vaddr 页对齐
//! （`seg.vaddr % PAGE_SIZE == 0`，否则 `SpawnError::PlanMismatch`）；
//! rust-lld 默认 64 KiB max-page-size 导致非页对齐（C2/BOOT-007 迭代修复）。
fn main() {
    // 仅对裸机 target 注入（host 构建用于测试，不需强制入口）。
    if std::env::var("TARGET")
        .map(|t| t == "aarch64-unknown-none")
        .unwrap_or(false)
    {
        println!("cargo:rustc-link-arg=-e");
        println!("cargo:rustc-link-arg=_start");
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=0x1000");
        // separate-code：让代码/数据段分别页对齐（vaddr % 4096 == 0）。
        // 实测仅 max-page-size 不足以对齐段 vaddr（.text 起始偏移仍
        // 非页对齐）；separate-code 强制每个 PT_LOAD 页对齐。
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=separate-code");
        // WP-37.4（rootserver 装载对齐敏感）：强制 .text 起始 0x210000
        // （64K 边界）——QEMU 实测 text 起始 0x208000（0x8000 偏移）时
        // text 尾部页内容异常（EC=0 user exception）；64K 对齐起始布局
        // 正常。确定性锚（非布局碰运气）：37 记录 §16 登记 kernel 侧
        // 根因调查（42/43）。
        println!("cargo:rustc-link-arg=--section-start=.text=0x210000");
    }
}
