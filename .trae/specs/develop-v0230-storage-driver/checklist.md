# Checklist — v0.23.0 存储驱动（eMMC/NVMe）

## Crate 骨架（Task 1）

- [x] C1: `crates/drivers/storage/Cargo.toml` 已创建（name=eneros-storage, version=0.23.0, edition=2021）
- [x] C2: `crates/drivers/storage/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C3: 根 `Cargo.toml` members 含 `"crates/drivers/storage"`，workspace 版本 `0.23.0`
- [x] C4: `Makefile` VERSION `0.23.0`，含 `storage-build`/`storage-test` 目标
- [x] C5: `.github/workflows/ci.yml` 版本标识 `v0.23.0` + eneros-storage 交叉编译步骤
- [x] C6: `ci/src/gate.rs` 注释含 v0.23.0 说明
- [x] C7: 新 crate 在 `crates/drivers/storage/` 下（规则 §2.3.1）

## error.rs — StorageError（Task 2）

- [x] C8: `StorageError` 枚举含 8 变体（NotInitialized/Timeout/BadBlock/DmaError/CrcMismatch/OutOfRange/HardwareFault/WriteProtected）
- [x] C9: `is_recoverable()` 实现：Timeout/DmaError → true，其余 → false
- [x] C10: `Display` trait 实现
- [x] C11: 单元测试覆盖每个变体 + is_recoverable + Display

## crc32.rs — CRC32 校验（Task 3）

- [x] C12: `crc32(data: &[u8]) -> u32` 实现（IEEE 802.3 多项式 0xEDB88320）
- [x] C13: 空数据返回 0
- [x] C14: `b"123456789"` 返回 `0xCBF43926`
- [x] C15: 单元测试覆盖空/已知向量/任意数据

## bad_block.rs — BadBlockTable（Task 4）

- [x] C16: `BadBlockTable` 结构含 bad_blocks/reserved_start/reserved_count/next_reserved/total_blocks/last_check_time
- [x] C17: `new(total_blocks, reserved_count)` 构造函数
- [x] C18: `is_bad(block_idx) -> bool` 查询
- [x] C19: `mark_bad(block_idx)` 标记
- [x] C20: `get_replacement(block_idx) -> Result<u64, StorageError>` 替换块分配
- [x] C21: `count()` / `wear_level()` / `remaining_life()` 统计方法
- [x] C22: 单元测试：标记/查询/替换/耗尽/越界/重复标记/统计

## driver/types.rs — 类型定义（Task 5）

- [x] C23: `StorageType` 枚举（Emmc/Nvme/SdCard），derive Clone/Copy/Debug/PartialEq/Eq
- [x] C24: `StorageConfig` 结构含 6 字段，derive Clone/Debug/Default
- [x] C25: `DeviceHealth` 结构含 5 字段，derive Clone/Copy/Debug/Default/PartialEq
- [x] C26: 单元测试：构造 + 字段访问 + Default

## driver/mod.rs — BlockDevice trait（Task 6）

- [x] C27: `BlockDevice` trait 含 7 方法（read_block/write_block/erase_block/block_count/block_size/flush/health_status）
- [x] C28: `StorageMmio` trait 含 read_reg/write_reg（抽象 MMIO 访问）
- [x] C29: `DmaTransfer` trait 含 dma_read/dma_write（抽象 DMA 传输）
- [x] C30: 单元测试：trait 对象可用性（dyn BlockDevice + Vec<dyn BlockDevice>）

## driver/mock.rs — MockBlockDevice（Task 7）

- [x] C31: `MockBlockDevice` 结构含 blocks/bad_block_table/block_size/crc_error_blocks
- [x] C32: `new(block_count, block_size)` 构造函数
- [x] C33: `BlockDevice` trait 完整实现（7 方法全部实现）
- [x] C34: `mark_bad(block_idx)` 公开方法
- [x] C35: `inject_crc_error(block_idx)` 公开方法
- [x] C36: 单元测试：读写往返/越界/坏块/擦除/health/CRC注入/多块

## driver/emmc.rs — eMMC 驱动骨架（Task 8）

- [x] C37: eMMC 寄存器偏移量常量定义（7 个）
- [x] C38: eMMC 命令常量定义（6 个）
- [x] C39: 状态标志位常量定义（7 个）
- [x] C40: `EmmcCmdType` 枚举定义
- [x] C41: `encode_cmd(cmd_type, arg) -> u32` 命令编码函数
- [x] C42: `EmmcDriver` 结构定义
- [x] C43: `EmmcDriver::new(config)` 构造函数
- [x] C44: `BlockDevice` trait 实现（host 返回 NotInitialized，框架完整）
- [x] C45: 单元测试：命令编码/构造/trait 方法

## driver/nvme.rs — NVMe 驱动骨架（Task 9）

- [x] C46: NVMe 寄存器偏移量常量定义（10 个）
- [x] C47: `NvmeDriver` 结构定义
- [x] C48: `NvmeDriver::new(config)` 构造函数
- [x] C49: `BlockDevice` trait 实现（host 返回 NotInitialized，框架完整）
- [x] C50: 单元测试：构造/trait 方法

## driver/dma.rs — DMA 传输抽象（Task 10）

- [x] C51: `DmaBuffer` 结构定义，手动 impl Send
- [x] C52: `MockDmaTransfer` 结构（RAM 后备）
- [x] C53: `DmaTransfer` trait 实现 for MockDmaTransfer
- [x] C54: 单元测试：DMA 读写往返/缓冲区校验

## driver/factory.rs — 工厂函数（Task 11）

- [x] C55: `create_block_device(config) -> Result<Box<dyn BlockDevice>, StorageError>` 实现
- [x] C56: 单元测试：Emmc/Nvme/SdCard 类型创建 + trait 对象 + mock helper

## lib.rs 导出（Task 12）

- [x] C57: `lib.rs` 含 `pub mod error/crc32/bad_block/driver`
- [x] C58: `pub use` 导出全部公开类型与函数
- [x] C59: 文档注释说明 v0.23.0 交付内容

## 文档与配置（Task 13）

- [x] C60: `docs/drivers/storage-driver-design.md` 已创建（~300 行）
- [x] C61: `configs/storage.toml` 已创建（配置模板）
- [x] C62: 文档放 `docs/drivers/` 子目录（规则 §2.3.3）

## 构建与质量（Task 14）

- [x] C63: `cargo fmt --all -- --check` 通过
- [x] C64: `cargo clippy -p eneros-storage --all-targets -- -D warnings` 无 warning
- [x] C65: `cargo test -p eneros-storage` 通过（111 测试，远超 ≥40 目标）
- [x] C66: `cargo build -p eneros-storage --target aarch64-unknown-none` 通过
- [x] C67: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全通过
- [x] C68: `cargo run -p eneros-ci` 通过（Overall: PASS）

## 验收标准（蓝图 §7）

- [x] C69: eMMC/NVMe 块设备可正确初始化（骨架框架完整，init() 方法可用）
- [x] C70: `read_block` / `write_block` 在 MockBlockDevice 上全盘范围内正常工作
- [x] C71: 坏块被正确识别和替换（BadBlockTable 测试通过）
- [x] C72: 越界块索引返回错误而非 panic
- [x] C73: 文档齐全（设计文档 + 配置模板 + 接口文档）
- [x] C74: 出口判定：块设备 trait + MockBlockDevice 验证通过 → 解锁 v0.24.0 文件系统开发
