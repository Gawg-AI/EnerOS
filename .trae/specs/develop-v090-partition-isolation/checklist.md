# Checklist — EnerOS v0.9.0 分区内存隔离验证

> **变更ID**：develop-v090-partition-isolation
> **蓝图依据**：`蓝图/phase0.md` §v0.9.0（第 1664–1852 行）

---

## 一、外科手术式修改

- [x] `mm/src/vspace.rs` MmError 新增 `PermissionDenied` 变体
- [x] `mm/src/vspace.rs` MmError Display 新增 PermissionDenied 分支
- [x] `mm/src/lib.rs` 新增 `pub mod partition;`
- [x] `mm/src/lib.rs` 新增 `pub mod dma_guard;`
- [x] workspace `Cargo.toml` version = 0.9.0
- [x] `hal/src/arm64/provider.rs` mem() panic 消息为 v0.10.0
- [x] `cargo build -p eneros-mm` 成功

## 二、partition.rs 实现

### PaddrRange
- [x] `PaddrRange { start: u64, end: u64 }`（Clone, Copy, Debug）
- [x] `contains(pa) -> bool`
- [x] `overlaps(other) -> bool`

### Partition
- [x] `Partition` 结构体（id/name/vspace/allowed_phys[8]/quota/used）
- [x] `Partition::new(id, name, vspace, quota)` 构造函数
- [x] `add_phys_range(range)` 添加物理区间
- [x] `check_access(pa, size)` — 区间检查 + 配额检查
- [x] `is_isolated_from(other)` — 区间重叠检测
- [x] `alloc_phys(size)` — bump 分配 + used 递增
- [x] `free_phys(pa, size)` — used 递减

## 三、dma_guard.rs 实现

- [x] `DeviceId(pub u32)`（Clone, Copy, Debug, PartialEq, Eq）
- [x] `DmaDomain { owner_partition, allowed_phys }`（Clone, Copy, Debug）
- [x] `DmaGuard` trait（authorize + check）
- [x] `SmmuGuard { domains: [Option<DmaDomain>; 16] }`
- [x] `SmmuGuard::new()` 构造函数
- [x] `DmaGuard::authorize` for SmmuGuard（stub，写入空槽）
- [x] `DmaGuard::check` for SmmuGuard（线性扫描）

## 四、单元测试

- [x] PaddrRange contains/overlaps 测试
- [x] check_access 授权范围内 → Ok
- [x] check_access 超出范围 → PermissionDenied
- [x] check_access 超出配额 → OutOfMemory
- [x] is_isolated_from 不重叠 → true
- [x] is_isolated_from 重叠 → false
- [x] alloc_phys 配额不足 → OutOfMemory
- [x] DmaGuard check 已授权 → Ok
- [x] DmaGuard check 未授权 → PermissionDenied
- [x] `cargo test -p eneros-mm` 通过
- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归）
- [x] `cargo test -p eneros-hal --features mock` 通过（hal 回归）

## 五、CI / Makefile

- [x] `.github/workflows/ci.yml` 版本标识为 v0.9.0
- [x] `Makefile` VERSION = 0.9.0
- [x] `ci/src/gate.rs` 注释更新
- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 通过
- [x] `cargo clippy -p eneros-hal --features mock --all-targets -- -D warnings` 通过
- [x] `cargo deny check advisories licenses bans sources` 通过

## 六、交叉编译验证

- [x] `cargo build -p eneros-kernel --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-runtime --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-board --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-sel4-sys --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hello --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-hal --target aarch64-unknown-none` 成功
- [x] `cargo build -p eneros-mm --target aarch64-unknown-none` 成功

## 七、文档交付

- [x] `docs/partition-isolation-design.md` 存在
- [x] 《分区隔离设计》含分区模型
- [x] 《分区隔离设计》含访问检查流程
- [x] 《分区隔离设计》含隔离判断逻辑
- [x] 《分区隔离设计》含 QEMU virt 示例配置
- [x] `docs/dma-protection-guide.md` 存在
- [x] 《DMA 保护方案》含威胁模型
- [x] 《DMA 保护方案》含 DmaGuard trait 接口
- [x] 《DMA 保护方案》含 SmmuGuard 实现说明

## 八、工作区整洁

- [x] `git status` 无 target/ build/ *.elf *.bin *.img *.dtb 被追踪
- [x] 无 IDE 缓存被追踪

## 九、蓝图合规性

- [x] 蓝图 §43.1：所有 Rust 代码 no_std
- [x] 蓝图 §43.2：非瓶颈版本，签名可编译
- [x] 蓝图 §7.1：分区 A 无法读写分区 B 内存（check_access 拒绝）
- [x] 蓝图 §7.2：DMA 越权被阻止（DmaGuard::check 拒绝）
- [x] 蓝图 §7.4：文档齐全
- [x] 蓝图 §7.5：出口判定：双分区隔离达成（部分）
- [x] v0.5.0~v0.8.0 回归兼容
