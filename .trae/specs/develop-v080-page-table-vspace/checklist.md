# Checklist — EnerOS v0.8.0 页表管理与地址空间

> **变更ID**：develop-v080-page-table-vspace
> **蓝图依据**：`蓝图/phase0.md` §v0.8.0（第 1443–1661 行）

---

## 一、mm crate 骨架

- [x] `mm/Cargo.toml` 存在，name="eneros-mm"，依赖 eneros-hal
- [x] `mm/src/lib.rs` 含 `pub mod page_table/vregion/vspace`，`#![cfg_attr(not(test), no_std)]`
- [x] workspace `Cargo.toml` members 含 `"mm"`，version = 0.8.0
- [x] Host 构建 `cargo build -p eneros-mm` 成功
- [x] v0.5.0~v0.7.0 的 crate 不受影响

## 二、PageTable 实现（page_table.rs）

### 常量
- [x] PAGE_SIZE = 4096, TABLE_ENTRIES = 512
- [x] PTE_VALID = 1<<0, PTE_TABLE = 1<<1, PTE_AF = 1<<10
- [x] PTE_SH_INNER = 3<<8, PTE_PXN = 1<<53, PTE_XN = 1<<54
- [x] MT_NORMAL = 0<<2, MT_DEVICE = 1<<2

### 结构体与方法
- [x] `Pte(pub u64)` 包装类型（Clone, Copy）
- [x] `PageLevel` 枚举（L0/L1/L2/L3，Clone, Copy, Debug）
- [x] `PageTable` 结构体（entries: [u64; 512]）
- [x] `PageTable::new()` const fn
- [x] `PageTable::index(level, va)` — 9 位索引提取
- [x] `PageTable::make_leaf(pa, flags)` — 构造 L3 叶子 PTE
- [x] `PageTable::make_table(child_pa)` — 构造中间表项

## 三、Vregion 实现（vregion.rs）

- [x] `Backing` 枚举（Identity/Phys(u64)/Demand）
- [x] `Vregion` 结构体（start_va/size/flags/backing）
- [x] `Vregion::new()` 构造函数
- [x] `Vregion::end_va()` 返回结束地址
- [x] `Vregion::contains(va)` 区域包含判断

## 四、Vspace 与 AddressSpace trait（vspace.rs）

### 错误类型
- [x] `MmError` 枚举（InvalidAddr/NotMapped/AlreadyMapped/OutOfMemory/Misaligned）
- [x] `MmError` 实现 Display trait

### AddressSpace trait
- [x] `map(&mut self, va, pa, size, flags) -> Result<(), MmError>`
- [x] `unmap(&mut self, va, size) -> Result<(), MmError>`
- [x] `translate(&self, va) -> Option<u64>`
- [x] `set_flags(&mut self, va, flags) -> Result<(), MmError>`

### Vspace 实现
- [x] `Vspace` 结构体（root_paddr/asid/regions[16]）
- [x] 静态页表页池 `PAGE_TABLE_POOL: [PageTable; 64]`
- [x] `alloc_page_table()` 静态分配函数
- [x] `Vspace::new(root_paddr, asid)` 构造函数
- [x] `map` 实现：对齐校验 + 四级遍历 + AlreadyMapped 检测 + TLB 刷新
- [x] `translate` 实现：四级遍历返回物理地址
- [x] `unmap` 实现：清除 L3 叶子 + TLB 刷新
- [x] `set_flags` 实现：重写 L3 叶子表项

## 五、provider.rs 更新

- [x] `mem()` panic 消息从 "v0.8.0" 改为 "v0.9.0"
- [x] 模块文档注释更新说明推迟原因

## 六、单元测试

- [x] page_table.rs 测试：PTE 常量、PAGE_SIZE、index 计算、make_leaf、make_table
- [x] vregion.rs 测试：Backing 变体、Vregion 构造、end_va、contains
- [x] vspace.rs 测试：MmError 变体、Vspace 构造、对齐校验
- [x] `cargo test -p eneros-mm` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归）
- [x] `cargo test -p eneros-hal --features mock` 通过（hal 回归）

## 七、no_std 合规

- [x] mm crate 代码不使用 `std::*`
- [x] `#![cfg_attr(not(test), no_std)]` 模式
- [x] 交叉编译 `cargo build -p eneros-mm --target aarch64-unknown-none` 成功
- [x] asm! 内联汇编用 `#[cfg(target_arch = "aarch64")]` 门控

## 八、CI / Makefile

- [x] `.github/workflows/ci.yml` 版本标识为 v0.8.0
- [x] ci.yml cross-build 新增 mm crate 编译步骤
- [x] `Makefile` VERSION = 0.8.0
- [x] Makefile 新增 `mm-build` / `mm-test` 目标
- [x] `ci/src/gate.rs` 注释更新
- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] `cargo deny check advisories licenses bans sources` 通过

## 九、交叉编译验证

- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-board --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hal --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-mm --target aarch64-unknown-none` 成功

## 十、文档交付

- [x] `docs/arm64-page-table-design.md` 存在
- [x] 《ARM64 页表设计》含四级页表架构
- [x] 《ARM64 页表设计》含 PTE 位域表
- [x] 《ARM64 页表设计》含索引计算公式
- [x] 《ARM64 页表设计》含 TLB/ASID 机制
- [x] `docs/address-space-layout.md` 存在
- [x] 《地址空间布局》含 Vspace/Vregion 模型
- [x] 《地址空间布局》含 AddressSpace trait 接口
- [x] 《地址空间布局》含 QEMU virt 地址空间布局

## 十一、工作区整洁

- [x] `git status` 无 target/ build/ *.elf *.bin *.img *.dtb 被追踪
- [x] 无 IDE 缓存被追踪

## 十二、蓝图合规性

- [x] 蓝图 §43.1：所有 Rust 代码 no_std
- [x] 蓝图 §43.2：非瓶颈版本，签名可编译
- [x] 蓝图 §7.1：虚拟地址映射后可读写（交叉编译验证）
- [x] 蓝图 §7.2：未映射地址访问触发 fault（AF 位设计）
- [x] 蓝图 §7.4：文档齐全
- [x] 蓝图 §7.5：出口判定：页表管理就绪
- [x] 蓝图 §5.1：48 位 VA + 4KB granule
- [x] 蓝图 §5.2：ASID 避免频繁 TLB 全刷
- [x] 蓝图 §5.4：静态页表页池（v0.10.0 后切堆）
- [x] v0.5.0/v0.6.0/v0.7.0 回归兼容
