# Tasks — EnerOS v0.9.0 分区内存隔离验证

> **变更ID**：develop-v090-partition-isolation
> **蓝图依据**：`蓝图/phase0.md` §v0.9.0（第 1664–1852 行）
> **原则**：非瓶颈版本，签名可编译（蓝图 §43.2）；最小可工作方案（Karpathy: Simplicity First）

---

# Task 1: 外科手术式修改现有文件

对 v0.8.0 已有文件做最小改动，为 v0.9.0 铺路。

- [x] SubTask 1.1: 修改 `mm/src/vspace.rs` — MmError 新增 `PermissionDenied` 变体 + Display 分支
- [x] SubTask 1.2: 修改 `mm/src/lib.rs` — 新增 `pub mod partition;` `pub mod dma_guard;`
- [x] SubTask 1.3: 创建空文件 `mm/src/partition.rs`、`mm/src/dma_guard.rs`（仅模块文档注释）
- [x] SubTask 1.4: 修改 workspace 根 `Cargo.toml` — version `0.8.0` → `0.9.0`
- [x] SubTask 1.5: 修改 `hal/src/arm64/provider.rs` — `mem()` panic 消息 `v0.9.0` → `v0.10.0`
- [x] SubTask 1.6: 验证 `cargo build -p eneros-mm` 成功（host）

---

# Task 2: 实现 partition.rs

实现物理内存分区与隔离检查。

- [x] SubTask 2.1: 定义 `PaddrRange { pub start: u64, pub end: u64 }`（Clone, Copy, Debug）
- [x] SubTask 2.2: 实现 `PaddrRange::contains(pa) -> bool`
- [x] SubTask 2.3: 实现 `PaddrRange::overlaps(other) -> bool`
- [x] SubTask 2.4: 定义 `Partition` 结构体（id, name, vspace, allowed_phys[8], quota, used）
- [x] SubTask 2.5: 实现 `Partition::new(id, name, vspace, quota) -> Self`（allowed_phys 初始化为空）
- [x] SubTask 2.6: 实现 `Partition::add_phys_range(range)` — 添加物理区间到首个空槽
- [x] SubTask 2.7: 实现 `Partition::check_access(pa, size) -> Result<(), MmError>` — 区间检查 + 配额检查
- [x] SubTask 2.8: 实现 `Partition::is_isolated_from(other) -> bool` — 任意区间重叠则 false
- [x] SubTask 2.9: 实现 `Partition::alloc_phys(size) -> Result<u64, MmError>` — bump 分配 + used 递增
- [x] SubTask 2.10: 实现 `Partition::free_phys(pa, size)` — used 递减（记账式）
- [x] SubTask 2.11: 添加 `#[cfg(test)] mod tests` — PaddrRange contains/overlaps、check_access 三路径、is_isolated_from、alloc/free

---

# Task 3: 实现 dma_guard.rs

实现 DMA 保护域与 DmaGuard trait。

- [x] SubTask 3.1: 定义 `DeviceId(pub u32)`（Clone, Copy, Debug, PartialEq, Eq）
- [x] SubTask 3.2: 定义 `DmaDomain { owner_partition: u32, allowed_phys: PaddrRange }`（Clone, Copy, Debug）
- [x] SubTask 3.3: 定义 `DmaGuard` trait — `authorize(&self, dev, range) -> Result<(), MmError>` + `check(&self, dev, pa) -> Result<(), MmError>`
- [x] SubTask 3.4: 定义 `SmmuGuard { domains: [Option<DmaDomain>; 16] }`
- [x] SubTask 3.5: 实现 `SmmuGuard::new() -> Self`
- [x] SubTask 3.6: 实现 `DmaGuard::authorize` for SmmuGuard — 写入首个空槽（stub，不配置硬件）
- [x] SubTask 3.7: 实现 `DmaGuard::check` for SmmuGuard — 线性扫描 domains，匹配 owner_partition + allowed_phys
- [x] SubTask 3.8: 添加 `#[cfg(test)] mod tests` — authorize + check 已授权/未授权

---

# Task 4: 编写单元测试

验证隔离逻辑正确性。

- [x] SubTask 4.1: 验证 `cargo test -p eneros-mm` 通过（含 partition + dma_guard 新测试）
- [x] SubTask 4.2: 验证 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归）
- [x] SubTask 4.3: 验证 `cargo test -p eneros-hal --features mock` 通过（hal 回归）

---

# Task 5: 集成到 CI / Makefile

更新版本号与构建配置。

- [x] SubTask 5.1: 修改 `.github/workflows/ci.yml` — 版本标识 v0.8.0 → v0.9.0，注释更新
- [x] SubTask 5.2: 修改 `Makefile` — VERSION 0.8.0 → 0.9.0
- [x] SubTask 5.3: 修改 `ci/src/gate.rs` — 注释更新说明 v0.9.0 partition/dma_guard
- [x] SubTask 5.4: 验证 `cargo fmt --all -- --check` 通过
- [x] SubTask 5.5: 验证 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] SubTask 5.6: 验证交叉编译 `cargo build -p eneros-mm --target aarch64-unknown-none` 通过

---

# Task 6: 编写文档

交付两份技术文档。

- [x] SubTask 6.1: 创建 `docs/partition-isolation-design.md`《分区隔离设计》
  - 分区模型概述（Partition/PaddrRange）
  - 物理区间授权（allowed_phys[8]）
  - 访问检查流程（check_access：区间检查→配额检查）
  - 隔离判断逻辑（is_isolated_from：区间重叠检测）
  - 配额管理（quota/used/alloc_phys/free_phys）
  - QEMU virt 双分区示例配置
- [x] SubTask 6.2: 创建 `docs/dma-protection-guide.md`《DMA 保护方案》
  - DMA 威胁模型（设备绕过 CPU 隔离）
  - SMMU/IOMMU 概述
  - DmaGuard trait 接口
  - SmmuGuard 实现说明（authorize stub / check 软件检查）
  - DmaDomain 与 DeviceId
  - 与 seL4 capability 的关系

---

# Task 7: 验证与收尾

全量验证。

- [x] SubTask 7.1: `cargo fmt --all -- --check`
- [x] SubTask 7.2: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
- [x] SubTask 7.3: `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings`
- [x] SubTask 7.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
- [x] SubTask 7.5: `cargo test -p eneros-hal --features mock`
- [x] SubTask 7.6: `cargo test -p eneros-mm`
- [x] SubTask 7.7: `cargo deny check advisories licenses bans sources`
- [x] SubTask 7.8: 交叉编译全部 crate 到 aarch64-unknown-none（kernel/runtime/board/sel4-sys/hello/hal/mm）
- [x] SubTask 7.9: 确认 `git status` 无垃圾文件
- [x] SubTask 7.10: 更新 checklist.md

---

# Task Dependencies

- Task 2/3 依赖 Task 1（MmError 新增变体 + 模块声明）
- Task 4 依赖 Task 2/3（测试）
- Task 5 依赖 Task 4（CI 集成）
- Task 6 可与 Task 2/3/4/5 并行（文档独立）
- Task 7 依赖全部前序
