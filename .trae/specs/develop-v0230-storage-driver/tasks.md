# Tasks — v0.23.0 存储驱动（eMMC/NVMe）

- [x] Task 1: 创建 eneros-storage crate 骨架
  - [x] SubTask 1.1: 创建 `crates/drivers/storage/Cargo.toml`（name=eneros-storage, version=0.23.0, edition=2021）
  - [x] SubTask 1.2: 创建 `crates/drivers/storage/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明）
  - [x] SubTask 1.3: 根 `Cargo.toml` 添加 `"crates/drivers/storage"` 到 workspace members，workspace 版本更新为 `0.23.0`
  - [x] SubTask 1.4: `Makefile` VERSION 更新为 `0.23.0`，添加 `storage-build`/`storage-test` 目标
  - [x] SubTask 1.5: `.github/workflows/ci.yml` 版本标识更新为 `v0.23.0`，添加 eneros-storage 交叉编译步骤
  - [x] SubTask 1.6: `ci/src/gate.rs` 注释含 v0.23.0 说明

- [x] Task 2: 实现 error.rs — StorageError 错误类型
  - [x] SubTask 2.1: 定义 `StorageError` 枚举（8 变体）
  - [x] SubTask 2.2: 实现 `is_recoverable()` 方法
  - [x] SubTask 2.3: 实现 `Display` trait
  - [x] SubTask 2.4: 单元测试：每个变体构造 + is_recoverable 判定 + Display 输出

- [x] Task 3: 实现 crc32.rs — CRC32 校验函数
  - [x] SubTask 3.1: 实现 `crc32(data: &[u8]) -> u32`（IEEE 802.3 多项式 0xEDB88320）
  - [x] SubTask 3.2: 单元测试：空数据返回 0、`b"123456789"` 返回 0xCBF43926、已知向量验证

- [x] Task 4: 实现 bad_block.rs — BadBlockTable 坏块管理
  - [x] SubTask 4.1: 定义 `BadBlockTable` 结构
  - [x] SubTask 4.2: 实现 `new(total_blocks, reserved_count)` 构造函数
  - [x] SubTask 4.3: 实现 `is_bad(block_idx) -> bool` 查询
  - [x] SubTask 4.4: 实现 `mark_bad(block_idx)` 标记坏块
  - [x] SubTask 4.5: 实现 `get_replacement(block_idx) -> Result<u64, StorageError>` 获取替换块
  - [x] SubTask 4.6: 实现 `count()` / `wear_level()` / `remaining_life()` 统计方法
  - [x] SubTask 4.7: 单元测试：标记/查询/替换/耗尽/越界/重复标记/统计

- [x] Task 5: 实现 driver/types.rs — 存储类型定义
  - [x] SubTask 5.1: 定义 `StorageType` 枚举
  - [x] SubTask 5.2: 定义 `StorageConfig` 结构
  - [x] SubTask 5.3: 定义 `DeviceHealth` 结构
  - [x] SubTask 5.4: 单元测试：构造 + 字段访问 + Default

- [x] Task 6: 实现 driver/mod.rs — BlockDevice trait
  - [x] SubTask 6.1: 定义 `BlockDevice` trait（7 方法）
  - [x] SubTask 6.2: 定义 `StorageMmio` trait
  - [x] SubTask 6.3: 定义 `DmaTransfer` trait
  - [x] SubTask 6.4: 单元测试：trait 对象可用性

- [x] Task 7: 实现 driver/mock.rs — MockBlockDevice
  - [x] SubTask 7.1: 定义 `MockBlockDevice` 结构
  - [x] SubTask 7.2: 实现 `new(block_count, block_size)` 构造函数
  - [x] SubTask 7.3: 实现 `BlockDevice` trait（7 方法全部实现）
  - [x] SubTask 7.4: 实现 `mark_bad(block_idx)` 公开方法
  - [x] SubTask 7.5: 实现 `inject_crc_error(block_idx)` 公开方法
  - [x] SubTask 7.6: 单元测试：读写往返/越界/坏块/擦除/health/CRC注入/多块

- [x] Task 8: 实现 driver/emmc.rs — eMMC 驱动骨架
  - [x] SubTask 8.1: 定义 eMMC 寄存器偏移量常量（7 个）
  - [x] SubTask 8.2: 定义 eMMC 命令常量（6 个）
  - [x] SubTask 8.3: 定义状态标志位常量（7 个）
  - [x] SubTask 8.4: 定义 `EmmcCmdType` 枚举
  - [x] SubTask 8.5: 实现 `encode_cmd(cmd_type, arg) -> u32` 命令编码函数
  - [x] SubTask 8.6: 定义 `EmmcDriver` 结构
  - [x] SubTask 8.7: 实现 `EmmcDriver::new(config)` 构造函数
  - [x] SubTask 8.8: 实现 `BlockDevice` trait
  - [x] SubTask 8.9: 单元测试：命令编码/构造/trait 方法

- [x] Task 9: 实现 driver/nvme.rs — NVMe 驱动骨架
  - [x] SubTask 9.1: 定义 NVMe 寄存器偏移量常量（10 个）
  - [x] SubTask 9.2: 定义 `NvmeDriver` 结构
  - [x] SubTask 9.3: 实现 `NvmeDriver::new(config)` 构造函数
  - [x] SubTask 9.4: 实现 `BlockDevice` trait
  - [x] SubTask 9.5: 单元测试：构造/trait 方法

- [x] Task 10: 实现 driver/dma.rs — DMA 传输抽象
  - [x] SubTask 10.1: 定义 `DmaBuffer` 结构，手动 impl Send
  - [x] SubTask 10.2: 定义 `MockDmaTransfer` 结构
  - [x] SubTask 10.3: 实现 `DmaTransfer` trait for MockDmaTransfer
  - [x] SubTask 10.4: 单元测试：DMA 读写往返/缓冲区校验

- [x] Task 11: 实现 driver/factory.rs — 块设备工厂函数
  - [x] SubTask 11.1: 实现 `create_block_device(config)` 工厂函数
  - [x] SubTask 11.2: 单元测试：Emmc/Nvme/SdCard 类型创建

- [x] Task 12: lib.rs 导出与文档注释
  - [x] SubTask 12.1: `lib.rs` 声明全部 pub mod
  - [x] SubTask 12.2: `pub use` 导出全部公开类型与函数
  - [x] SubTask 12.3: 文档注释说明 v0.23.0 交付内容

- [x] Task 13: 文档与配置
  - [x] SubTask 13.1: 创建 `docs/drivers/storage-driver-design.md`（~300 行）
  - [x] SubTask 13.2: 创建 `configs/storage.toml`（配置模板）

- [x] Task 14: 构建与质量验证
  - [x] SubTask 14.1: `cargo fmt --all -- --check` 通过
  - [x] SubTask 14.2: `cargo clippy -p eneros-storage --all-targets -- -D warnings` 无 warning
  - [x] SubTask 14.3: `cargo test -p eneros-storage` 通过（111 测试，远超 ≥40 目标）
  - [x] SubTask 14.4: `cargo build -p eneros-storage --target aarch64-unknown-none` 通过
  - [x] SubTask 14.5: workspace 回归 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全通过
  - [x] SubTask 14.6: `cargo run -p eneros-ci` 通过（Overall: PASS）

# Task Dependencies

- Task 1（crate 骨架）独立，最先执行
- Task 2（error）独立，可与 Task 1 并行
- Task 3（crc32）独立，可与 Task 1/2 并行
- Task 4（bad_block）依赖 Task 2（StorageError）
- Task 5（types）依赖 Task 2（StorageError）
- Task 6（BlockDevice trait）依赖 Task 2/5
- Task 7（mock）依赖 Task 4/5/6
- Task 8（emmc）依赖 Task 4/5/6
- Task 9（nvme）依赖 Task 4/5/6，可与 Task 8 并行
- Task 10（dma）依赖 Task 2/5
- Task 11（factory）依赖 Task 7/8/9
- Task 12（lib 导出）依赖全部前序
- Task 13（文档配置）依赖 Task 12
- Task 14（验证）依赖全部前序

# Notes

- 遵循 no_std：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 遵循 §2.3.1：crate 放 `crates/drivers/storage/` 下
- 遵循 §2.3.3：文档放 `docs/drivers/` 下
- eMMC/NVMe 硬件驱动为结构骨架，host 侧返回 NotInitialized，aarch64 可编译
- MockBlockDevice 是主机测试和 v0.24.0 FS 开发的主要实现
- 不修改现有 crate 代码（surgical changes）
