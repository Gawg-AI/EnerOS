# Tasks

- [x] Task 1: 创建 crate 骨架与 Cargo.toml
  - [x] SubTask 1.1: 创建 `crates/protocols/agent-bus-dds/Cargo.toml`（package name = `eneros-agent-bus-dds`，依赖 `slotmap` with `default-features = false`，`[features] cyclone-dds = []`）
  - [x] SubTask 1.2: 创建 `src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + D1~D12 偏差声明 + 模块声明）
  - [x] SubTask 1.3: 根 `Cargo.toml` 添加 `members` `"crates/protocols/agent-bus-dds"`，版本号 `0.74.0` → `0.75.0`
  - [x] SubTask 1.4: 验证 `cargo metadata --format-version 1` 成功

- [x] Task 2: 实现 error.rs — DdsError
  - [x] SubTask 2.1: 定义 `DdsError` 枚举（8 变体：Ffi(i32) / InvalidHandle / Closed / InconsistentQos(String) / Serialization(String) / TopicNotFound(String) / ParticipantNotFound / Timeout）
  - [x] SubTask 2.2: 派生 `Debug`，实现 `core::fmt::Display`
  - [x] SubTask 2.3: 实现 `core::error::Error`

- [x] Task 3: 实现 config.rs — DdsConfig 与 DiscoveryPolicy
  - [x] SubTask 3.1: 定义 `DiscoveryPolicy` 枚举（Multicast / Unicast / Static）
  - [x] SubTask 3.2: 定义 `DdsConfig` 结构体（domain_id: u32 / discovery: DiscoveryPolicy / interface: Option<String>）
  - [x] SubTask 3.3: 实现 `DdsConfig::default()`（domain_id=0 / Multicast / None）
  - [x] SubTask 3.4: 实现 `DdsConfig::new(domain_id, discovery)` 构造

- [x] Task 4: 实现 qos.rs — QosPolicy
  - [x] SubTask 4.1: 定义 `Reliability` 枚举（BestEffort / Reliable），派生 `Default` = Reliable
  - [x] SubTask 4.2: 定义 `Durability` 枚举（Volatile / TransientLocal），派生 `Default` = Volatile
  - [x] SubTask 4.3: 定义 `History` 枚举（KeepAll / KeepLast），派生 `Default` = KeepLast
  - [x] SubTask 4.4: 定义 `QosPolicy` 结构体（reliability / durability / history / history_depth: i32）
  - [x] SubTask 4.5: 实现 `QosPolicy::default()`（Reliable / Volatile / KeepLast / depth=10）
  - [x] SubTask 4.6: 实现 `QosPolicy::state_default()`（BestEffort / Volatile / KeepLast / depth=1）

- [x] Task 5: 实现 types.rs — DdsSample 与句柄类型
  - [x] SubTask 5.1: 定义 `InstanceHandle` 类型别名（u64）
  - [x] SubTask 5.2: 定义 `DdsSample` 结构体（payload: Vec<u8> / instance_handle: InstanceHandle / source_timestamp: u64）
  - [x] SubTask 5.3: 使用 `slotmap::new_key_type!` 定义 `ParticipantId` / `ReaderId` / `WriterId`

- [x] Task 6: 实现 node.rs — DdsNode trait
  - [x] SubTask 6.1: 定义 `DdsNode` trait（create_participant / create_reader / create_writer / read / take / write / shutdown / is_shutdown）
  - [x] SubTask 6.2: trait 不要求 `Send + Sync`（D2）

- [x] Task 7: 实现 mock.rs — MockDdsNode
  - [x] SubTask 7.1: 定义 `MockParticipant` 结构体（readers: SlotMap / writers: SlotMap）
  - [x] SubTask 7.2: 定义 `MockReader` 结构体（topic / qos / buffer: VecDeque<DdsSample>）
  - [x] SubTask 7.3: 定义 `MockWriter` 结构体（topic / qos / instance_counter: u64）
  - [x] SubTask 7.4: 定义 `MockDdsNode` 结构体（config / participants / shutdown / now_ns / message_bus: BTreeMap<String, VecDeque<DdsSample>>）
  - [x] SubTask 7.5: 实现 `MockDdsNode::new(config)` / `new_default()` / `set_now_ns(now_ns)`
  - [x] SubTask 7.6: 实现 `DdsNode` trait for `MockDdsNode`（create_participant / create_reader / create_writer / read / take / write / shutdown / is_shutdown）
  - [x] SubTask 7.7: write 实现：写入 message_bus[topic]（受 KeepLast 深度限制截断）
  - [x] SubTask 7.8: take 实现：从 message_bus[topic] 取出所有样本并清空；read 实现：取出但不清空
  - [x] SubTask 7.9: shutdown 后所有操作返回 `Err(DdsError::Closed)`

- [x] Task 8: 实现 ffi.rs 与 cyclone_dds.rs（feature-gated）
  - [x] SubTask 8.1: `#[cfg(feature = "cyclone-dds")]` 门控 `ffi` 模块与 `cyclone_dds` 模块
  - [x] SubTask 8.2: ffi.rs 声明 Cyclone DDS C API extern 函数（dds_create_participant / dds_create_writer / dds_write / dds_delete 等），每个 unsafe 块附 SAFETY 注释
  - [x] SubTask 8.3: cyclone_dds.rs 定义 `CycloneDdsNode` 结构体，实现 `DdsNode` trait（FFI 调用 + Drop 释放）
  - [x] SubTask 8.4: 默认 feature 关闭时不编译 ffi/cyclone_dds 模块

- [x] Task 9: 集成测试（lib.rs #[cfg(test)]）
  - [x] SubTask 9.1: T1 DdsError 变体构造与 Display
  - [x] SubTask 9.2: T2 DdsConfig::default 字段验证
  - [x] SubTask 9.3: T3 DdsConfig::new 自定义构造
  - [x] SubTask 9.4: T4 QosPolicy::default 字段验证
  - [x] SubTask 9.5: T5 QosPolicy::state_default 字段验证
  - [x] SubTask 9.6: T6 MockDdsNode::new_default 构造与 is_shutdown=false
  - [x] SubTask 9.7: T7 MockDdsNode create_participant 返回 ParticipantId
  - [x] SubTask 9.8: T8 MockDdsNode create_writer + create_reader 句柄分配
  - [x] SubTask 9.9: T9 MockDdsNode 单节点往返（write → take）
  - [x] SubTask 9.10: T10 MockDdsNode read 不清空 buffer
  - [x] SubTask 9.11: T11 MockDdsNode 跨 topic 隔离
  - [x] SubTask 9.12: T12 MockDdsNode 多 reader 广播语义
  - [x] SubTask 9.13: T13 MockDdsNode KeepLast 深度截断
  - [x] SubTask 9.14: T14 MockDdsNode shutdown 后操作返回 Closed
  - [x] SubTask 9.15: T15 MockDdsNode InvalidHandle（无效 ParticipantId）
  - [x] SubTask 9.16: T16 MockDdsNode set_now_ns 注入时间戳
  - [x] SubTask 9.17: T17 feature 未启用时 CycloneDdsNode 不存在（编译验证）

- [x] Task 10: 配置文件
  - [x] SubTask 10.1: 创建 `configs/dds.toml`（domain_id / discovery / interface / peers）

- [x] Task 11: 设计文档
  - [x] SubTask 11.1: 创建 `docs/protocols/dds-integration-design.md`（12 章节 + 2 Mermaid 图 + D1~D12 偏差声明）

- [x] Task 12: 版本同步与构建校验
  - [x] SubTask 12.1: `Makefile` 版本号 `0.75.0`（header + VERSION 变量）
  - [x] SubTask 12.2: `.github/workflows/ci.yml` 版本号 `0.75.0`
  - [x] SubTask 12.3: `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-agent-bus-dds`
  - [x] SubTask 12.4: `cargo test -p eneros-agent-bus-dds` 全部通过（17/17）
  - [x] SubTask 12.5: `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 12.6: `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
  - [x] SubTask 12.7: `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
  - [x] SubTask 12.8: `cargo deny check licenses bans sources` 通过（deny.toml 添加 Zlib 许可证）

# Task Dependencies

- Task 2 / Task 3 / Task 4 / Task 5 可并行（独立类型定义）
- Task 6 依赖 Task 2 / Task 3 / Task 4 / Task 5（trait 引用类型）
- Task 7 依赖 Task 6（Mock 实现 trait）
- Task 8 依赖 Task 6（FFI 实现 trait，feature-gated）
- Task 9 依赖 Task 7 / Task 8（测试覆盖实现）
- Task 1 / Task 10 / Task 11 可与其他任务并行（骨架 / 配置 / 文档）
- Task 12 依赖所有前置任务完成
