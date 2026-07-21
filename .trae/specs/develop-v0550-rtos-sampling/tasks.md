# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.54.0` → `0.55.0`
  - [x] members 添加 `crates/kernel/rtos-sampling`
  - [x] `cargo metadata --format-version 1` 验证 workspace 解析成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-rtos-sampling` crate 骨架
  - [x] 新建 `crates/kernel/rtos-sampling/Cargo.toml`，package name = `eneros-rtos-sampling`
  - [x] dependencies：`eneros-protocol-abstract`（path = `../../protocols/protocol-abstract`，跨子系统）+ `eneros-upa-model`（path = `../../protocols/upa-model`，跨子系统）
  - [x] 新建 `src/lib.rs`，包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 模块声明：error / snapshot / shared_memory / service / stats / mock
  - [x] lib.rs 包含 D1~D10 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 3: 实现 `error.rs` — SamplingError 错误类型
  - [x] `SamplingError` 枚举：PointReadFailed / SnapshotInconsistent / TooManyPoints / NotInitialized
  - [x] 实现 `Display` + `Debug`
  - [x] 验证：`cargo build -p eneros-rtos-sampling` 通过

- [x] Task 4: 实现 `snapshot.rs` — SampledPoint + StateSnapshot
  - [x] `SampledPoint` 结构体（point_id: u32 / value: f64 / quality: u8）派生 Copy/Clone/Debug
  - [x] `MAX_POINTS = 256` 常量
  - [x] `StateSnapshot` 结构体（timestamp: u64 / seq: u64 / point_count: u32 / points: [SampledPoint; MAX_POINTS]）
  - [x] `StateSnapshot::new()` 默认构造（全零）
  - [x] `StateSnapshot::get_points(&self) -> &[SampledPoint]` 返回有效部分切片
  - [x] 验证：单元测试 — 构造、默认值、切片访问

- [x] Task 5: 实现 `shared_memory.rs` — SharedMemorySnapshot 双缓冲
  - [x] `SharedMemorySnapshot` 结构体（buffers: [StateSnapshot; 2] / active: AtomicU8 / write_seq: AtomicU64）
  - [x] `new() -> Self`：初始化两个空快照，active=0, write_seq=0
  - [x] `write(&self, timestamp_us: u64, points: &[SampledPoint])`：写非活跃缓冲区 → 原子切换（D8：用 core::sync::atomic）
  - [x] `read(&self) -> Option<StateSnapshot>`：读活跃缓冲区 + 序列号一致性验证 + 重试上限 MAX_READ_RETRIES=10（D4）
  - [x] 验证：单元测试 — 单次写入读取、序列号递增、并发写入读取一致性、重试上限

- [x] Task 6: 实现 `stats.rs` — SamplingStats
  - [x] `SamplingStats` 结构体（sample_count: u64 / read_failures: u64 / last_sample_time_us: u64）
  - [x] `new() -> Self`：全零初始化
  - [x] `record_sample(&mut self, now_us: u64, failure_count: u64)`：更新统计
  - [x] 不使用 AtomicU64（D7：单线程）
  - [x] 验证：单元测试 — 更新与读取

- [x] Task 7: 实现 `service.rs` — SamplingService 采样服务
  - [x] `SamplingService<P: PointAccess>` 泛型结构体（D6：不用 Box<dyn PointAccess>）
  - [x] 字段：point_ids: Vec<PointId> / period_us: u64 / snapshot: SharedMemorySnapshot / protocol: P / stats: SamplingStats
  - [x] `new(point_ids, period_us, protocol) -> Self`
  - [x] `sample(&mut self, now_us: u64) -> SampleReport`：遍历 point_ids 读点 → 转换 PointValue → 写快照 → 更新统计
  - [x] `SampleReport` 结构体（sampled_count: usize / failed_count: usize / snapshot_seq: u64）
  - [x] `PointValue::Float(v) → v` / `PointValue::Int(v) → v as f64` / 其他跳过
  - [x] `quality = point.quality.valid as u8`（D10）
  - [x] `snapshot()` / `stats()` 访问器
  - [x] 验证：单元测试 — 正常采样、部分失败、空列表、PointValue 类型转换

- [x] Task 8: 实现 `mock.rs` — MockPointAccess 测试工具
  - [x] `MockPointAccess` 结构体（points: BTreeMap<PointId, DataPoint>）
  - [x] 实现 `PointAccess` trait 全部 6 个方法
  - [x] `set_point(point_id, value, valid)` 设置测试值
  - [x] `fail_on_read(point_id)` 标记下次 read_point 返回 Err（用于测试失败场景）
  - [x] 验证：编译通过（在测试中使用）

- [x] Task 9: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 SampledPoint 构造与 Copy
  - [x] T2 StateSnapshot 默认值与切片访问
  - [x] T3 SharedMemorySnapshot 单次写入读取
  - [x] T4 SharedMemorySnapshot 序列号递增
  - [x] T5 SharedMemorySnapshot 多次写入读取一致性
  - [x] T6 SharedMemorySnapshot 重试上限（模拟切换频繁场景，read 返回 None）
  - [x] T7 SamplingStats 更新
  - [x] T8 SamplingService 正常采样（3 点全部成功）
  - [x] T9 SamplingService 部分点读取失败（1 点失败，2 点成功）
  - [x] T10 SamplingService 空采样点列表
  - [x] T11 SamplingService PointValue 类型转换（Float/Int/Bool/Null）
  - [x] T12 SamplingService 多次采样后 snapshot.seq 递增
  - [x] 验证：`cargo test -p eneros-rtos-sampling` 全部通过

- [x] Task 10: 设计文档 `docs/kernel/rtos-sampling-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / 核心类型 / StateSnapshot / SharedMemorySnapshot / SamplingService / 错误处理 / 数据一致性 / 性能 / 测试策略 / 与上下游关系 / 偏差声明
  - [x] 2 Mermaid 图：双缓冲架构图 + sample 时序图
  - [x] D1~D10 偏差声明表
  - [x] 文档位置在 `docs/kernel/` 下（符合目录规范）

- [x] Task 11: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.54.0` → `0.55.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.54.0` → `0.55.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-rtos-sampling` 说明
  - [x] 验证：`cargo build -p eneros-rtos-sampling` 通过

- [x] Task 12: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-rtos-sampling` 全部通过（12 tests passed）
  - [x] `cargo build -p eneros-rtos-sampling --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-rtos-sampling -- --check` 格式通过
  - [x] `cargo clippy -p eneros-rtos-sampling --all-targets -- -D warnings` lint 通过（Finished dev profile, 无 warning）
  - [x] `cargo deny check advisories licenses bans sources` 安全扫描通过（允许 advisories 网络问题降级 — GitHub 连接失败）

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~8 → Task 2（各模块依赖 crate 骨架）
- Task 5（shared_memory）依赖 Task 4（snapshot）
- Task 7（service）依赖 Task 4 + 5 + 6 + 8
- Task 9 → Task 4, 5, 6, 7, 8（集成测试依赖各模块）
- Task 10 → Task 9（文档在测试通过后撰写）
- Task 11 → Task 10（版本同步在功能完成后）
- Task 12 → Task 11（构建校验在所有改动完成后）

# Parallelizable Work

- Task 3（error）+ Task 4（snapshot）+ Task 6（stats）+ Task 8（mock）可并行
- Task 5（shared_memory）依赖 Task 4
- Task 7（service）依赖 Task 4 + 5 + 6 + 8
