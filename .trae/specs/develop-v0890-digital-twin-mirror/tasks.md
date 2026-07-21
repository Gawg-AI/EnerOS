# Tasks

- [x] Task 1: 创建 crate 骨架 `crates/agents/twin-agent/`（Cargo.toml + lib.rs）
  - [x] SubTask 1.1: `Cargo.toml` — package `eneros-twin-agent`（version/edition/authors/license workspace）+ description（v0.89.0 数字孪生数据镜像, no_std）+ dependencies：`eneros-agent-bus-dds = { path = "../../protocols/agent-bus-dds" }` / `eneros-grid-agent = { path = "../grid_agent" }` / `eneros-device-agent = { path = "../device-agent" }` / `serde = { version = "1", default-features = false, features = ["alloc", "derive"] }` / `serde_json = { version = "1", default-features = false, features = ["alloc"] }`
  - [x] SubTask 1.2: 根 `Cargo.toml` workspace `members` 追加 `"crates/agents/twin-agent"`（其他行不动）
  - [x] SubTask 1.3: `src/lib.rs` — `#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + `pub mod mirror; pub mod model;` + 中文 crate 文档（版本目标 + 偏差 D1~D12 简表）+ 重导出（DeviceTwin, MarketMirror, TwinModel, TwinSnapshot, TwinError, TwinMirror）
  - [x] SubTask 1.4: 空模块 `src/model.rs` / `src/mirror.rs` 占位后 `cargo metadata --format-version 1` 通过

- [x] Task 2: 实现 `src/model.rs` — 数据结构 + 测试 T1~T6
  - [x] SubTask 2.1: `MarketMirror { timestamp: u64, current_price: f32 }`（Debug, Clone, Copy, PartialEq, Default，中文字段 doc）
  - [x] SubTask 2.2: `DeviceTwin { device_id: u64, state: DeviceState }`（Debug, Clone, PartialEq, Default；`use eneros_device_agent::DeviceState;`）
  - [x] SubTask 2.3: `TwinModel { devices: BTreeMap<u64, DeviceTwin>, grid: GridState, market: Option<MarketMirror>, last_update: u64 }`（Debug, Clone, Default）+ `device_count()`；`use eneros_grid_agent::GridState;`
  - [x] SubTask 2.4: `TwinSnapshot { timestamp: u64, model: TwinModel }`（Debug, Clone）+ `summary_json(&self) -> String`（serde_json 序列化本地 DTO：timestamp/last_update/device_count/grid_timestamp/market_timestamp）
  - [x] SubTask 2.5: 中文模块文档（D2/D6/D7/D9/D10 引用）；无 std/panic!/unsafe/todo!/unimplemented!
  - [x] SubTask 2.6: T1 — `MarketMirror::default()` 全零；Copy 语义
  - [x] SubTask 2.7: T2 — `DeviceTwin::default()`：device_id==0，state 默认（soc==0.0 / online==false / last_update_ms==0）
  - [x] SubTask 2.8: T3 — `TwinModel::default()`：devices 空 / market None / last_update==0 / device_count()==0 / grid 全零
  - [x] SubTask 2.9: T4 — devices 乱序插入 30/10/20 → keys 顺序 [10,20,30]（D2）
  - [x] SubTask 2.10: T5 — `TwinSnapshot` 构造 + Clone 一致（timestamp + model.devices 长度）
  - [x] SubTask 2.11: T6 — `summary_json()`：含 `"device_count":2` / `"last_update":1234` / grid_timestamp / market None → 无 market_timestamp 或为 null（以实现为准断言）；返回值可被 `serde_json::from_str::<serde_json::Value>` 解析

- [x] Task 3: 实现 `src/mirror.rs` — TwinError + TwinMirror::new/apply_update + 测试 T7~T30
  - [x] SubTask 3.1: `TwinError { Dds(DdsError) }`（Debug；D8 单变体；From<DdsError> 可选）
  - [x] SubTask 3.2: payload DTO（serde Deserialize）：`GridPayload`（frequency/voltage_a~c/current_a~c/active_power/reactive_power/power_factor 全 `Option<f32>` + `timestamp: Option<u64>`）/ `DevicePayload`（soc/voltage/current/temperature/power 全 `Option<f64>` + `online: Option<bool>` + `last_update_ms: Option<u64>`）/ `MarketPayload { timestamp: u64, current_price: f32 }`
  - [x] SubTask 3.3: `TwinMirror` 结构体 10 字段（model/node/participant/readers/writer/publish_interval_ms/last_publish_ms/applied_count/skipped_count/published_count）
  - [x] SubTask 3.4: `new(node, topics, publish_interval_ms)`：create_participant → 逐 topic create_reader（QosPolicy::default()）→ create_writer("/power/twin/update")；失败 → `Err(TwinError::Dds)`；空 topics 合法（readers 空）
  - [x] SubTask 3.5: `apply_update` — grid 分支：JSON 解析失败 → false+skipped；payload.timestamp < model.grid.timestamp → false+skipped；逐 Option 字段 merge（Some 才覆盖）；grid.timestamp = payload.timestamp 若 Some；true+applied+last_update=now_ms
  - [x] SubTask 3.6: `apply_update` — battery 分支：`topic.strip_prefix("/power/state/battery/")` → `parse::<u64>()` 失败 → false+skipped；entry.or_default()；payload.last_update_ms < entry.state.last_update_ms → false+skipped；逐字段 merge；state.last_update_ms = now_ms
  - [x] SubTask 3.7: `apply_update` — market 分支：解析失败/字段缺失 → false+skipped；timestamp < 现有 market.timestamp → false+skipped；设置 Some(MarketMirror)
  - [x] SubTask 3.8: `apply_update` — 未知 topic → false+skipped；`snapshot()`（timestamp = model.last_update）
  - [x] SubTask 3.9: 中文模块文档（D3/D4/D5/D8/D9/D11 引用）；use 仅 alloc + core + serde/serde_json + 3 个 workspace crate；主代码无 unwrap/std/async
  - [x] SubTask 3.10: T7~T12 — grid：全字段更新 / 仅 active_power 其余保留 / 无效 JSON false / 过期 timestamp false 且 model 不变 / 同 timestamp 接受 / grid.timestamp 更新
  - [x] SubTask 3.11: T13~T19 — battery：新设备 id=7 soc=0.8 power=1.5 last_update_ms==now_ms / 二次部分合并 soc 保留 / 无效 id（"/power/state/battery/abc"）false / 过期 last_update_ms false / online=true 合并 / applied_count/skipped_count 正确
  - [x] SubTask 3.12: T20~T23 — market：正常设置 Some / 第二次缺字段 JSON false 保留旧值 / 无效 JSON false / 过期 timestamp false
  - [x] SubTask 3.13: T24~T26 — 未知 topic false / last_update==now_ms / applied+skipped 与调用次数一致
  - [x] SubTask 3.14: T27~T30 — snapshot 字段一致 / snapshot.timestamp==model.last_update / 快照 clone 后改原 model 不影响快照 / summary_json 经 snapshot 调用正确

- [x] Task 4: 实现 `src/mirror.rs` — on_tick/publish + 测试 T31~T40（MockDdsNode 端到端）
  - [x] SubTask 4.1: `on_tick(now_ms)`：逐 reader take(100) → apply_update（先收集 (topic, samples) 避免借用冲突）→ 周期判定 `now_ms - last_publish_ms >= publish_interval_ms` → publish + 更新 last_publish_ms → Ok(true)，否则 Ok(false)
  - [x] SubTask 4.2: `publish()`：摘要 DTO（snapshot.summary_json 字段 + applied_count/skipped_count/published_count）→ serde_json::to_vec → node.write → published_count+=1；write 失败 → Err(Dds)
  - [x] SubTask 4.3: 测试辅助：构造 MockDdsNode + 外部 writer（向 `/power/state/grid`、`/power/state/battery/1` 写 JSON 样本）；QosPolicy::default()
  - [x] SubTask 4.4: T31 — 外部 writer 写 grid JSON → on_tick 后 model.grid 已更新
  - [x] SubTask 4.5: T32 — take 消费语义：同一批样本两次 on_tick 不重复应用（applied_count 不增加第二次）
  - [x] SubTask 4.6: T33 — 周期到（now_ms=1000, interval=1000）→ 返回 true 且 writer 收到样本
  - [x] SubTask 4.7: T34 — 周期未到（now_ms=500）→ 返回 false，无发布
  - [x] SubTask 4.8: T35 — 两次周期发布 → published_count==2
  - [x] SubTask 4.9: T36 — 多 reader（grid + battery/1）各自接收互不干扰
  - [x] SubTask 4.10: T37 — node 已 shutdown → new 返回 Err(TwinError::Dds(_))
  - [x] SubTask 4.11: T38 — 空 topics 列表 → new 成功 readers 空；on_tick 仅做周期 publish
  - [x] SubTask 4.12: T39 — publish 后 writer 样本 payload 可被 serde_json 解析，含 device_count/applied_count/published_count
  - [x] SubTask 4.13: T40 — apply 2 设备 + 1 grid 后 publish → 摘要 device_count==2 且 grid_timestamp 正确

- [x] Task 5: 创建 `configs/twin_mirror.toml`
  - [x] SubTask 5.1: `[[subscriptions]]` 或数组：topics = ["/power/state/grid", "/power/state/battery/1"]（示例 3 条：grid + battery/1 + market/price）
  - [x] SubTask 5.2: `[mirror]` 段：publish_interval_ms = 1000（蓝图 §6.3 镜像延迟 < 1s）、take_max_samples = 100（蓝图 §4.5）
  - [x] SubTask 5.3: 中文注释：只读旁路（蓝图 §7.3）/ 过期判定规则（D11）/ 逐字段合并保留旧值（§4.4）/ 显式 topic 列表原因（D11 Mock 精确匹配）

- [x] Task 6: 创建 `docs/agents/digital-twin-design.md`
  - [x] SubTask 6.1: 12 章节（版本目标 / 前置依赖 / 交付物清单 / 数据结构 / 接口设计 / 错误处理 / 选型对比 / 实现路径 / 测试计划 / 验收标准 / 风险与坑点 / 偏差声明）
  - [x] SubTask 6.2: Mermaid 图 1：核心算法（订阅 /power/state/* → apply_update 分支 grid/battery/market → TwinModel → 周期 → publish /power/twin/update）
  - [x] SubTask 6.3: Mermaid 图 2：apply_update 决策流程（topic 匹配 → JSON 解析 ? → 过期判定 ? → 逐字段合并 → 计数 → last_update）
  - [x] SubTask 6.4: D1~D12 偏差声明表（从 spec.md 复制）
  - [x] SubTask 6.5: 前置依赖引用 v0.75.0 DDS + v0.77.0 路由 + v0.82.0 GridState + v0.73.0 DeviceState
  - [x] SubTask 6.6: 性能目标（镜像延迟 < 1s，标注"集成阶段验收，本版本交付算法骨架+单元测试"）+ 只读旁路安全（蓝图 §7.3）+ 内存风险（§8.1 设备数增长，BTreeMap 无上限声明）
  - [x] SubTask 6.7: 下游引用 v0.90.0 预测 / v0.91.0 What-if / v0.112.0 云端孪生
  - [x] SubTask 6.8: 选型对比表（旁路订阅 ⭐ / 主动查询 / 数据库快照，蓝图 §5.1）

- [x] Task 7: 版本同步根目录文件
  - [x] SubTask 7.1: 根 `Cargo.toml` `[workspace.package] version = "0.88.0"` → `"0.89.0"`
  - [x] SubTask 7.2: `Makefile` `# Version: v0.89.0` + `VERSION := 0.89.0`
  - [x] SubTask 7.3: `.github/workflows/ci.yml` `# Version: v0.89.0`
  - [x] SubTask 7.4: `ci/src/gate.rs` clippy 段 + test 段注释追加 `+ v0.89.0 数字孪生：TwinMirror / TwinModel / TwinSnapshot / DeviceTwin / MarketMirror / TwinError`

- [x] Task 8: 构建校验（§2.4.2 C6~C11）
  - [x] SubTask 8.1: `cargo metadata --format-version 1` 成功（新 member 解析）
  - [x] SubTask 8.2: `cargo test -p eneros-twin-agent` 40 tests 全过（0 failures）
  - [x] SubTask 8.3: `cargo build -p eneros-twin-agent --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
  - [x] SubTask 8.4: `cargo fmt -p eneros-twin-agent -- --check` 通过
  - [x] SubTask 8.5: `cargo clippy -p eneros-twin-agent --all-targets -- -D warnings` 无 warning
  - [x] SubTask 8.6: `cargo deny check licenses bans sources` 通过（serde/serde_json 既有 SBOM）
  - [x] SubTask 8.7: 回归 — `cargo test -p eneros-grid-agent`（130+1）/ `cargo test -p eneros-device-agent`（24）
  - [x] SubTask 8.8: 回归 — `cargo test -p eneros-energy-market-agent`（185）/ `cargo test -p eneros-agent-bus-dds`（63）

# Task Dependencies

- [Task 2] depends on [Task 1]
- [Task 3] depends on [Task 2]
- [Task 4] depends on [Task 3]
- [Task 5, Task 6] 独立（可与 1~4 并行）
- [Task 7] depends on [Task 1]
- [Task 8] depends on [Task 4, Task 5, Task 6, Task 7]

# 并行执行计划

- **Sub-Agent A**：Task 1 + Task 2 + Task 3 + Task 4（同 crate 源文件，串行单 agent 保证一致性）
- **Sub-Agent B**：Task 5 + Task 6（configs + docs，与 A 并行）
- **Sub-Agent C**：Task 7（版本同步，与 A/B 并行；仅根目录 4 文件，不碰 crate 源码）
- **主 agent**：Task 8（全部完成后统一构建校验 + 回归）
