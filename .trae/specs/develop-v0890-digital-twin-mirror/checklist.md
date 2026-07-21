# Checklist

## Task 1: crate 骨架
- [x] C1: `crates/agents/twin-agent/Cargo.toml` 存在，包名 `eneros-twin-agent`
- [x] C2: dependencies 为 3 个 workspace path 依赖（agent-bus-dds / grid-agent / device-agent）+ serde + serde_json（与 energy-market-agent 同 version/features）
- [x] C3: 根 `Cargo.toml` members 含 `"crates/agents/twin-agent"`，其余 member 不变
- [x] C4: `lib.rs` 含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C5: `lib.rs` 含 `pub mod model;` + `pub mod mirror;` + 6 项重导出（DeviceTwin/MarketMirror/TwinModel/TwinSnapshot/TwinError/TwinMirror）
- [x] C6: `lib.rs` 中文 crate 文档含 v0.89.0 与 D1~D12 偏差简表
- [x] C7: `cargo metadata --format-version 1` 成功

## Task 2: model.rs 数据结构
- [x] C8: `MarketMirror` 2 字段 + 派生 `Debug, Clone, Copy, PartialEq, Default`
- [x] C9: `DeviceTwin` 2 字段（device_id: u64 / state: eneros_device_agent::DeviceState）+ 派生 `Debug, Clone, PartialEq, Default`
- [x] C10: `TwinModel` 4 字段（devices: BTreeMap / grid: eneros_grid_agent::GridState / market: Option<MarketMirror> / last_update: u64）+ 派生 `Debug, Clone, Default`
- [x] C11: `TwinModel::device_count() -> usize`
- [x] C12: `TwinSnapshot` 2 字段 + 派生 `Debug, Clone`
- [x] C13: `TwinSnapshot::summary_json() -> String`（含 timestamp/last_update/device_count/grid_timestamp/market_timestamp）
- [x] C14: model.rs 无 std/HashMap/panic!/unsafe/todo!/unimplemented!
- [x] C15: T1 — MarketMirror default 全零 + Copy
- [x] C16: T2 — DeviceTwin default（device_id==0 / state 默认）
- [x] C17: T3 — TwinModel default（空/None/0/count==0）
- [x] C18: T4 — BTreeMap 乱序插入 keys 有序 [10,20,30]
- [x] C19: T5 — TwinSnapshot 构造 + Clone
- [x] C20: T6 — summary_json 可被 serde_json::Value 解析且含 device_count

## Task 3: mirror.rs TwinError/new/apply_update
- [x] C21: `TwinError` 单变体 `Dds(DdsError)`，派生 Debug
- [x] C22: `GridPayload` 全 Option<f32> 11 数值字段 + Option<u64> timestamp（serde Deserialize）
- [x] C23: `DevicePayload` 全 Option<f64> 5 数值字段 + Option<bool> online + Option<u64> last_update_ms
- [x] C24: `MarketPayload { timestamp: u64, current_price: f32 }`（必填）
- [x] C25: `TwinMirror` 10 字段全 pub（model/node/participant/readers/writer/publish_interval_ms/last_publish_ms/applied_count/skipped_count/published_count）
- [x] C26: `new` 创建 participant + N readers + 1 writer（"/power/twin/update"）；DDS 失败 → Err(Dds)
- [x] C27: `new` 空 topics 合法（readers 空）
- [x] C28: apply_update grid：无效 JSON → false + skipped_count+=1 + model 不变
- [x] C29: apply_update grid：过期 timestamp → false + 跳过
- [x] C30: apply_update grid：同 timestamp 接受（非严格 <）
- [x] C31: apply_update grid：逐字段合并（Some 覆盖 / None 保留）
- [x] C32: apply_update battery：id 解析失败 → false + skipped
- [x] C33: apply_update battery：新设备 or_default 创建
- [x] C34: apply_update battery：过期 last_update_ms → false + 跳过
- [x] C35: apply_update battery：部分合并保留旧值；last_update_ms = now_ms
- [x] C36: apply_update market：缺字段/无效 JSON → false + 保留旧值
- [x] C37: apply_update market：过期 timestamp → false + 跳过
- [x] C38: apply_update：未知 topic → false + skipped
- [x] C39: apply_update 成功 → applied_count+=1 + model.last_update = now_ms
- [x] C40: snapshot() timestamp == model.last_update
- [x] C41: mirror.rs 主代码无 unwrap/std/async/panic!/unsafe
- [x] C42: T7~T12 grid 6 测试存在且断言符合 spec
- [x] C43: T13~T19 battery 7 测试存在且断言符合 spec
- [x] C44: T20~T23 market 4 测试存在且断言符合 spec
- [x] C45: T24~T26 通用 3 测试存在且断言符合 spec
- [x] C46: T27~T30 snapshot 4 测试存在且断言符合 spec

## Task 4: on_tick/publish + 端到端
- [x] C47: `on_tick` 逐 reader take(100) 并 apply（无借用冲突）
- [x] C48: `on_tick` 周期判定 `now_ms - last_publish_ms >= publish_interval_ms` → publish + 更新 last_publish_ms → Ok(true)
- [x] C49: `on_tick` 周期未到 → Ok(false) 不发布
- [x] C50: `publish` 摘要 JSON 含计数器（applied/skipped/published）→ write → published_count+=1
- [x] C51: `publish` write 失败 → Err(TwinError::Dds)
- [x] C52: 测试辅助 MockDdsNode + 外部 writer 可用
- [x] C53: T31 — 外部写 grid → on_tick 后 model 更新
- [x] C54: T32 — take 消费不重复应用
- [x] C55: T33 — 周期到返回 true 且收到样本
- [x] C56: T34 — 周期未到返回 false
- [x] C57: T35 — 两次发布 published_count==2
- [x] C58: T36 — 多 reader 各自接收
- [x] C59: T37 — shutdown 节点 new → Err(Dds)
- [x] C60: T38 — 空 topics 合法
- [x] C61: T39 — publish payload 可解析含计数器
- [x] C62: T40 — 摘要 device_count==2 + grid_timestamp 正确

## Task 5: configs/twin_mirror.toml
- [x] C63: 文件位于 `configs/twin_mirror.toml`
- [x] C64: 订阅 topic 列表 ≥ 3 条（grid / battery/1 / market/price）
- [x] C65: `[mirror]` 段 publish_interval_ms=1000 + take_max_samples=100
- [x] C66: 中文注释含只读旁路 / 过期判定（D11）/ 字段合并（§4.4）/ 显式 topic 原因

## Task 6: docs/agents/digital-twin-design.md
- [x] C67: 文件位于 `docs/agents/digital-twin-design.md`（非 docs/phase2，D12）
- [x] C68: 12 章节完整
- [x] C69: Mermaid 图 1（核心算法：订阅→分支→模型→周期→发布）
- [x] C70: Mermaid 图 2（apply_update 决策流程）
- [x] C71: D1~D12 偏差声明表完整
- [x] C72: 前置依赖引用 v0.75.0 + v0.77.0 + v0.82.0 + v0.73.0
- [x] C73: 性能目标（镜像延迟 < 1s，标注集成阶段验收）
- [x] C74: 只读旁路安全（蓝图 §7.3）
- [x] C75: 内存风险声明（§8.1 设备数增长）
- [x] C76: 下游引用 v0.90.0 / v0.91.0 / v0.112.0
- [x] C77: 选型对比表（旁路订阅 ⭐ / 主动查询 / 数据库快照）

## Task 7: 版本同步
- [x] C78: 根 `Cargo.toml` version = "0.89.0"
- [x] C79: 根 `Cargo.toml` members 既有项不变（仅追加 twin-agent）
- [x] C80: `Makefile` `# Version: v0.89.0` + `VERSION := 0.89.0`
- [x] C81: `.github/workflows/ci.yml` `# Version: v0.89.0`
- [x] C82: `ci/src/gate.rs` clippy 段注释追加 v0.89.0 类型列表
- [x] C83: `ci/src/gate.rs` test 段注释追加 v0.89.0 类型列表

## Task 8: 构建校验（§2.4.2）
- [x] C84: `cargo metadata` 成功
- [x] C85: `cargo test -p eneros-twin-agent` 40 tests 全过
- [x] C86: aarch64-unknown-none 交叉编译通过
- [x] C87: `cargo fmt --check` 通过
- [x] C88: `cargo clippy --all-targets -- -D warnings` 无 warning
- [x] C89: `cargo deny check licenses bans sources` 通过
- [x] C90: 回归 grid-agent 130+1 / device-agent 24
- [x] C91: 回归 energy-market-agent 185 / agent-bus-dds 63

## 总体校验
- [x] C92: 新 crate 位于 `crates/agents/twin-agent/`（§2.3.1，无根目录 crate）
- [x] C93: crate 目录名与包名去 `eneros-` 前缀一致（twin-agent，D1）
- [x] C94: 无 `docs/` 根目录平面化文档（docs/agents/ 下）
- [x] C95: 无 `config/` 目录（configs/ 下）
- [x] C96: `.gitignore` 无需更新（无新文件类型）
- [x] C97: `git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C98: ADR 决策未违反（未引入研究特性、未自研已有开源替代、复用既有 DdsNode/GridState/DeviceState）
- [x] C99: no_std 合规：lib.rs crate 级属性 + 子模块不重复 + 无 std/async
- [x] C100: 内存预算声明：TwinModel 随设备数线性增长（文档 §8.1 风险声明，MVP 无上限）
- [x] C101: SBOM 无新第三方依赖（serde/serde_json 既有）
- [x] C102: Surgical Changes：既有 crate 源码零改动（仅根目录 4 文件版本同步 + members）
- [x] C103: 命名不冲突（twin_agent/twin-agent 前缀全新）
- [x] C104: 时间注入合规：now_ms 参数，无 Instant::now()
- [x] C105: apply_update 无效输入无 panic（解析失败走 false+skipped 路径）
