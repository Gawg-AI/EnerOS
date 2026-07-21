# Tasks — v0.25.0 TSDB

- [x] Task 1: 创建 eneros-tsdb crate 骨架
  - [ ] SubTask 1.1: 创建 `crates/drivers/tsdb/Cargo.toml`（name=eneros-tsdb, version=0.25.0, 依赖 eneros-fs + 压缩 crate）
  - [ ] SubTask 1.2: 创建 `crates/drivers/tsdb/src/lib.rs`（#![cfg_attr(not(test), no_std)] + extern crate alloc + 模块声明）
  - [ ] SubTask 1.3: 根 `Cargo.toml` workspace.members 添加 "crates/drivers/tsdb"，workspace.package.version 改为 "0.25.0"
  - [ ] 验证: `cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 2: 实现 schema.rs — 时序数据 schema 定义
  - [ ] SubTask 2.1: 定义 DeviceId (u32)、MetricId (u32)、DataQuality (enum: Good/Uncertain/Bad + as u8)
  - [ ] SubTask 2.2: 定义 TimeSeriesPoint { timestamp: u64, device_id, metric, value: f64, quality: DataQuality }
  - [ ] SubTask 2.3: 定义 TsdbConfig { data_dir: String, chunk_duration_ms, max_points_per_chunk, compression: CompressionType, retention_ms, flush_interval_ms }
  - [ ] SubTask 2.4: 定义 ColumnarChunk { header: ChunkHeader, timestamps: Vec<u64>, values: Vec<f64>, qualities: Vec<u8>, compressed: Vec<u8> } + new()
  - [ ] SubTask 2.5: 定义 ChunkHeader { device_id, metric, start_time, end_time, point_count, crc32 } + to_bytes()/from_bytes()
  - [ ] SubTask 2.6: 定义 Query { device_ids, metrics, time_range, aggregation: Option<Aggregation>, limit } + Aggregation (enum: Avg/Max/Min/Sum/Count) + AggResult
  - [ ] SubTask 2.7: 定义 CompressionType (enum: None/Snappy)
  - [ ] 验证: `cargo build -p eneros-tsdb` 编译通过（schema 模块独立编译）

- [x] Task 3: 实现 error.rs — TsdbError 错误类型
  - [ ] SubTask 3.1: 定义 TsdbError 枚举（8 变体：DiskFull/IoError(String)/DecompressFailed/IndexCorrupted/InvalidQuery/DeviceNotFound(DeviceId)/MetricNotFound(MetricId)/ChunkCorrupted{chunk_id:u32}）
  - [ ] SubTask 3.2: 实现 Debug + Clone + PartialEq
  - [ ] SubTask 3.3: 实现 From<FsError> for TsdbError（FsError::DiskFull→DiskFull, 其他→IoError）
  - [ ] SubTask 3.4: 实现 Display（core::fmt::Display，no_std 友好）
  - [ ] 验证: 单元测试 FsError→TsdbError 转换正确

- [x] Task 4: 实现 compression.rs — Snappy 压缩
  - [ ] SubTask 4.1: 定义 Compressor trait { compress(&self, &[u8]) -> Result<Vec<u8>, TsdbError>; decompress(&self, &[u8]) -> Result<Vec<u8>, TsdbError>; ratio(&self) -> f32 }
  - [ ] SubTask 4.2: 验证 snap crate raw API 的 no_std 兼容性（`snap::raw::Encoder`/`Decoder` 是否依赖 std::io）
  - [ ] SubTask 4.3: 若 snap raw 不兼容 no_std，按 spec 策略选择备选（compcol/自研/lz4_flex）
  - [ ] SubTask 4.4: 实现 SnappyCompressor（compress/decompress/ratio，内部用选定后端）
  - [ ] SubTask 4.5: 实现 NoopCompressor（CompressionType::None 时的透传实现）
  - [ ] 验证: 压缩-解压往返测试 + 压缩比统计测试

- [x] Task 5: 实现 index.rs — 时间索引
  - [ ] SubTask 5.1: 定义 IndexEntry { time: u64, file_path: String, chunk_id: u32, point_count: u32 }
  - [ ] SubTask 5.2: 定义 TimeIndex { entries: BTreeMap<u64, IndexEntry> }（按时间排序）
  - [ ] SubTask 5.3: 实现 add(time, file_path, chunk_id, point_count)
  - [ ] SubTask 5.4: 实现 find_range(start, end) -> Vec<&IndexEntry>（返回覆盖时间范围的 entries）
  - [ ] SubTask 5.5: 实现 remove_before(time) -> Vec<IndexEntry>（TTL 清理用，返回被移除的 entries）
  - [ ] SubTask 5.6: 实现 serialize()/deserialize()（持久化到索引文件）
  - [ ] 验证: 索引查找/删除测试

- [x] Task 6: 实现 writer.rs — 时序数据写入器
  - [ ] SubTask 6.1: 定义 TsdbWriter trait { append(&mut self, &TimeSeriesPoint) -> Result<(), TsdbError>; flush(&mut self) -> Result<(), TsdbError>; current_chunk_size(&self) -> usize }
  - [ ] SubTask 6.2: 实现 TsdbWriterImpl { config, chunks: BTreeMap<(DeviceId,MetricId), ColumnarChunk>, index: TimeIndex, fs: &mut Lfs, total_written: u64 }
  - [ ] SubTask 6.3: 实现 append()（按 device+metric 分组写入内存 chunk，检查 chunk 切换条件）
  - [ ] SubTask 6.4: 实现 flush_chunk()（列式压缩：timestamps→Delta-of-delta，values/qualities→Snappy，写入文件，更新索引）
  - [ ] SubTask 6.5: 实现 serialize_timestamps()（Delta-of-delta 编码 + encode_varint_signed）
  - [ ] SubTask 6.6: 实现 deserialize_timestamps()（Delta-of-delta 解码 + decode_varint_signed）
  - [ ] SubTask 6.7: 实现 serialize_values()/deserialize_values()（f64 → little-endian bytes）
  - [ ] 验证: 写入测试 + Delta-of-delta 编解码测试（等间隔/非等间隔）

- [x] Task 7: 实现 reader.rs — 时序数据查询器
  - [ ] SubTask 7.1: 定义 TsdbReader trait { read_range(&self, DeviceId, MetricId, u64, u64) -> Result<Vec<TimeSeriesPoint>, TsdbError>; read_last(&self, DeviceId, MetricId) -> Result<Option<TimeSeriesPoint>, TsdbError>; aggregate(&self, &Query) -> Result<AggResult, TsdbError> }
  - [ ] SubTask 7.2: 实现 TsdbReaderImpl { fs: &Lfs, index: &TimeIndex, compressor: &dyn Compressor }
  - [ ] SubTask 7.3: 实现 read_range()（索引查找→读取 chunk 文件→解压→过滤时间范围）
  - [ ] SubTask 7.4: 实现 read_last()（索引查找最新 chunk→读取最后一个数据点）
  - [ ] SubTask 7.5: 实现 aggregate()（avg/max/min/sum/count 聚合计算）
  - [ ] 验证: 范围查询/最新点查询/聚合查询测试

- [x] Task 8: 实现 retention.rs — TTL 过期清理
  - [ ] SubTask 8.1: 定义 RetentionPolicy { retention_ms: u64 } + should_expire(time, now) -> bool
  - [ ] SubTask 8.2: 实现 cleanup_expired(index: &mut TimeIndex, fs: &mut Lfs, now: u64, retention_ms: u64) -> Result<u64, TsdbError>（删除过期 chunk 文件，返回删除数）
  - [ ] 验证: TTL 清理测试（过期数据被删除，未过期保留）

- [x] Task 9: 实现 TimeSeriesDB 主入口
  - [ ] SubTask 9.1: 定义 TimeSeriesDB { fs: Lfs, writer: TsdbWriterImpl, index: TimeIndex, config: TsdbConfig, compressor: Box<dyn Compressor> }
  - [ ] SubTask 9.2: 实现 open(fs: Lfs, config: TsdbConfig) -> Result<Self, TsdbError>（创建数据目录，加载索引）
  - [ ] SubTask 9.3: 实现 write(&mut self, &TimeSeriesPoint) -> Result<(), TsdbError>
  - [ ] SubTask 9.4: 实现 write_batch(&mut self, &[TimeSeriesPoint]) -> Result<(), TsdbError>
  - [ ] SubTask 9.5: 实现 query(&self, &Query) -> Result<Vec<TimeSeriesPoint>, TsdbError>（含聚合分支）
  - [ ] SubTask 9.6: 实现 query_range(&self, DeviceId, MetricId, u64, u64) -> Result<Vec<TimeSeriesPoint>, TsdbError>
  - [ ] SubTask 9.7: 实现 compact(&mut self) -> Result<(), TsdbError>（flush 所有内存 chunk）
  - [ ] SubTask 9.8: 实现 cleanup_expired(&mut self) -> Result<u64, TsdbError>
  - [ ] SubTask 9.9: 实现 close(self) -> Result<(), TsdbError>（flush + 保存索引）
  - [ ] 验证: 集成测试（写入→查询→清理全流程）

- [x] Task 10: lib.rs 导出与文档注释
  - [ ] SubTask 10.1: lib.rs 添加模块导出（pub mod schema/error/compression/index/writer/reader/retention）+ pub use 关键类型
  - [ ] SubTask 10.2: lib.rs 添加 crate 文档注释（架构图 + 使用示例 + 设计决策）
  - [ ] 验证: `cargo doc -p eneros-tsdb --no-deps` 生成文档无警告

- [x] Task 11: 文档与配置
  - [ ] SubTask 11.1: 创建 `docs/drivers/tsdb-design.md`（设计文档：架构 + 数据结构 + 压缩算法 + 索引 + TTL + 性能基准）
  - [ ] SubTask 11.2: 创建 `configs/tsdb.toml`（配置模板：data_dir/chunk_duration/max_points/compression/retention/flush_interval）
  - [ ] 验证: 文档位于 `docs/drivers/`（§2.3.3 文档分类），非 docs/ 根

- [x] Task 12: 版本标识更新
  - [ ] SubTask 12.1: 根 `Cargo.toml` workspace.package.version = "0.25.0"
  - [ ] SubTask 12.2: `Makefile` VERSION := 0.25.0
  - [ ] SubTask 12.3: `.github/workflows/ci.yml` Version: v0.25.0 + 添加 eneros-tsdb 交叉编译步骤
  - [ ] SubTask 12.4: `ci/src/gate.rs` 注释添加 eneros-tsdb（v0.25.0 TSDB）说明
  - [ ] 验证: 版本号一致性 + ci.yml 含 eneros-tsdb 构建步骤

- [x] Task 13: 构建与质量验证
  - [x] SubTask 13.1: `cargo fmt --all -- --check` 通过
  - [x] SubTask 13.2: `cargo clippy -p eneros-tsdb --all-targets -- -D warnings` 通过（修复 doc_lazy_continuation 后通过）
  - [x] SubTask 13.3: `cargo test -p eneros-tsdb` 通过（104 单元测试 + 2 doc-tests 忽略）
  - [x] SubTask 13.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试）
  - [x] SubTask 13.5: `cargo run -p eneros-ci` 通过（Overall: PASS — fmt/clippy/test 全绿）
  - [x] SubTask 13.6: `cargo build -p eneros-tsdb --target aarch64-unknown-none` 本地缺 gcc 阻塞，CI（Ubuntu）已配置 `Build tsdb crate` 步骤验证
  - [x] SubTask 13.7: `cargo deny check licenses bans sources` 通过；advisories 因本地无法访问 advisory-db 网络阻塞，由 CI 验证
  - [x] 验证: 所有本地可执行检查项 PASS

# Task Dependencies

- Task 2 (schema) 无依赖，可先开始
- Task 3 (error) 无依赖，可与 Task 2 并行
- Task 4 (compression) 依赖 Task 3 (error: TsdbError)
- Task 5 (index) 依赖 Task 2 (schema: DeviceId/MetricId) + Task 3 (error)
- Task 6 (writer) 依赖 Task 2,3,4,5
- Task 7 (reader) 依赖 Task 2,3,4,5（与 Task 6 可部分并行）
- Task 8 (retention) 依赖 Task 3,5
- Task 9 (TimeSeriesDB) 依赖 Task 6,7,8
- Task 10 (lib.rs) 依赖 Task 9
- Task 11 (文档) 依赖 Task 9（可在 Task 9 完成后并行于 Task 10）
- Task 12 (版本) 可与 Task 2-9 并行（仅改配置文件）
- Task 13 (验证) 依赖 Task 10,11,12 全部完成

# 并行化建议

- **Wave 1（并行）**: Task 1（骨架）、Task 12（版本标识）
- **Wave 2（并行）**: Task 2（schema）、Task 3（error）
- **Wave 3（并行）**: Task 4（compression，依赖 3）、Task 5（index，依赖 2,3）
- **Wave 4（并行）**: Task 6（writer，依赖 2,3,4,5）、Task 8（retention，依赖 3,5）
- **Wave 5**: Task 7（reader，依赖 2,3,4,5）
- **Wave 6**: Task 9（TimeSeriesDB，依赖 6,7,8）
- **Wave 7（并行）**: Task 10（lib.rs）、Task 11（文档）
- **Wave 8**: Task 13（验证）
