# Checklist

## Workspace 同步
- [x] C1 根 `Cargo.toml` 版本号已更新为 `0.75.0`
- [x] C2 members 列表已添加 `crates/protocols/agent-bus-dds`（置于 `crates/protocols/soe-engine` 之后）
- [x] C3 `cargo metadata --format-version 1` 成功

## Crate 骨架
- [x] C4 `crates/protocols/agent-bus-dds/Cargo.toml` 存在，package name = `eneros-agent-bus-dds`
- [x] C5 dependencies 包含 `slotmap`（`default-features = false`）
- [x] C6 `[features]` 段声明 `default = []` + `cyclone-dds = []`
- [x] C7 `src/lib.rs` 包含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C8 `src/lib.rs` 包含 D1~D12 偏差声明表
- [x] C9 模块声明：error / config / qos / types / node / mock + `#[cfg(feature = "cyclone-dds")] ffi` + `cyclone_dds`

## error.rs — DdsError
- [x] C10 `DdsError` 枚举 8 变体（Ffi(i32) / InvalidHandle / Closed / InconsistentQos(String) / Serialization(String) / TopicNotFound(String) / ParticipantNotFound / Timeout）
- [x] C11 派生 `Debug`
- [x] C12 实现 `core::fmt::Display`
- [x] C13 实现 `core::error::Error`

## config.rs — DdsConfig 与 DiscoveryPolicy
- [x] C14 `DiscoveryPolicy` 枚举（Multicast / Unicast / Static）
- [x] C15 `DdsConfig` 结构体（domain_id: u32 / discovery: DiscoveryPolicy / interface: Option<String>）
- [x] C16 `DdsConfig::default()`（domain_id=0 / Multicast / None）
- [x] C17 `DdsConfig::new(domain_id, discovery)` 构造

## qos.rs — QosPolicy
- [x] C18 `Reliability` 枚举（BestEffort / Reliable），`#[default]` = Reliable
- [x] C19 `Durability` 枚举（Volatile / TransientLocal），`#[default]` = Volatile
- [x] C20 `History` 枚举（KeepAll / KeepLast），`#[default]` = KeepLast
- [x] C21 `QosPolicy` 结构体（reliability / durability / history / history_depth: i32）
- [x] C22 `QosPolicy::default()`（Reliable / Volatile / KeepLast / depth=10）
- [x] C23 `QosPolicy::state_default()`（BestEffort / Volatile / KeepLast / depth=1）

## types.rs — DdsSample 与句柄
- [x] C24 `InstanceHandle` 类型别名（u64）
- [x] C25 `DdsSample` 结构体（payload: Vec<u8> / instance_handle: InstanceHandle / source_timestamp: u64）
- [x] C26 `slotmap::new_key_type!` 定义 `ParticipantId` / `ReaderId` / `WriterId`

## node.rs — DdsNode trait
- [x] C27 `DdsNode` trait 定义（create_participant / create_reader / create_writer / read / take / write / shutdown / is_shutdown）
- [x] C28 trait 不要求 `Send + Sync`（D2）
- [x] C29 方法签名与 spec 一致

## mock.rs — MockDdsNode
- [x] C30 `MockParticipant` 结构体（readers: SlotMap / writers: SlotMap）
- [x] C31 `MockReader` 结构体（topic / qos / buffer: VecDeque<DdsSample>）
- [x] C32 `MockWriter` 结构体（topic / qos / instance_counter: u64）
- [x] C33 `MockDdsNode` 结构体（config / participants / shutdown / now_ns / message_bus: BTreeMap<String, VecDeque<DdsSample>>）
- [x] C34 `MockDdsNode::new(config)` / `new_default()` / `set_now_ns(now_ns)`
- [x] C35 `DdsNode` trait for `MockDdsNode` 实现
- [x] C36 write 实现：写入 message_bus[topic]，KeepLast 深度截断
- [x] C37 take 实现：取出并清空；read 实现：取出不清空
- [x] C38 shutdown 后所有操作返回 `Err(DdsError::Closed)`

## ffi.rs + cyclone_dds.rs（feature-gated）
- [x] C39 `#[cfg(feature = "cyclone-dds")]` 门控 `ffi` 模块
- [x] C40 `#[cfg(feature = "cyclone-dds")]` 门控 `cyclone_dds` 模块
- [x] C41 ffi.rs 声明 Cyclone DDS C API extern 函数，每个 unsafe 块附 SAFETY 注释
- [x] C42 cyclone_dds.rs 定义 `CycloneDdsNode`，实现 `DdsNode` trait + Drop
- [x] C43 默认 feature 关闭时不编译 ffi/cyclone_dds 模块

## 集成测试（lib.rs）
- [x] C44 T1 DdsError 变体构造与 Display
- [x] C45 T2 DdsConfig::default 字段验证
- [x] C46 T3 DdsConfig::new 自定义构造
- [x] C47 T4 QosPolicy::default 字段验证
- [x] C48 T5 QosPolicy::state_default 字段验证
- [x] C49 T6 MockDdsNode::new_default 构造与 is_shutdown=false
- [x] C50 T7 MockDdsNode create_participant 返回 ParticipantId
- [x] C51 T8 MockDdsNode create_writer + create_reader 句柄分配
- [x] C52 T9 MockDdsNode 单节点往返（write → take）
- [x] C53 T10 MockDdsNode read 不清空 buffer
- [x] C54 T11 MockDdsNode 跨 topic 隔离
- [x] C55 T12 MockDdsNode 多 reader 广播语义
- [x] C56 T13 MockDdsNode KeepLast 深度截断
- [x] C57 T14 MockDdsNode shutdown 后操作返回 Closed
- [x] C58 T15 MockDdsNode InvalidHandle（无效 ParticipantId）
- [x] C59 T16 MockDdsNode set_now_ns 注入时间戳
- [x] C60 T17 feature 未启用时 CycloneDdsNode 不存在（编译验证）
- [x] C61 `cargo test -p eneros-agent-bus-dds` 全部通过（17/17）

## 配置文件
- [x] C62 `configs/dds.toml` 存在
- [x] C63 包含 domain_id / discovery / interface / peers 字段

## 设计文档
- [x] C64 `docs/protocols/dds-integration-design.md` 存在
- [x] C65 12 章节完整
- [x] C66 2 Mermaid 图（DDS 节点创建时序图 + Mock 发布订阅往返流程图）
- [x] C67 D1~D12 偏差声明表
- [x] C68 文档在 `docs/protocols/` 下

## 版本同步
- [x] C69 `Makefile` 版本号 `0.75.0`（header + VERSION 变量 2 处）
- [x] C70 `.github/workflows/ci.yml` 版本号 `0.75.0`
- [x] C71 `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-agent-bus-dds`

## 构建校验（§2.4.2 C6~C11）
- [x] C72 `cargo metadata --format-version 1` 成功
- [x] C73 `cargo test -p eneros-agent-bus-dds` 全部通过（17/17）
- [x] C74 `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C75 `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
- [x] C76 `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
- [x] C77 `cargo deny check licenses bans sources` 通过（deny.toml 添加 Zlib 许可证）

## no_std 合规
- [x] C78 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C79 无 `panic!` / `todo!` / `unimplemented!`
- [x] C80 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C81 默认 feature 下无 `unsafe` 块（仅 `cyclone-dds` feature 启用时 ffi 模块含 unsafe）
- [x] C82 无 `SystemTime::now()` / `thread::sleep`（D11：用 `set_now_ns` 注入）
- [x] C83 无 `std::collections::HashMap` / `std::sync::Mutex`（用 `alloc::collections::BTreeMap`）

## 目录规范
- [x] C84 crate 在 `crates/protocols/agent-bus-dds/`
- [x] C85 跨 crate path 引用均为相对路径（本 crate 无跨 crate 依赖）
- [x] C86 文档在 `docs/protocols/` 下
- [x] C87 配置在 `configs/` 下
- [x] C88 无根目录 crate（除 `ci/`）
- [x] C89 无垃圾文件

## 简化设计验证（Karpathy 原则）
- [x] C90 Mock 默认可用（不依赖 C 库，可交叉编译）
- [x] C91 合并 reader/writer trait 为 DdsNode（单一结构体更简单）
- [x] C92 简化 DiscoveryPolicy（三态 enum，不嵌入 IP 地址）
- [x] C93 简化 QosPolicy（结构体非 builder，v0.76.0 扩展）
- [x] C94 无 panic hook 安装（no_std 由 #[panic_handler] 处理）
- [x] C95 无 valgrind 集成（Mock 无 FFI 内存泄漏）
- [x] C96 无跨机集成测试（Mock 仅验证单节点往返）
- [x] C97 无外科手术式变更（仅新增 crate + 版本同步 + deny.toml 添加 Zlib 许可证）
