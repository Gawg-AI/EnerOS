# Tasks

- [x] Task 1: 修改 qos.rs — History 枚举与 QosPolicy 扩展（**BREAKING**）
  - [x] SubTask 1.1: 修改 `History` 枚举：`KeepLast` → `KeepLast(u32)`，移除 `#[default]`（带参数的变体不能 derive Default）
  - [x] SubTask 1.2: 修改 `QosPolicy` 结构体：移除 `history_depth: i32`，新增 `deadline: Option<core::time::Duration>` / `lifespan: Option<core::time::Duration>` / `priority: i32`
  - [x] SubTask 1.3: 修改 `QosPolicy::default()`：`KeepLast(10)` + `deadline=None` + `lifespan=None` + `priority=0`
  - [x] SubTask 1.4: 修改 `QosPolicy::state_default()`：`KeepLast(1)` + `lifespan=5s` + `priority=0`
  - [x] SubTask 1.5: 新增 `QosPolicy::command_default()`：`Reliable` + `TransientLocal` + `KeepAll` + `deadline=2s` + `lifespan=10s` + `priority=6`
  - [x] SubTask 1.6: 新增 `QosPolicy::alert_default()`：`Reliable` + `TransientLocal` + `KeepLast(10)` + `priority=7`
  - [x] SubTask 1.7: 移除 `QosPolicy` 的 `#[derive(Default)]`（手动 impl Default，因 History 不再 derive Default）

- [x] Task 2: 修改 mock.rs — 适配 History::KeepLast(u32)
  - [x] SubTask 2.1: 修改 `MockDdsNode::write()` 的 KeepLast 截断逻辑：从 `r.qos.history == History::KeepLast && r.qos.history_depth > 0` 改为 `if let History::KeepLast(depth) = r.qos.history`

- [x] Task 3: 新建 topic.rs — TopicSpec / TopicCategory / PayloadType / validate_topic_name / standard_topics / TopicError
  - [x] SubTask 3.1: 定义 `TopicCategory` 枚举（State / Command / Alert / Twin / Market / Log），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.2: 定义 `PayloadType` 枚举（Json / Bincode / Cdr），派生 `Debug, Clone, Copy, PartialEq, Eq`
  - [x] SubTask 3.3: 定义 `TopicSpec` 结构体（name / category / payload_type / default_qos / ttl: Option<core::time::Duration>），派生 `Debug, Clone`
  - [x] SubTask 3.4: 定义 `TopicError` 枚举（InvalidName(String) / Conflict { name: String } / InvalidQos(String)），派生 `Debug`，实现 `Display` + `core::error::Error`
  - [x] SubTask 3.5: 实现 `validate_topic_name(name: &str) -> Result<(), TopicError>`：必须以 `/` 开头，仅含 `[a-zA-Z0-9_/{}` 字符
  - [x] SubTask 3.6: 实现 `standard_topics() -> Vec<TopicSpec>`：返回 8 个标准预置 Topic（battery/pv/grid/market price/market signal/command internal/alert fault/twin update）

- [x] Task 4: 新建 registry.rs — TopicRegistry
  - [x] SubTask 4.1: 定义 `TopicRegistry` 结构体（specs: `BTreeMap<String, TopicSpec>`）
  - [x] SubTask 4.2: 实现 `TopicRegistry::new()` — 空注册表
  - [x] SubTask 4.3: 实现 `TopicRegistry::with_standards()` — 预加载 `standard_topics()` 的 8 个 Topic
  - [x] SubTask 4.4: 实现 `TopicRegistry::register(spec)` — 校验 topic 名 → 查重（同名且 QoS 不一致返回 Conflict，一致返回 Ok）→ 插入
  - [x] SubTask 4.5: 实现 `TopicRegistry::lookup(name)` — 按 name 查询返回 `Option<&TopicSpec>`
  - [x] SubTask 4.6: 实现 `TopicRegistry::match_pattern(pattern)` — 简化通配符匹配（仅支持 `*` 后缀通配，如 `/power/state/*`）

- [x] Task 5: 修改 lib.rs — 模块声明 + 重新导出 + 测试更新
  - [x] SubTask 5.1: 添加 `pub mod topic;` + `pub mod registry;` 模块声明
  - [x] SubTask 5.2: 添加 `pub use topic::{TopicCategory, TopicError, TopicSpec, PayloadType, validate_topic_name, standard_topics};` + `pub use registry::TopicRegistry;`
  - [x] SubTask 5.3: 更新偏差声明表（D1~D12 for v0.76.0）
  - [x] SubTask 5.4: 修改现有 T4 测试：`qos.history_depth == 10` → `qos.history == History::KeepLast(10)` + 验证新字段（deadline=None, lifespan=None, priority=0）
  - [x] SubTask 5.5: 修改现有 T5 测试：`qos.history_depth == 1` → `qos.history == History::KeepLast(1)` + 验证 lifespan=5s, priority=0
  - [x] SubTask 5.6: 修改现有 T13 测试：构造 `QosPolicy { history: History::KeepLast(2), deadline: None, lifespan: None, priority: 0 }`（移除 history_depth，新增新字段）
  - [x] SubTask 5.7: 新增 T18：validate_topic_name 合法 topic 名
  - [x] SubTask 5.8: 新增 T19：validate_topic_name 非法 topic 名（不以/开头、含空格、含特殊字符）
  - [x] SubTask 5.9: 新增 T20：QosPolicy::command_default() 字段验证
  - [x] SubTask 5.10: 新增 T21：QosPolicy::alert_default() 字段验证
  - [x] SubTask 5.11: 新增 T22：standard_topics() 返回 8 个标准 Topic
  - [x] SubTask 5.12: 新增 T23：TopicRegistry::with_standards() 预加载 8 个 Topic
  - [x] SubTask 5.13: 新增 T24：TopicRegistry::register() 注册新 Topic 成功
  - [x] SubTask 5.14: 新增 T25：TopicRegistry::register() 重复注册同名且 QoS 一致 → Ok
  - [x] SubTask 5.15: 新增 T26：TopicRegistry::register() 重复注册同名且 QoS 不一致 → Err(Conflict)
  - [x] SubTask 5.16: 新增 T27：TopicRegistry::register() 非法 topic 名 → Err(InvalidName)
  - [x] SubTask 5.17: 新增 T28：TopicRegistry::lookup() 查询已注册 Topic
  - [x] SubTask 5.18: 新增 T29：TopicRegistry::lookup() 查询未注册 Topic → None
  - [x] SubTask 5.19: 新增 T30：TopicRegistry::match_pattern() 通配符匹配
  - [x] SubTask 5.20: 新增 T31：MockDdsNode with KeepAll — write 3 条不截断

- [x] Task 6: 配置文件
  - [x] SubTask 6.1: 创建 `configs/topics.toml`（8 个标准 Topic 的 TOML 配置模板）

- [x] Task 7: 设计文档
  - [x] SubTask 7.1: 创建 `docs/protocols/dds-topic-qos-design.md`（12 章节 + 2 Mermaid 图 + D1~D12 偏差声明）

- [x] Task 8: 版本同步与构建校验
  - [x] SubTask 8.1: 根 `Cargo.toml` 版本号 `0.75.0` → `0.76.0`
  - [x] SubTask 8.2: `Makefile` 版本号 `0.76.0`（header + VERSION 变量）
  - [x] SubTask 8.3: `.github/workflows/ci.yml` 版本号 `0.76.0`
  - [x] SubTask 8.4: `ci/src/gate.rs` clippy 段 + test 段注释更新 `eneros-agent-bus-dds v0.76.0`
  - [x] SubTask 8.5: `cargo metadata --format-version 1` 成功
  - [x] SubTask 8.6: `cargo test -p eneros-agent-bus-dds` 全部通过（31 个测试 + 1 doctest）
  - [x] SubTask 8.7: `cargo build -p eneros-agent-bus-dds --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.8: `cargo fmt -p eneros-agent-bus-dds -- --check` 通过
  - [x] SubTask 8.9: `cargo clippy -p eneros-agent-bus-dds --all-targets -- -D warnings` 无 warning
  - [x] SubTask 8.10: `cargo deny check licenses bans sources` 通过

# Task Dependencies

- Task 1（qos.rs 修改）必须先完成 — Task 2/3/4/5 依赖新的 History/QosPolicy
- Task 2（mock.rs 适配）依赖 Task 1
- Task 3（topic.rs）依赖 Task 1（QosPolicy 类型）
- Task 4（registry.rs）依赖 Task 3（TopicSpec/TopicError）
- Task 5（lib.rs）依赖 Task 1/2/3/4 全部完成
- Task 6/7 可与其他任务并行（配置文件 / 文档）
- Task 8 依赖所有前置任务完成
