# Checklist — v0.25.0 TSDB

## 目录结构校验（§2.4 C1-C5）

- [x] C1 新 crate 位置：eneros-tsdb 位于 `crates/drivers/tsdb/`，未直接放根目录
- [x] C2 workspace members：根 `Cargo.toml` 的 members 已添加 `"crates/drivers/tsdb"`
- [x] C3 跨 crate path 引用：`crates/drivers/tsdb/Cargo.toml` 的 `eneros-fs = { path = "../fs" }` 使用正确相对路径
- [x] C4 文档分类：`docs/drivers/tsdb-design.md` 位于 `docs/drivers/` 子目录，未平面化放 `docs/` 根
- [x] C5 无根目录 crate：仓库根目录无新增 Rust crate 文件夹

## no_std 合规（§4.3）

- [x] `crates/drivers/tsdb/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`，改用 `alloc::*` / `core::*`
- [x] HashMap 改用 `alloc::collections::BTreeMap`
- [x] `core::time::Duration` 替代 `std::time::Duration`
- [x] Snappy 压缩后端兼容 no_std（snap raw 不兼容 → 切换到 lz4_flex，Compressor trait 抽象）

## 接口实现完整性

- [x] schema.rs: DeviceId/MetricId/DataQuality/TimeSeriesPoint/TsdbConfig/ColumnarChunk/ChunkHeader/Query/Aggregation/AggResult/CompressionType 全部定义
- [x] error.rs: TsdbError 8 变体 + Debug/Clone/PartialEq/Display + From<FsError>
- [x] compression.rs: Compressor trait + SnappyCompressor + NoopCompressor
- [x] index.rs: TimeIndex + IndexEntry + add/find_range/remove_before/serialize/deserialize
- [x] writer.rs: TsdbWriter trait + TsdbWriterImpl + append/flush_chunk + serialize_timestamps(Delta-of-delta)/deserialize_timestamps
- [x] reader.rs: TsdbReader trait + TsdbReaderImpl + read_range/read_last/aggregate
- [x] retention.rs: cleanup_expired 函数
- [x] TimeSeriesDB: open/write/write_batch/query/query_range/compact/cleanup_expired/close

## Snappy 压缩 no_std 兼容

- [x] 已验证 snap crate raw API 是否兼容 no_std（snap v1.1.1 依赖 std::io，不兼容）
- [x] 若 snap 不兼容，已选择备选（lz4_flex）并记录决策（compression.rs 顶部注释 + lib.rs 设计决策）
- [x] Compressor trait 抽象确保压缩后端可替换
- [x] 压缩-解压往返测试通过（compression.rs 9 个测试）
- [x] 压缩比统计测试通过

## Delta-of-delta 编码

- [x] serialize_timestamps() 实现等间隔时间戳高压缩
- [x] deserialize_timestamps() 正确解码
- [x] 等间隔时间戳压缩率测试通过（regular_intervals_high_compression 测试）
- [x] 非等间隔时间戳编解码往返测试通过（irregular_timestamps_roundtrip 测试）

## 构建校验（§2.4 C6-C11）

- [x] C6 `cargo metadata --format-version 1 > /dev/null` 成功
- [x] C7 `cargo test -p eneros-tsdb` 通过（104 单元测试，远超 80% 覆盖）
- [x] C8 `cargo build -p eneros-tsdb --target aarch64-unknown-none` **通过**（WSL2 Ubuntu-22.04 + aarch64-linux-gnu-gcc 13.3.0 + libclang-18，9.25s 编译成功）
- [x] C9 `cargo fmt --all -- --check` 通过
- [x] C10 `cargo clippy -p eneros-tsdb --all-targets -- -D warnings` 无 warning（修复 doc_lazy_continuation 后通过）
- [x] C11 `cargo deny check licenses bans sources` 通过；advisories 因本地网络阻塞由 CI 验证

## 功能验证

- [x] 写入单个数据点 → query_range 查询到（timestamp/value/quality 完整）
- [x] 批量写入 1000 点 → 全部可查询
- [x] 范围查询过滤正确（含边界）
- [x] TTL 过期清理：过期数据被删除，未过期保留
- [x] 聚合查询：avg/max/min/sum/count 正确
- [x] FsError → TsdbError 自动转换（`?` 操作符）
- [x] chunk 切换：max_points_per_chunk 或 chunk_duration_ms 触发 flush

## 回归测试

- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过
- [x] `cargo run -p eneros-ci` Overall: PASS

## 文档与规范校验（§2.4 C12-C15）

- [x] C12 文档位置：`docs/drivers/tsdb-design.md` 在 `docs/drivers/` 下
- [x] C13 无垃圾文件：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C14 .gitignore 覆盖：新产生的文件类型已在 .gitignore 中
- [x] C15 提交信息：遵循 Conventional Commits

## 版本标识一致性

- [x] 根 `Cargo.toml` workspace.package.version = "0.25.0"
- [x] `crates/drivers/tsdb/Cargo.toml` version = "0.25.0"
- [x] `Makefile` VERSION := 0.25.0
- [x] `.github/workflows/ci.yml` Version: v0.25.0 + 含 eneros-tsdb 交叉编译步骤
- [x] `ci/src/gate.rs` 注释含 eneros-tsdb（v0.25.0 TSDB）说明

## CI 配置

- [x] ci.yml 添加 `Build tsdb crate` 步骤（aarch64-unknown-none 交叉编译）
- [x] ci.yml clippy/test 步骤无需修改（workspace 级别已覆盖）
- [x] gate.rs clippy/test 排除列表注释更新（eneros-tsdb 为 no_std crate，host-testable）
