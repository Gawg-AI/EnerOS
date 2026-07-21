# Tasks

- [x] Task 1: 同步 workspace 版本号与 members 列表
  - [x] 修改 `e:\eneros\Cargo.toml`：`version = "0.52.0"` → `version = "0.53.0"`
  - [x] 在 members 中追加：`crates/protocols/soe-engine` / `crates/protocols/mqtt` / `crates/agents/alarm`
  - 验证：`cargo metadata --format-version 1` 成功（METADATA_OK）

- [x] Task 2: 创建 v0.53.0 soe-engine crate 骨架
  - [x] `Cargo.toml` + `lib.rs` + `error.rs` + `config.rs` + D1~D10 偏差声明表
  - 验证：编译通过

- [x] Task 3: 实现 v0.53.0 SoeEvent + SoeEventType + EventPriority
  - [x] `src/event.rs`：11 变体 SoeEventType + 4 级 EventPriority + SoeEvent 11 字段 + new/is_critical
  - 验证：编译通过

- [x] Task 4: 实现 v0.53.0 SoeStorage trait + UploadChannel trait
  - [x] `src/storage.rs`：SoeStorage trait + InMemorySoeStorage
  - [x] `src/upload.rs`：UploadChannel trait + MockUploadChannel
  - 验证：编译通过

- [x] Task 5: 实现 v0.53.0 EventTrigger trait + 两个内置触发器
  - [x] `src/trigger.rs`：EventTrigger trait + DigitalChangeTrigger + OverLimitTrigger
  - 验证：编译通过

- [x] Task 6: 实现 v0.53.0 SoeEngine 引擎
  - [x] `src/engine.rs`：SoeEngine + EventByTimestamp（BinaryHeap reversed-Ord）+ 全部方法
  - 验证：编译通过

- [x] Task 7: v0.53.0 集成测试（T1~T20）
  - [x] 20 个集成测试全部通过（20 passed; 0 failed）

- [x] Task 8: v0.53.0 设计文档
  - [x] `docs/protocols/soe-engine-design.md` — 12 章节 + Mermaid 架构图 + 流程图

- [x] Task 9: 创建 v0.53.1 mqtt crate 骨架 + 类型
  - [x] `Cargo.toml` + `lib.rs` + error/qos/will/topic 模块 + D11~D18 偏差声明
  - 验证：编译通过

- [x] Task 10: 实现 v0.53.1 MQTT 报文编解码
  - [x] `src/packet.rs`：14 种控制报文 + encode/decode + 变长剩余长度
  - 验证：编译通过

- [x] Task 11: 实现 v0.53.1 MqttTransport + MqttClient
  - [x] `src/transport.rs`：MqttTransport trait + MockTransport
  - [x] `src/client.rs`：MqttClient 状态机 + QoS 0/1/2 + 指数退避重连 + 恢复订阅
  - 验证：编译通过

- [x] Task 12: v0.53.1 集成测试 + 设计文档
  - [x] 15 个集成测试 + 19 个模块级单元测试（共 34 个，全部通过）
  - [x] `docs/protocols/mqtt-client-design.md` — 12 章节 + Mermaid 状态机图 + QoS 2 时序图

- [x] Task 13: 创建 v0.53.2 alarm crate + 完整实现
  - [x] `Cargo.toml` + `lib.rs` + error/level/record/suppression/escalation/manager 全模块
  - [x] D19~D25 偏差声明表
  - 验证：编译通过

- [x] Task 14: v0.53.2 集成测试 + 设计文档
  - [x] 15 个集成测试全部通过（15 passed; 0 failed）
  - [x] `docs/runtime/alarm-management-design.md` — 12 章节 + Mermaid 生命周期图 + 升级流程图

- [x] Task 15: 版本同步 + 构建校验
  - [x] `Makefile`：0.52.0 → 0.53.0
  - [x] `.github/workflows/ci.yml`：0.52.0 → 0.53.0
  - [x] `ci/src/gate.rs`：补充 v0.53.0/v0.53.1/v0.53.2 三个 crate 注释（clippy + test 两段）
  - [x] `cargo metadata --format-version 1` — METADATA_OK
  - [x] `cargo test -p eneros-soe-engine -p eneros-mqtt -p eneros-alarm` — 20 + 34 + 15 = 69 passed; 0 failed
  - [x] `cargo build --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` — CROSS_COMPILE_OK
  - [x] `cargo fmt -- --check` — FMT_OK
  - [x] `cargo clippy --all-targets -- -D warnings` — CLIPPY_OK
  - [x] `cargo deny check advisories licenses bans sources` — DENY_OK（advisories ok, bans ok, licenses ok, sources ok）

# Task Dependencies
- Task 1 独立（workspace 准备）
- Task 2 依赖 Task 1
- Task 3 依赖 Task 2
- Task 4 依赖 Task 3
- Task 5 依赖 Task 3
- Task 6 依赖 Task 4+5
- Task 7 依赖 Task 6
- Task 8 依赖 Task 7
- Task 9 独立（与 Task 2~8 并行）
- Task 10 依赖 Task 9
- Task 11 依赖 Task 10
- Task 12 依赖 Task 11
- Task 13 独立（与 Task 2~12 并行）
- Task 14 依赖 Task 13
- Task 15 依赖全部完成

# 可并行执行分组
- **Group A**（v0.53.0 SOE）：Task 2 → 3 → (4 ∥ 5) → 6 → 7 → 8 ✅
- **Group B**（v0.53.1 MQTT）：Task 9 → 10 → 11 → 12 ✅（与 Group A 并行）
- **Group C**（v0.53.2 Alarm）：Task 13 → 14 ✅（与 Group A/B 并行）
- **Group D**（同步 + 校验）：Task 15 ✅（最后）
