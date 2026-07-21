# Checklist

## Task 1: crate 骨架
- [x] C1: `crates/agents/coordinator/Cargo.toml` 存在，包名 `eneros-coordinator`
- [x] C2: dependencies 仅 `eneros-agent`（workspace path `../agent`），无其他第三方依赖
- [x] C3: 根 `Cargo.toml` members 含 `"crates/agents/coordinator"`，其余 member 不变
- [x] C4: `lib.rs` 含 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- [x] C5: `lib.rs` 含 `pub mod arbiter;` + `pub mod bid;` + `pub mod conflict;` + 10 项重导出（arbiter 5 + bid 3 + conflict 2，公共 API 全覆盖）
- [x] C6: `lib.rs` 中文 crate 文档含 v0.92.0 与 D1~D12 偏差简表
- [x] C7: `cargo metadata --format-version 1` 成功

## Task 2: bid.rs 数据模型
- [x] C8: `Priority` 5 变体升序声明（Low<Normal<High<Critical<Safety）+ derive Ord + Default=Normal（D7）
- [x] C9: `Claim` 5 字段（agent_id: AgentId / priority / bid: f32 / safety_critical / deadline: u64）+ derive Debug, Clone, Copy, PartialEq（D3）
- [x] C10: `cmp_bid` 全序：NaN 恒最低、双 NaN Equal、±Inf 保留偏序（D11）
- [x] C11: `Claim::is_urgent`：`deadline < now_ms.saturating_add(window_ms)`（D12，严格 <，过去必 urgent）
- [x] C12: bid.rs 无 std/panic!/unsafe/todo!/unimplemented!/unwrap（主代码）+ 中文模块文档含偏差引用
- [x] C13: T1~T2（Priority 序+Default / Claim 构造+Copy+字段回显）存在且通过
- [x] C14: T3~T5（cmp_bid 正常/NaN 最低/双 NaN+±Inf）存在且通过
- [x] C15: T6~T7（urgent 过去+窗口内 / 非 urgent+严格边界）存在且通过
- [x] C16: T8（u64::MAX saturating 不 panic）存在且通过

## Task 3: arbiter.rs 三级仲裁
- [x] C17: `ArbitrationRequest` 3 字段（resource_id: &'static str / claimants: Vec\<Claim\> / deadline: u64）+ Debug, Clone（D2/D3）
- [x] C18: `ArbitrationReason` 4 变体（SafetyFirst/HighestBid/Deadline/Default）+ Debug, Clone, Copy, PartialEq, Eq
- [x] C19: `ArbitrationResult` 4 字段（winner: Option\<AgentId\> / reason / timestamp / conflict）+ Debug, Clone, Copy, PartialEq（D8/D10）
- [x] C20: `ArbiterPolicy { urgent_window_ms: u64 }` + Default==1000（D12）
- [x] C21: `DomainArbiter` 7 字段全 pub（policy + 6 计数器）+ `new` 计数器全零（D9）
- [x] C22: `arbitrate(&mut self, req, now_ms)` 三级顺序：safety > deadline urgent > 最高 bid（不可逾越）
- [x] C23: 空 claimants → winner None + reason Default + empty_count+=1 + 不 panic（D8）
- [x] C24: safety 分支：max priority 胜出；≥2 safety → conflict=true + conflict_count+=1；同优先级确定性首个（D10）
- [x] C25: urgent 分支：最早 deadline 胜出 + deadline_count+=1
- [x] C26: bid 分支：cmp_bid max 胜出（NaN 不胜出，全 NaN 首个）+ bid_count+=1（D11）
- [x] C27: timestamp == now_ms 回显；每次仲裁 total_count+=1 且分类计数和==total
- [x] C28: arbiter.rs 无 std/panic!/unsafe/unwrap（主代码）+ 中文模块文档含偏差引用
- [x] C29: T9~T12（policy 默认/计数器零/空请求/安全压制高 bid+过期 deadline）存在且通过
- [x] C30: T13~T15（多安全优先级/同优先级 conflict/计数器）存在且通过
- [x] C31: T16~T19（urgent 胜出/最早 deadline/safety 压 urgent/超窗落 bid）存在且通过
- [x] C32: T20~T23（最高 bid/等 bid 首个/NaN 不胜出/全 NaN 首个）存在且通过
- [x] C33: T24~T27（timestamp 回显/计数器组合/单 claimant/自定义 window）存在且通过
- [x] C34: T28~T29（确定性双实例一致/Result 派生语义）存在且通过
- [x] C35: T30（100 claimants 末位 safety 仍胜出 + 无 panic）存在且通过

## Task 4: conflict.rs 冲突/死锁检测
- [x] C36: `has_safety_conflict`：safety_critical ≥ 2 → true（D10）
- [x] C37: `detect_deadlock`：wait-for 图 BTreeMap 邻接 + 三色 DFS（灰→环）+ 全节点扫描覆盖不连通分量
- [x] C38: conflict.rs 无 std/panic!/unsafe/unwrap（主代码）+ 中文模块文档
- [x] C39: T31~T33（0/1/2 个 safety → false/false/true）存在且通过
- [x] C40: T34~T36（空边/链无环/自环）存在且通过
- [x] C41: T37~T38（2-cycle/3-cycle → true）存在且通过
- [x] C42: T39~T40（菱形不误报/不连通分量带环 → true）存在且通过

## Task 5: configs/edge_arbiter.toml
- [x] C43: 文件位于 `configs/edge_arbiter.toml`
- [x] C44: `[arbiter]` 段 urgent_window_ms=1000（与 ArbiterPolicy::default 一致）
- [x] C45: 中文注释含三级顺序 / safety 永远胜出（§7.3）/ conflict 告警（D10）/ <10ms 集成阶段 / NaN 防御（D11）/ 结果广播坑点（§8.5）/ 老化机制（§8.1）

## Task 6: docs/agents/edge-arbiter-design.md
- [x] C46: 文件位于 `docs/agents/edge-arbiter-design.md`（非 docs/phase2，D5）
- [x] C47: 12 章节完整
- [x] C48: Mermaid 图 1（三级仲裁决策流：空 → safety → urgent → bid）
- [x] C49: Mermaid 图 2（死锁检测：建图 → 三色 DFS → 遇灰成环）
- [x] C50: D1~D12 偏差声明表完整
- [x] C51: 前置依赖引用 v0.88.0 + v0.77.0 + v0.33.0（AgentId）；下游引用 v0.93.0 / v0.94.0
- [x] C52: 选型对比表（安全优先+竞价 ⭐ / 纯竞价 / 轮询，蓝图 §5.1）
- [x] C53: 性能目标（仲裁 <10ms，标注集成阶段验收）+ GPU 规则（§6.6 不涉及 GPU）
- [x] C54: 安全语义（§7.3 safety 永远胜出）+ 可观测（D9 六计数器，§9）
- [x] C55: 风险（§8.1 饥饿老化 / §8.5 结果广播 / §5.4 仅检测不预防）

## Task 7: 版本同步
- [x] C56: 根 `Cargo.toml` version = "0.92.0"
- [x] C57: 根 `Cargo.toml` members 既有项不变（仅追加 coordinator）
- [x] C58: `Makefile` `# Version: v0.92.0` + `VERSION := 0.92.0`
- [x] C59: `.github/workflows/ci.yml` `# Version: v0.92.0`
- [x] C60: `ci/src/gate.rs` clippy 段 + test 段注释追加 v0.92.0 类型列表

## Task 8: 构建校验（§2.4.2）
- [x] C61: `cargo metadata` 成功
- [x] C62: `cargo test -p eneros-coordinator` 40 tests 全过
- [x] C63: aarch64-unknown-none 交叉编译通过
- [x] C64: `cargo fmt --check` 通过
- [x] C65: `cargo clippy --all-targets -- -D warnings` 无 warning
- [x] C66: `cargo deny check licenses bans sources` 通过
- [x] C67: 回归 eneros-agent / twin-agent / energy-market-agent / agent-bus-dds 全过

## 总体校验
- [x] C68: 新 crate 位于 `crates/agents/coordinator/`（§2.3.1，无根目录 crate，D1）
- [x] C69: crate 目录名与包名去 `eneros-` 前缀一致（coordinator）
- [x] C70: 无 `docs/` 根目录平面化文档（docs/agents/ 下）
- [x] C71: 配置文件在 `configs/` 下（非 config/）
- [x] C72: `.gitignore` 无需更新（无新文件类型）
- [x] C73: `git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C74: ADR 决策未违反（未引入研究特性、复用既有 AgentId、无重复造轮子）
- [x] C75: no_std 合规：lib.rs crate 级属性 + 子模块不重复 + 无 std/async
- [x] C76: SBOM 无新第三方依赖（仅 workspace 内 eneros-agent）
- [x] C77: Surgical Changes：既有 crate 源码零改动（仅根目录 4 文件版本同步 + members 追加）
- [x] C78: 无效输入无 panic（空 claimants/NaN bid/全 NaN/u64::MAX now/空边表/自环 全走安全路径）+ 确定性无随机源
