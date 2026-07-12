# v0.23.0 存储驱动（eMMC/NVMe）Spec

## Why

Phase 1 的所有持久化数据（配置、模型权重、日志、时序数据）都需要可靠的块设备底层。v0.23.0 是 P1-A 存储与文件系统的第一个版本，提供统一的 `BlockDevice` trait 接口，解锁 v0.24.0 文件系统开发。

## What Changes

- 新增 `crates/drivers/storage/` crate（`eneros-storage`），实现块设备驱动抽象层
- 定义 `BlockDevice` trait — 统一的块设备读写接口（read/write/erase/flush/health）
- 实现 `BadBlockTable` — 坏块管理（增删查改 + 预留块替换），纯逻辑可全测试
- 实现 `StorageError` — 错误类型 + `is_recoverable()` 判定
- 实现 `MockBlockDevice` — RAM 后备的 mock 块设备，供主机测试和 v0.24.0 FS 开发使用
- 实现 `EmmcDriver` / `NvmeDriver` — 结构骨架，含寄存器常量和命令定义，MMIO 操作通过 `StorageMmio` trait 抽象（aarch64 用 volatile 读写，host 用 mock）
- 实现 `DmaTransfer` trait — DMA 传输抽象（mock 实现）
- 实现 CRC32 校验函数 — 纯逻辑
- 新增文档 `docs/drivers/storage-driver-design.md`
- 新增配置模板 `configs/storage.toml`
- 更新根 `Cargo.toml`、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs` 版本标识

## Impact

- Affected specs: v0.24.0（文件系统）依赖本版本的 `BlockDevice` trait
- Affected code: 新增 `crates/drivers/storage/`，不修改现有 crate
- 前置依赖：v0.7.0（HAL MMIO）、v0.11.0（用户堆）、v0.12.0（RTC 时间戳）

## ADDED Requirements

### Requirement: BlockDevice Trait

系统 SHALL 提供统一的 `BlockDevice` trait，抽象块设备的读写操作，使文件系统层无需关心底层是 eMMC、NVMe 还是 mock 设备。

#### Scenario: 读取块成功
- **WHEN** 调用 `read_block(block_idx, buf)` 且块索引合法、非坏块、DMA 传输成功
- **THEN** 返回 `Ok(())`，`buf` 填充块数据

#### Scenario: 读取越界块
- **WHEN** 调用 `read_block(block_idx, buf)` 且 `block_idx >= block_count()`
- **THEN** 返回 `Err(StorageError::OutOfRange { block_idx, max })`

#### Scenario: 读取坏块
- **WHEN** 调用 `read_block(block_idx, buf)` 且该块在坏块表中
- **THEN** 返回 `Err(StorageError::BadBlock { block_idx })`

#### Scenario: 写入坏块自动替换
- **WHEN** 调用 `write_block(block_idx, buf)` 且该块已标记为坏块
- **THEN** 系统自动分配预留块替换，写入成功返回 `Ok(())`

### Requirement: BadBlockTable 坏块管理

系统 SHALL 提供坏块表数据结构，支持坏块的标记、查询、替换和磨损均衡统计。

#### Scenario: 标记坏块
- **WHEN** 调用 `mark_bad(block_idx)` 且块索引合法
- **THEN** 该块被加入坏块列表，后续 `is_bad(block_idx)` 返回 `true`

#### Scenario: 获取替换块
- **WHEN** 调用 `get_replacement(block_idx)` 且有可用预留块
- **THEN** 返回 `Ok(replacement_block_idx)`

#### Scenario: 预留块耗尽
- **WHEN** 调用 `get_replacement(block_idx)` 且预留块已全部用完
- **THEN** 返回 `Err(StorageError::HardwareFault)`

### Requirement: MockBlockDevice 主机测试

系统 SHALL 提供 RAM 后备的 `MockBlockDevice`，实现 `BlockDevice` trait，用于主机测试和 v0.24.0 文件系统开发。

#### Scenario: Mock 读写往返
- **WHEN** 创建 `MockBlockDevice`（1024 块 × 512 字节），写入块 0 数据，再读取块 0
- **THEN** 读回数据与写入数据一致

#### Scenario: Mock 坏块注入
- **WHEN** 调用 `mock.mark_bad(5)` 后读取块 5
- **THEN** 返回 `Err(StorageError::BadBlock { block_idx: 5 })`

### Requirement: StorageError 错误处理

系统 SHALL 定义 `StorageError` 枚举，覆盖所有存储操作错误场景，并提供 `is_recoverable()` 方法区分可恢复错误（超时/DMA 错误）和不可恢复错误（坏块/硬件故障）。

#### Scenario: 可恢复错误判定
- **WHEN** 错误为 `Timeout` 或 `DmaError`
- **THEN** `is_recoverable()` 返回 `true`

#### Scenario: 不可恢复错误判定
- **WHEN** 错误为 `BadBlock`、`CrcMismatch`、`HardwareFault`、`OutOfRange`、`WriteProtected`、`NotInitialized`
- **THEN** `is_recoverable()` 返回 `false`

### Requirement: EmmcDriver / NvmeDriver 结构骨架

系统 SHALL 提供 eMMC 和 NVMe 驱动的结构骨架，包含寄存器常量定义、命令编码和 `BlockDevice` trait 实现。实际 MMIO 操作通过 `StorageMmio` trait 抽象，aarch64 目标使用 volatile 读写，主机测试使用 mock。

#### Scenario: eMMC 命令编码
- **WHEN** 调用 `EmmcDriver::encode_cmd(CmdType::ReadSingleBlock, arg)`
- **THEN** 返回正确的 32 位命令编码（`0x112A0000 | (arg & 0x1FFFF)`）

#### Scenario: aarch64 交叉编译
- **WHEN** 执行 `cargo build -p eneros-storage --target aarch64-unknown-none`
- **THEN** 编译成功，无错误

### Requirement: CRC32 数据校验

系统 SHALL 提供 CRC32 校验函数，用于块数据完整性验证。

#### Scenario: CRC32 计算正确
- **WHEN** 输入已知数据（如 `b"123456789"`）
- **THEN** 返回 `0xCBF43926`（标准 CRC32 校验值）

### Requirement: no_std 合规

所有代码 SHALL 遵循 no_std 规范（蓝图 §43.1），`#![cfg_attr(not(test), no_std)]`，仅测试模块内允许 `use std::`。

#### Scenario: no_std 编译
- **WHEN** 交叉编译到 `aarch64-unknown-none`
- **THEN** 无 `std::` 引用错误

### Requirement: 读写重试机制

系统 SHALL 对可恢复错误（超时/DMA 错误）实施最多 3 次重试，超过重试次数后标记坏块并返回错误。

#### Scenario: 重试成功
- **WHEN** 前两次 DMA 传输超时，第三次成功
- **THEN** 返回 `Ok(())`，数据正确

#### Scenario: 重试耗尽标记坏块
- **WHEN** 连续 3 次 DMA 传输超时
- **THEN** 该块被标记为坏块，返回 `Err(StorageError::BadBlock)`
