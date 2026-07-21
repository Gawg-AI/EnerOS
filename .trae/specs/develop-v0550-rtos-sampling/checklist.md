# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.55.0`
- [x] C2 members 列表已添加 `crates/kernel/rtos-sampling`
- [x] C3 `cargo metadata --format-version 1` 解析成功

## Crate 骨架
- [x] C4 `crates/kernel/rtos-sampling/Cargo.toml` 存在，package name 为 `eneros-rtos-sampling`
- [x] C5 dependencies 包含 `eneros-protocol-abstract` + `eneros-upa-model`（path 引用正确）
- [x] C6 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C7 模块声明完整：error / snapshot / shared_memory / service / stats / mock
- [x] C8 D1~D10 偏差声明表存在于 lib.rs

## SamplingError 错误类型
- [x] C9 `SamplingError` 枚举包含 PointReadFailed/SnapshotInconsistent/TooManyPoints/NotInitialized
- [x] C10 实现 `Display` + `Debug`

## SampledPoint + StateSnapshot
- [x] C11 `SampledPoint` 结构体（point_id: u32 / value: f64 / quality: u8）派生 Copy/Clone/Debug
- [x] C12 `MAX_POINTS = 256` 常量
- [x] C13 `StateSnapshot` 结构体（timestamp/seq/point_count/points 数组）
- [x] C14 `StateSnapshot::new()` 默认构造（全零）
- [x] C15 `StateSnapshot::get_points()` 返回有效部分切片

## SharedMemorySnapshot 双缓冲
- [x] C16 `SharedMemorySnapshot` 结构体（buffers: [StateSnapshot; 2] / active: AtomicU8 / write_seq: AtomicU64）
- [x] C17 `new() -> Self` 初始化
- [x] C18 `write(timestamp_us, points)` 写非活跃缓冲区 + 原子切换（D8）
- [x] C19 `read() -> Option<StateSnapshot>` 序列号一致性验证
- [x] C20 `read()` 重试上限 MAX_READ_RETRIES=10（D4）
- [x] C21 使用 `core::sync::atomic::{AtomicU8, AtomicU64}`（D8）

## SamplingStats
- [x] C22 `SamplingStats` 结构体（sample_count/read_failures/last_sample_time_us）
- [x] C23 `new() -> Self` 全零初始化
- [x] C24 `record_sample(now_us, failure_count)` 方法
- [x] C25 不使用 AtomicU64（D7）

## SamplingService
- [x] C26 `SamplingService<P: PointAccess>` 泛型结构体（D6）
- [x] C27 字段：point_ids / period_us / snapshot / protocol / stats
- [x] C28 `new(point_ids, period_us, protocol) -> Self`
- [x] C29 `sample(now_us) -> SampleReport` 单步驱动（D5）
- [x] C30 PointValue::Float/Int/Bool/Null 类型转换
- [x] C31 `quality = point.quality.valid as u8`（D10）
- [x] C32 `SampleReport` 结构体（sampled_count/failed_count/snapshot_seq）
- [x] C33 `snapshot()` / `stats()` 访问器

## MockPointAccess
- [x] C34 `MockPointAccess` 结构体（points: BTreeMap<PointId, DataPoint>）
- [x] C35 实现 `PointAccess` trait 全部 6 个方法
- [x] C36 `set_point(point_id, value, valid)` 设置测试值

## 集成测试
- [x] C37 T1 SampledPoint 构造与 Copy
- [x] C38 T2 StateSnapshot 默认值与切片访问
- [x] C39 T3 SharedMemorySnapshot 单次写入读取
- [x] C40 T4 SharedMemorySnapshot 序列号递增
- [x] C41 T5 SharedMemorySnapshot 多次写入读取一致性
- [x] C42 T6 SharedMemorySnapshot 重试上限
- [x] C43 T7 SamplingStats 更新
- [x] C44 T8 SamplingService 正常采样（3 点全部成功）
- [x] C45 T9 SamplingService 部分点读取失败
- [x] C46 T10 SamplingService 空采样点列表
- [x] C47 T11 SamplingService PointValue 类型转换
- [x] C48 T12 SamplingService 多次采样后 snapshot.seq 递增

## 设计文档
- [x] C49 `docs/kernel/rtos-sampling-design.md` 存在
- [x] C50 文档包含 12 章节
- [x] C51 文档包含 2 Mermaid 图（双缓冲架构图 + sample 时序图）
- [x] C52 D1~D10 偏差声明表
- [x] C53 文档位置在 `docs/kernel/` 下

## 版本号同步
- [x] C54 `Makefile` 版本号 0.54.0 → 0.55.0
- [x] C55 `.github/workflows/ci.yml` 版本号 0.54.0 → 0.55.0
- [x] C56 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-sampling` 说明

## 构建校验（§2.4.2 C6~C11）
- [x] C57 `cargo metadata --format-version 1` 成功
- [x] C58 `cargo test -p eneros-rtos-sampling` 全部通过
- [x] C59 `cargo build -p eneros-rtos-sampling --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
- [x] C60 `cargo fmt -p eneros-rtos-sampling -- --check` 格式通过
- [x] C61 `cargo clippy -p eneros-rtos-sampling --all-targets -- -D warnings` lint 通过
- [x] C62 `cargo deny check advisories licenses bans sources` 安全扫描通过

## 目录结构校验（§2.4.1）
- [x] C63 rtos-sampling 在 `crates/kernel/` 下（子系统归属正确）
- [x] C64 跨 crate path 引用使用相对路径
- [x] C65 设计文档在 `docs/kernel/` 下
- [x] C66 无根目录 crate
- [x] C67 .gitignore 覆盖新产生的文件类型

## no_std 合规
- [x] C68 所有 Rust 代码无 `use std::*`
- [x] C69 不使用 `panic!` / `todo!` / `unimplemented!`
- [x] C70 不要求 `Send + Sync`（除 SharedMemorySnapshot 需要原子操作，D8）
- [x] C71 子模块不重复添加 `#![cfg_attr(not(test), no_std)]`
