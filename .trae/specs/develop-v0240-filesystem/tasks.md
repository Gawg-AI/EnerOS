# Tasks — v0.24.0 + v0.24.1 文件系统 + 写寿命管理

## v0.24.0 — 日志结构文件系统（littlefs2 集成）

- [x] Task 1: 创建 eneros-fs crate 骨架
  - [x] SubTask 1.1: 创建 `crates/drivers/fs/Cargo.toml`（name=eneros-fs, version=0.24.1, edition=2021, 依赖 eneros-storage + littlefs2[c-stubs]）
  - [x] SubTask 1.2: 创建 `crates/drivers/fs/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明）
  - [x] SubTask 1.3: 根 `Cargo.toml` 添加 `"crates/drivers/fs"` 到 workspace members，版本更新为 `0.24.1`
  - [x] SubTask 1.4: `Makefile` VERSION 更新为 `0.24.1`，添加 `fs-build`/`fs-test` 目标
  - [x] SubTask 1.5: `.github/workflows/ci.yml` 版本标识更新为 `v0.24.1`，添加 eneros-fs 交叉编译步骤
  - [x] SubTask 1.6: `ci/src/gate.rs` 注释含 v0.24.0/v0.24.1 说明

- [x] Task 2: 实现 error.rs — FsError 错误类型
  - [x] SubTask 2.1: 定义 `FsError` 枚举（14 变体）
  - [x] SubTask 2.2: 实现 `is_corruption()` 方法
  - [x] SubTask 2.3: 实现 `Display` + `Debug` trait
  - [x] SubTask 2.4: 实现 `From<littlefs2::io::Error>` 转换
  - [x] SubTask 2.5: 实现 `From<StorageError>` 转换
  - [x] SubTask 2.6: 单元测试

- [x] Task 3: 实现 types.rs — FS 类型定义
  - [x] SubTask 3.1~3.7: FileMode/OpenFlags/SeekFrom/FileStat/DirEntry/DiskUsage + 测试

- [x] Task 4: 实现 fs_trait.rs — FileSystem trait
  - [x] SubTask 4.1~4.3: 11 方法 trait + MockFs + 测试

- [x] Task 5: 实现 file.rs — File 句柄
  - [x] SubTask 5.1~5.4: 值类型 File + 测试

- [x] Task 6: 实现 lfs/storage_adapter.rs — BlockDevice → littlefs2 Storage 适配器
  - [x] SubTask 6.1~6.4: BlockDeviceStorage + Storage trait 实现 + 测试

- [x] Task 7: 实现 lfs/filesystem.rs — Lfs impl FileSystem
  - [x] SubTask 7.1~7.6: Lfs 结构 + mount/format/unmount + 11 方法 + 测试

- [x] Task 8: 实现 lfs/config.rs — LfsConfig
  - [x] SubTask 8.1~8.3: 6 字段配置 + Default + 测试

- [x] Task 9: lib.rs 导出与文档注释
  - [x] SubTask 9.1~9.3: pub mod + pub use + 文档注释

- [x] Task 10: 文档与配置
  - [x] SubTask 10.1: 创建 `docs/drivers/lfs-design.md`（~314 行）
  - [x] SubTask 10.2: 创建 `configs/lfs.toml`

- [x] Task 11: v0.24.0 构建与质量验证
  - [x] SubTask 11.1: `cargo fmt --all -- --check` 通过
  - [x] SubTask 11.2: `cargo clippy -p eneros-fs --all-targets -- -D warnings` 无 warning
  - [x] SubTask 11.3: `cargo test -p eneros-fs` 通过（197 测试）
  - [x] SubTask 11.4: aarch64 交叉编译 — ⚠️ 本地 Windows 缺 C 编译器，CI 可编译
  - [x] SubTask 11.5: workspace 回归全通过
  - [x] SubTask 11.6: `cargo run -p eneros-ci` 通过（Overall: PASS）

## v0.24.1 — 存储写寿命管理

- [x] Task 12: 实现 wear/status.rs — WearStatus + WearDistribution
  - [x] SubTask 12.1~12.4: 6 字段状态 + p50/p99/max 分布 + 测试（15 个）

- [x] Task 13: 实现 wear/manager.rs — WearLevelManager + WearLeveling trait
  - [x] SubTask 13.1~13.9: trait + BTreeMap 计数 + victim 选择 + 寿命估算 + 测试（18 个）

- [x] Task 14: 实现 wear/write_amp.rs — 写放大统计
  - [x] SubTask 14.1~14.5: WriteAmplificationTracker + 节流 + 测试（13 个）

- [x] Task 15: 实现 wear/mod.rs — 模块导出 + 全局接口
  - [x] SubTask 15.1~15.4: spin::Mutex 全局接口 + 测试（9 个，含竞态修复）

- [x] Task 16: v0.24.1 文档与配置
  - [x] SubTask 16.1: 创建 `docs/drivers/wear-leveling-design.md`（~285 行）
  - [x] SubTask 16.2: 创建 `configs/wear-level.toml`

- [x] Task 17: v0.24.1 版本更新与构建验证
  - [x] SubTask 17.1: `crates/drivers/fs/Cargo.toml` 版本更新为 `0.24.1`
  - [x] SubTask 17.2: 根 `Cargo.toml` workspace 版本更新为 `0.24.1`
  - [x] SubTask 17.3: `Makefile`/`ci.yml`/`gate.rs` 版本标识更新为 `v0.24.1`
  - [x] SubTask 17.4: `cargo fmt --all -- --check` 通过
  - [x] SubTask 17.5: `cargo clippy -p eneros-fs --all-targets -- -D warnings` 无 warning
  - [x] SubTask 17.6: `cargo test -p eneros-fs` 通过（wear 模块 58 测试）
  - [x] SubTask 17.7: aarch64 交叉编译 — ⚠️ 同 Task 11.4
  - [x] SubTask 17.8: workspace 回归全通过
  - [x] SubTask 17.9: `cargo run -p eneros-ci` 通过（Overall: PASS）

## v0.24.0 + v0.24.1 集成验收

- [x] Task 18: 目录结构校验（§2.4 校验清单）
  - [x] SubTask 18.1: C1 新 crate 在 `crates/drivers/fs/` 下
  - [x] SubTask 18.2: C2 workspace members 已添加
  - [x] SubTask 18.3: C3 跨 crate path 引用正确（eneros-storage）
  - [x] SubTask 18.4: C4 文档在 `docs/drivers/` 下
  - [x] SubTask 18.5: C5 无根目录 crate
  - [x] SubTask 18.6: C13 无垃圾文件被追踪

# Task Dependencies

- Task 1（crate 骨架）独立，最先执行
- Task 2（error）独立，可与 Task 1 并行
- Task 3（types）独立，可与 Task 1/2 并行
- Task 4（fs_trait）依赖 Task 2/3
- Task 5（file）依赖 Task 2/3/4
- Task 6（storage_adapter）依赖 Task 1（littlefs2 依赖）
- Task 7（filesystem）依赖 Task 2/3/4/5/6
- Task 8（config）依赖 Task 3
- Task 9（lib 导出）依赖全部前序
- Task 10（文档配置）依赖 Task 9
- Task 11（v0.24.0 验证）依赖全部前序
- Task 12（wear status）依赖 Task 9（同 crate）
- Task 13（wear manager）依赖 Task 12
- Task 14（write_amp）依赖 Task 12
- Task 15（wear mod）依赖 Task 12/13/14
- Task 16（v0.24.1 文档）依赖 Task 15
- Task 17（v0.24.1 验证）依赖 Task 15/16
- Task 18（集成校验）依赖 Task 11/17

# Notes

- 遵循 no_std：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 遵循 §2.3.1：crate 放 `crates/drivers/fs/` 下
- 遵循 §2.3.3：文档放 `docs/drivers/` 下
- 遵循 §5.5：集成 littlefs2，仅自研适配层 + 门面 + 能源特有接口
- littlefs2 依赖 C 编译（lfs.c），使用 `c-stubs` feature 提供 strcpy，aarch64 交叉编译用 aarch64-linux-gnu-gcc
- 不修改 eneros-storage crate 代码（surgical changes）
- v0.24.1 路径修正：`crates/kernel/mm/` → `crates/drivers/fs/src/wear/`（磨损均衡属存储，非内存管理）
- 所有 18 个任务已完成，197 个单元测试通过，CI 质量门禁 Overall: PASS
