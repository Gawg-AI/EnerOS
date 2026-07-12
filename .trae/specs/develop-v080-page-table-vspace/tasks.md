# Tasks — EnerOS v0.8.0 页表管理与地址空间

> **变更ID**：develop-v080-page-table-vspace
> **蓝图依据**：`蓝图/phase0.md` §v0.8.0（第 1443–1661 行）
> **原则**：非瓶颈版本，trait/struct 签名必须可编译（蓝图 §43.2）

---

# Task 1: 创建 mm crate 骨架

新建 mm crate，注册到 workspace，验证 host 构建。

- [x] SubTask 1.1: 创建 `mm/Cargo.toml`
  - `[package]` name="eneros-mm", version="0.8.0", edition="2021"
  - `[dependencies]` eneros-hal = { path = "../hal" }
  - `#![cfg_attr(not(test), no_std)]` 支持模式
- [x] SubTask 1.2: 创建 `mm/src/lib.rs`
  - 模块级文档注释
  - `pub mod page_table;`
  - `pub mod vregion;`
  - `pub mod vspace;`
  - `#![cfg_attr(not(test), no_std)]`
- [x] SubTask 1.3: 创建空文件 `mm/src/page_table.rs`、`vregion.rs`、`vspace.rs`（仅模块文档注释）
- [x] SubTask 1.4: 修改 workspace 根 `Cargo.toml`
  - members 添加 `"mm"`
  - version `0.7.0` → `0.8.0`
- [x] SubTask 1.5: 验证 `cargo build -p eneros-mm` 成功（host）

---

# Task 2: 实现 PageTable（mm/src/page_table.rs）

实现 ARM64 四级页表核心数据结构与操作。

- [x] SubTask 2.1: 定义页表常量
  - `PAGE_SIZE: u64 = 4096`
  - `TABLE_ENTRIES: usize = 512`
- [x] SubTask 2.2: 定义 PTE 位标志常量
  - `PTE_VALID: u64 = 1 << 0`
  - `PTE_TABLE: u64 = 1 << 1`
  - `PTE_AF: u64 = 1 << 10`
  - `PTE_SH_INNER: u64 = 3 << 8`
  - `PTE_PXN: u64 = 1 << 53`
  - `PTE_XN: u64 = 1 << 54`
  - `MT_NORMAL: u64 = 0 << 2`
  - `MT_DEVICE: u64 = 1 << 2`
- [x] SubTask 2.3: 定义 `Pte(pub u64)` 包装类型（derive Clone, Copy）
- [x] SubTask 2.4: 定义 `PageLevel` 枚举（L0/L1/L2/L3，derive Clone, Copy, Debug）
- [x] SubTask 2.5: 定义 `PageTable` 结构体（`entries: [u64; TABLE_ENTRIES]`）
- [x] SubTask 2.6: 实现 `PageTable::new() -> Self`（const fn，entries 全零）
- [x] SubTask 2.7: 实现 `PageTable::index(level: PageLevel, va: u64) -> usize`
  - 根据 level 从 VA 提取 9 位索引：L0 取 bit[47:39]，L1 取 bit[38:30]，L2 取 bit[29:21]，L3 取 bit[20:12]
  - 公式：`(va >> (39 - level as u8 * 9)) & 0x1FF`
- [x] SubTask 2.8: 实现 `PageTable::make_leaf(pa: u64, flags: MemFlags) -> u64`
  - 基础值 = (pa & !0xFFF) | PTE_VALID | PTE_AF | PTE_SH_INNER
  - 根据 flags.device 选择 MT_DEVICE 或 MT_NORMAL
  - flags.executable == false → 置 PTE_XN
  - flags.writable == false → 置 PTE_PXN
- [x] SubTask 2.9: 实现 `PageTable::make_table(child_pa: u64) -> u64`
  - 返回 (child_pa & !0xFFF) | PTE_VALID | PTE_TABLE
- [x] SubTask 2.10: 添加 `#[cfg(test)] mod tests`
  - PTE 位标志常量验证（PTE_VALID==1, PTE_TABLE==2, PTE_AF==1024 等）
  - PAGE_SIZE == 4096, TABLE_ENTRIES == 512
  - index 计算验证（L3 索引对已知 VA 的正确性）
  - make_leaf 验证（device 标志、XN/PXN 置位）
  - make_table 验证（TABLE 位置位）

---

# Task 3: 实现 Vregion（mm/src/vregion.rs）

实现虚拟内存区域描述。

- [x] SubTask 3.1: 定义 `Backing` 枚举（Identity/Phys(u64)/Demand，derive Clone, Copy, Debug）
- [x] SubTask 3.2: 定义 `Vregion` 结构体（start_va: u64, size: u64, flags: MemFlags, backing: Backing，derive Clone, Copy）
- [x] SubTask 3.3: 实现 `Vregion::new(start_va: u64, size: u64, flags: MemFlags, backing: Backing) -> Self`
- [x] SubTask 3.4: 实现 `Vregion::end_va(&self) -> u64`（返回 start_va + size）
- [x] SubTask 3.5: 实现 `Vregion::contains(&self, va: u64) -> bool`（判断 va 是否在区域内）
- [x] SubTask 3.6: 添加 `#[cfg(test)] mod tests`
  - Backing 枚举变体验证
  - Vregion 构造与 end_va/contains 验证

---

# Task 4: 实现 Vspace 与 AddressSpace trait（mm/src/vspace.rs）

实现虚拟地址空间管理与 AddressSpace trait。

- [x] SubTask 4.1: 定义 `MmError` 枚举（InvalidAddr/NotMapped/AlreadyMapped/OutOfMemory/Misaligned，derive Debug）
- [x] SubTask 4.2: 实现 `MmError` 的 `Display` trait
- [x] SubTask 4.3: 定义 `AddressSpace` trait
  - `fn map(&mut self, va: u64, pa: u64, size: u64, flags: MemFlags) -> Result<(), MmError>`
  - `fn unmap(&mut self, va: u64, size: u64) -> Result<(), MmError>`
  - `fn translate(&self, va: u64) -> Option<u64>`
  - `fn set_flags(&mut self, va: u64, flags: MemFlags) -> Result<(), MmError>`
- [x] SubTask 4.4: 定义 `Vspace` 结构体（root_paddr: u64, asid: u16, regions: [Option<Vregion>; 16]）
- [x] SubTask 4.5: 定义静态页表页池 `static mut PAGE_TABLE_POOL: [PageTable; 64]` 与分配计数器
- [x] SubTask 4.6: 实现 `Vspace::new(root_paddr: u64, asid: u16) -> Self`
- [x] SubTask 4.7: 实现静态页表页分配 `fn alloc_page_table() -> Option<&'static mut PageTable>`
- [x] SubTask 4.8: 实现 `AddressSpace::map` for Vspace
  - 校验 va/pa 4KB 对齐（Misaligned）
  - 遍历 L0→L1→L2→L3，中间表项缺失时 alloc_page_table
  - 检测 AlreadyMapped（L3 已有有效叶子）
  - 写 L3 叶子表项
  - 循环处理 size 范围内所有 4KB 页
  - TLB 刷新（asm! tlbi asid，cfg 门控 aarch64）
- [x] SubTask 4.9: 实现 `AddressSpace::translate` for Vspace
  - 遍历四级页表，返回 L3 叶子的物理地址
  - 未映射返回 None
- [x] SubTask 4.10: 实现 `AddressSpace::unmap` for Vspace
  - 遍历到 L3，清除叶子表项（置零）
  - TLB 刷新
- [x] SubTask 4.11: 实现 `AddressSpace::set_flags` for Vspace
  - 遍历到 L3，用 make_leaf 重写表项（保留 pa，更新 flags）
- [x] SubTask 4.12: 添加 `#[cfg(test)] mod tests`
  - MmError 变体验证
  - Vspace 构造验证
  - 对齐校验（map 未对齐地址返回 Misaligned）

---

# Task 5: 更新 provider.rs 注释

更新 hal crate 的 provider.rs mem() panic 消息。

- [x] SubTask 5.1: 修改 `hal/src/arm64/provider.rs`
  - `mem()` panic 消息从 "v0.8.0" 改为 "v0.9.0"
  - 更新模块文档注释说明 HalMem 适配推迟原因

---

# Task 6: 编写单元测试

Host 端可测试的部分（常量验证、索引计算、结构体构造）；aarch64 内联汇编通过交叉编译验证。

- [x] SubTask 6.1: 验证 `cargo test -p eneros-mm` 通过（page_table/vregion/vspace 测试）
- [x] SubTask 6.2: 验证 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归）
- [x] SubTask 6.3: 验证 `cargo test -p eneros-hal --features mock` 通过（hal 回归）

---

# Task 7: 集成到 CI / Makefile

更新版本号与构建配置。

- [x] SubTask 7.1: 修改 `.github/workflows/ci.yml`
  - 版本标识 v0.7.0 → v0.8.0
  - cross-build 新增 `Build mm crate` 步骤
- [x] SubTask 7.2: 修改 `Makefile`
  - VERSION 0.7.0 → 0.8.0
  - 新增 `mm-build` 目标
  - 新增 `mm-test` 目标
  - help 文本更新
- [x] SubTask 7.3: 修改 `ci/src/gate.rs` — 注释更新说明 v0.8.0 mm crate
- [x] SubTask 7.4: 验证 `cargo fmt --all -- --check` 通过
- [x] SubTask 7.5: 验证 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 7.6: 验证交叉编译 `cargo build -p eneros-mm --target aarch64-unknown-none` 通过

---

# Task 8: 编写文档

交付两份技术文档。

- [x] SubTask 8.1: 创建 `docs/arm64-page-table-design.md`《ARM64 页表设计》
  - 四级页表架构概述（L0 PGD → L1 PUD → L2 PMD → L3 PTE）
  - 48 位 VA 地址分解（9+9+9+9+12）
  - PTE 位域表（VALID/TABLE/AF/SH/PXN/XN/AttrIndex）
  - 索引计算公式与示例
  - make_leaf / make_table 构造逻辑
  - TLB 管理与 ASID 机制
  - 与 ARMv8 ARM D5 章节对应
  - EnerOS 实现说明（静态页表页池）
- [x] SubTask 8.2: 创建 `docs/address-space-layout.md`《地址空间布局》
  - Vspace/Vregion 模型概述
  - AddressSpace trait 接口说明
  - Backing 类型（Identity/Phys/Demand）
  - ASID 分配策略
  - QEMU virt 地址空间布局（RAM/设备/IO 区域）
  - 映射流程（map 遍历→分配页表页→写叶子→TLB 刷新）
  - 使用示例

---

# Task 9: 验证与收尾

全量验证。

- [x] SubTask 9.1: `cargo fmt --all -- --check`
- [x] SubTask 9.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
- [x] SubTask 9.3: `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings`
- [x] SubTask 9.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
- [x] SubTask 9.5: `cargo test -p eneros-hal --features mock`
- [x] SubTask 9.6: `cargo test -p eneros-mm`
- [x] SubTask 9.7: `cargo deny check advisories licenses bans sources`
- [x] SubTask 9.8: 交叉编译全部 crate 到 aarch64-unknown-none（kernel/runtime/board/sel4-sys/hello/hal/mm）
- [x] SubTask 9.9: 确认 `git status` 无垃圾文件
- [x] SubTask 9.10: 更新 checklist.md

---

# Task Dependencies

- Task 2/3/4 依赖 Task 1（crate 骨架）
- Task 4 依赖 Task 2/3（vspace 引用 PageTable/Vregion）
- Task 5 独立（provider.rs 注释更新）
- Task 6 依赖 Task 4（测试）
- Task 7 依赖 Task 6（CI 集成）
- Task 8 可与 Task 4/6/7 并行（文档独立）
- Task 9 依赖全部前序
