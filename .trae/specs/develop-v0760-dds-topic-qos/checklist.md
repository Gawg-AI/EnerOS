# Checklist

## qos.rs 修改（BREAKING）
- [x] C1 `History` 枚举修改：`KeepLast` → `KeepLast(u32)`，移除 `#[default]`
- [x] C2 `QosPolicy` 结构体修改：移除 `history_depth: i32`，新增 `deadline: Option<Duration>` / `lifespan: Option<Duration>` / `priority: i32`
- [x] C3 `QosPolicy::default()` 返回 `KeepLast(10)` + `deadline=None` + `lifespan=None` + `priority=0`
- [x] C4 `QosPolicy::state_default()` 返回 `KeepLast(1)` + `lifespan=5s` + `priority=0`
- [x] C5 新增 `QosPolicy::command_default()` 返回 `Reliable` + `TransientLocal` + `KeepAll` + `deadline=2s` + `lifespan=10s` + `priority=6`
- [x] C6 新增 `QosPolicy::alert_default()` 返回 `Reliable` + `TransientLocal` + `KeepLast(10)` + `priority=7`
- [x] C7 移除 `QosPolicy` 的 `#[derive(Default)]`，改为手动 `impl Default`

## mock.rs 适配
- [x] C8 `MockDdsNode::write()` KeepLast 截断逻辑改为 `if let History::KeepLast(depth) = r.qos.history`

## topic.rs — 新建
- [x] C9 `TopicCategory` 枚举（State / Command / Alert / Twin / Market / Log），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C10 `PayloadType` 枚举（Json / Bincode / Cdr），派生 `Debug, Clone, Copy, PartialEq, Eq`
- [x] C11 `TopicSpec` 结构体（name / category / payload_type / default_qos / ttl），派生 `Debug, Clone`
- [x] C12 `TopicError` 枚举（InvalidName(String) / Conflict { name: String } / InvalidQos(String)），派生 `Debug`，实现 `Display` + `core::error::Error`
- [x] C13 `validate_topic_name()` 实现：以 `/` 开头，仅含 `[a-zA-Z0-9_/{}` 字符
- [x] C14 `standard_topics()` 返回 8 个标准预置 Topic

## registry.rs — 新建
- [x] C15 `TopicRegistry` 结构体（specs: `BTreeMap<String, TopicSpec>`）
- [x] C16 `TopicRegistry::new()` 空注册表
- [x] C17 `TopicRegistry::with_standards()` 预加载 8 个标准 Topic
- [x] C18 `TopicRegistry::register(spec)` 校验 + 查重 + 插入
- [x] C19 `TopicRegistry::lookup(name)` 返回 `Option<&TopicSpec>`
- [x] C20 `TopicRegistry::match_pattern(pattern)` 简化通配符匹配（仅 `*` 后缀）

## lib.rs — 模块声明 + 导出 + 测试
- [x] C21 添加 `pub mod topic;` + `pub mod registry;`
- [x] C22 添加 `pub use topic::{...}` + `pub use registry::TopicRegistry;`
- [x] C23 更新偏差声明表（v0.76.0 D1~D12）
- [x] C24 T4 修改：`qos.history == History::KeepLast(10)` + 新字段验证
- [x] C25 T5 修改：`qos.history == History::KeepLast(1)` + lifespan=5s, priority=0
- [x] C26 T13 修改：构造 `QosPolicy` 使用 `KeepLast(2)` + 新字段
- [x] C27 T18 新增：validate_topic_name 合法 topic 名
- [x] C28 T19 新增：validate_topic_name 非法 topic 名（3 种非法情况）
- [x] C29 T20 新增：QosPolicy::command_default() 字段验证
- [x] C30 T21 新增：QosPolicy::alert_default() 字段验证
- [x] C31 T22 新增：standard_topics() 返回 8 个标准 Topic
- [x] C32 T23 新增：TopicRegistry::with_standards() 预加载 8 个 Topic
- [x] C33 T24 新增：TopicRegistry::register() 注册新 Topic 成功
- [x] C34 T25 新增：TopicRegistry::register() 重复注册同名且 QoS 一致 → Ok
- [x] C35 T26 新增：TopicRegistry::register() 重复注册同名且 QoS 不一致 → Err(Conflict)
- [x] C36 T27 新增：TopicRegistry::register() 非法 topic 名 → Err(InvalidName)
- [x] C37 T28 新增：TopicRegistry::lookup() 查询已注册 Topic
- [x] C38 T29 新增：TopicRegistry::lookup() 查询未注册 Topic → None
- [x] C39 T30 新增：TopicRegistry::match_pattern() 通配符匹配
- [x] C40 T31 新增：MockDdsNode with KeepAll — write 3 条不截断
- [x] C41 `cargo test -p eneros-agent-bus-dds` 全部通过（31 个测试 + 1 doctest）

## 配置文件
- [x] C42 `configs/topics.toml` 存在
- [x] C43 包含 8 个标准 Topic 的 TOML 配置

## 设计文档
- [x] C44 `docs/protocols/dds-topic-qos-design.md` 存在
- [x] C45 12 章节完整
- [x] C46 2 Mermaid 图（Topic 注册流程图 + QoS 分级策略矩阵图）
- [x] C47 D1~D12 偏差声明表
- [x] C48 文档在 `docs/protocols/` 下

## 版本同步
- [x] C49 根 `Cargo.toml` 版本号 `0.76.0`
- [x] C50 `Makefile` 版本号 `0.76.0`（header + VERSION 变量）
- [x] C51 `.github/workflows/ci.yml` 版本号 `0.76.0`
- [x] C52 `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-agent-bus-dds v0.76.0`

## 构建校验（§2.4.2 C6~C11）
- [x] C53 `cargo metadata --format-version 1` 成功
- [x] C54 `cargo test -p eneros-agent-bus-dds` 全部通过（31 个测试 + 1 doctest）
- [x] C55 `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C56 `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
- [x] C57 `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
- [x] C58 `cargo deny check licenses bans sources` 通过

## no_std 合规
- [x] C59 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] C60 无 `panic!` / `todo!` / `unimplemented!`
- [x] C61 子模块不重复 `#![cfg_attr(not(test), no_std)]`
- [x] C62 无 `once_cell::sync::Lazy`（D8：用普通函数）
- [x] C63 无 `regex` 依赖（D4：手动通配符匹配）
- [x] C64 无 `std::collections::HashMap`（D1：用 `BTreeMap`）
- [x] C65 使用 `core::time::Duration`（非 `std::time::Duration`）

## 目录规范
- [x] C66 crate 在 `crates/protocols/agent-bus-dds/`（扩展现有 crate）
- [x] C67 文档在 `docs/protocols/` 下
- [x] C68 配置在 `configs/` 下
- [x] C69 无根目录 crate（除 `ci/`）
- [x] C70 无垃圾文件

## 简化设计验证（Karpathy 原则）
- [x] C71 无 `regex` 依赖（手动 `*` 通配符匹配）
- [x] C72 无 `once_cell::sync::Lazy`（普通函数）
- [x] C73 无 `toml` 运行时解析（配置模板仅文档）
- [x] C74 无 CDR 编码实现（仅枚举定义）
- [x] C75 无 QoS 兼容性强制校验（Mock 继承 v0.75.0 策略）
- [x] C76 `History::KeepLast(u32)` 替代 `history_depth` 独立字段
- [x] C77 扩展现有 crate 而非新建
- [x] C78 破坏性变更仅限于必要的 v0.75.0 适配（qos.rs/mock.rs/3 个测试）
