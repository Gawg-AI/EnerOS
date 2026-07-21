# v0.24.0 + v0.24.1 — 日志结构文件系统 + 存储写寿命管理 Spec

## Why

Phase 1 所有需要文件读写的后续版本（v0.25.0 时序存储、v0.26.0 配置管理、v0.60.0 模型加载）均依赖文件系统。v0.24.0 是 ★瓶颈版本，阻塞整个 Phase 1 上层开发。v0.24.1 为刚性子版本，保障 10 年免维护部署的存储可靠性。

**架构决策**：依据项目规则 §5.5（默认集成清单：文件系统 → littlefs2）+ §8.1 #14（禁止重复造轮子）+ 蓝图 §42.4（自研 8 模块 LFS 属中度过度设计，建议集成 littlefs2），本版本采用 **littlefs2 集成方案**，不自研 LFS。littlefs2 自带掉电安全（copy-on-write）+ 动态磨损均衡 + 坏块管理，v0.24.1 仅需构建监控/可观测层。

## What Changes

### v0.24.0 — 日志结构文件系统（基于 littlefs2 集成）

- **新增 crate** `crates/drivers/fs/`（eneros-fs，v0.24.0）
- 集成 `littlefs2` crate（no_std + c-stubs feature，BSD-3-Clause 许可，deny.toml 已允许）
- 实现 `BlockDevice → littlefs2::driver::Storage` 适配器，桥接 v0.23.0 块设备驱动
- 实现蓝图 `FileSystem` trait（11 方法：open/create/remove/rename/stat/mkdir/rmdir/readdir/sync/df）作为 littlefs2 的门面层
- 实现 `File` 句柄（read/write/seek/close/truncate）
- 实现 `FsError` 错误类型（14 变体，含 is_corruption()）
- 实现 `FileStat`/`FileMode`/`OpenFlags`/`DirEntry`/`DiskUsage`/`SeekFrom` 类型
- 文档：`docs/drivers/lfs-design.md`
- 配置：`configs/lfs.toml`

### v0.24.1 — 存储写寿命管理

- 在 eneros-fs crate 内新增 `wear` 子模块（`crates/drivers/fs/src/wear/`）
  - **注意**：蓝图原路径 `crates/kernel/mm/src/wear_level.rs` 不合理（磨损均衡属存储概念，非内存管理），按 §2.3.2 归属判定放入 drivers/fs
- 实现 `WearLevelManager`（擦写计数记录、victim 块选择、寿命估算）
- 实现 `WearStatus`/`WearDistribution` 状态报告
- 实现 `WearLeveling` trait（record_erase/select_victim_block/estimate_lifespan）
- 实现写放大统计（write_amplification = 实际写入 / 应用写入）
- 文档：`docs/drivers/wear-leveling-design.md`
- 配置：`configs/wear-level.toml`

## Impact

- **Affected specs**: v0.23.0（storage，提供 BlockDevice trait）、v0.25.0（TSDB，依赖 FileSystem）、v0.26.0（配置管理）、v0.60.0（模型加载）
- **Affected code**: 新增 `crates/drivers/fs/`，修改根 `Cargo.toml`（workspace members + version）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- **New dependency**: `littlefs2`（+ `littlefs2-sys` C 编译，使用现有 `aarch64-linux-gnu-gcc` 工具链）
- **License**: BSD-3-Clause（littlefs C 库）+ Apache-2.0/MIT（Rust 封装），deny.toml 已允许

## ADDED Requirements

### Requirement: FileSystem Trait（文件系统统一接口）

系统 SHALL 提供统一的 `FileSystem` trait，支持 open/create/remove/rename/stat/mkdir/rmdir/readdir/sync/df 操作，底层通过 littlefs2 实现掉电安全与磨损均衡。

#### Scenario: 创建并写入文件
- **WHEN** 调用 `fs.create("/test.txt", FileMode::FILE)` 后 `file.write(b"hello")`
- **THEN** 返回写入字节数 5，`fs.stat("/test.txt").size == 5`

#### Scenario: 掉电恢复
- **WHEN** 写入数据后未调用 `sync()` 即"断电"（释放 FS 实例），重新 mount
- **THEN** 已 sync 的数据完整，未 sync 的最后写入可能丢失但文件系统不损坏

#### Scenario: 目录操作
- **WHEN** 调用 `fs.mkdir("/data")` 后 `fs.readdir("/")` 
- **THEN** 返回包含 "data" 的 DirEntry 列表

### Requirement: BlockDevice 适配器

系统 SHALL 提供 `BlockDeviceStorage` 适配器，将 v0.23.0 的 `BlockDevice` trait 适配为 littlefs2 的 `Storage` trait，使 littlefs2 能直接读写 EnerOS 块设备。

#### Scenario: 通过 MockBlockDevice 挂载
- **WHEN** 使用 `MockBlockDevice::new(1024, 4096)` 创建块设备，通过适配器挂载 littlefs2
- **THEN** format + mount 成功，可创建/读写文件

### Requirement: WearLeveling 监控（v0.24.1）

系统 SHALL 提供写寿命监控接口，记录块擦写次数、计算写放大因子、估算存储寿命，为 10 年免维护部署提供可观测性。

#### Scenario: 擦写计数记录
- **WHEN** 对块 42 执行 erase 操作后调用 `wear_level_status()`
- **THEN** `WearStatus.max_block_erases` 反映块 42 的擦写次数

#### Scenario: 寿命估算
- **GIVEN** 日均写入 500MB，块大小 4096，总块数 65536，SLC 擦写上限 100000
- **WHEN** 调用 `estimate_lifespan(500)` 
- **THEN** 返回估算寿命 ≥ 10 年

## MODIFIED Requirements

### Requirement: Workspace 版本号

根 `Cargo.toml` workspace.package.version 从 `0.23.0` 更新为 `0.24.0`（v0.24.0 开发）→ `0.24.1`（v0.24.1 开发）。

## 设计决策记录

### 为什么用 littlefs2 而非自研 LFS

| 维度 | littlefs2 集成 | 自研 LFS（蓝图原设计） |
|------|---------------|----------------------|
| 项目规则合规 | ✅ §5.5 强制 | ❌ 违反 §8.1 #14 |
| 掉电安全 | ✅ copy-on-write，工业验证 | 需自实现原子日志提交 |
| 磨损均衡 | ✅ 内置动态 WL | 需自实现（v0.24.1） |
| 开发量 | 适配层 + 门面（~6 模块） | 8 模块完整 LFS |
| C 依赖 | lfs.c（aarch64-linux-gnu-gcc 已配置） | 无 |
| 许可证 | BSD-3-Clause（deny.toml 已允许） | — |
| 成熟度 | 27K downloads/month，工业级 | 未验证 |

### v0.24.1 路径修正

蓝图原路径 `crates/kernel/mm/src/wear_level.rs` → 修正为 `crates/drivers/fs/src/wear/`。理由：磨损均衡属存储概念（flash 块擦写），非内存管理（RAM），按 §2.3.2 归属 drivers 子系统。且 littlefs2 已内置 WL，v0.24.1 为监控层，与 FS 同 crate 更内聚。
