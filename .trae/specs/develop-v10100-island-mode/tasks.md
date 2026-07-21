# Tasks — v0.101.0 断网处理与孤岛模式

> Spec：`spec.md`（develop-v10100-island-mode）。T1/T2 无依赖可并行，T3 依赖 T1，T4 依赖 T1+T2+T3，T5~T7 顺序收尾。

- [x] **T1：cache.rs — EventCache\<T\> 泛型事件缓存**
  - [ ] 1.1 `pub struct EventCache<T> { pub events: VecDeque<T>, pub max_size: usize, pub overflow_count: u64 }`（Debug/Clone）
  - [ ] 1.2 `new(max_size)`；`push`：len == max_size → pop_front + overflow_count+=1，再 push_back（蓝图 §4.4 丢弃最旧）
  - [ ] 1.3 `len()/is_empty()/clear()`；clear 保留 overflow_count（历史可观测不归零）
  - [ ] 1.4 测试 TC1~TC7：new 初始 / push 顺序 / 溢出丢弃最旧且计数 / max_size=1 边界 / clear 保留 overflow_count / len/is_empty / 泛型双型实例化（u64 + 自定义 struct）
  - 验证：`cargo test -p eneros-federation cache` 全过

- [x] **T2：detector.rs — PartitionDetector 四态状态机**
  - [ ] 2.1 `pub enum PartitionState { Connected, Suspected, Partitioned, Recovering }`（Debug/Clone/Copy/PartialEq/Eq）
  - [ ] 2.2 `PartitionDetector` 字段全 pub：heartbeat_timeout_ms / last_contact: BTreeMap\<NodeId, u64\> / state / total_nodes / partition_count（NodeId 复用 `crate::consensus::NodeId`）
  - [ ] 2.3 `new(nodes, heartbeat_timeout_ms, now_ms)`：全部节点 last_contact=now_ms，state=Connected
  - [ ] 2.4 `on_heartbeat(from, now_ms)`：未知节点忽略；`alive_count(now_ms)`：`now - last <= timeout` 计活跃（边界含等）
  - [ ] 2.5 `check(now_ms)` 状态机：Connected(alive<total)→Suspected；Suspected(alive==total)→Connected / (alive<quorum)→Partitioned+partition_count+=1；Partitioned(alive≥quorum)→Recovering；Recovering(alive<quorum)→Partitioned+partition_count+=1（quorum 复用 `crate::pbft::quorum`，D8）
  - [ ] 2.6 `trading_frozen()` = state ∈ {Partitioned, Recovering}（D9）；`complete_recovery(now_ms)`：仅 Recovering → Connected 返回 true，其余 false
  - [ ] 2.7 测试 TD8~TD19：12 个（见 spec 测试规划表）
  - 验证：`cargo test -p eneros-federation detector` 全过

- [x] **T3：partition.rs — IslandMode\<T\> 孤岛模式**
  - [ ] 3.1 `pub struct IslandMode<T> { pub active, pub since, pub cache: EventCache<T>, pub activated_count }`（Debug/Clone）
  - [ ] 3.2 `new(cache_max_size)`；`activate(now_ms)`：幂等——已 active 不重置 since、不重复计数；`deactivate()`：缓存保留
  - [ ] 3.3 `cache_event(e) -> bool`：!active → false；active → cache.push + true（D10）
  - [ ] 3.4 测试 TI20~TI27：8 个（见 spec 测试规划表）
  - 验证：`cargo test -p eneros-federation partition` 全过

- [x] **T4：recovery.rs — RecoverySync 恢复同步 + e2e**
  - [ ] 4.1 `SyncError { UploadFailed, Conflict }`（Debug/Clone/Copy/PartialEq/Eq）；`SyncReport { uploaded, conflicts }`（同派生）
  - [ ] 4.2 `SyncSink<T>` trait：`fn upload(&mut self, event: &T) -> Result<(), SyncError>`（sync trait，无 async，D4）
  - [ ] 4.3 `MockSyncSink<T> { pub uploaded: Vec<T>, pub fail_times: u32, pub conflict_times: u32 }`：fail_times>0 → 递减 + Err(UploadFailed)；conflict_times>0 → 递减 + Err(Conflict)；否则 push uploaded + Ok
  - [ ] 4.4 `RecoverySync::sync(cache, sink)`：按队序遍历，`Conflict → conflicts+=1 继续`，`UploadFailed → 立即 Err`（缓存保留待重试，蓝图 §8.5），全过 → `Ok(SyncReport)`
  - [ ] 4.5 测试 TR28~TR35 单测 8 个 + TR36 **e2e 断网全流程**：4 节点 detector（quorum=3）→ 2 节点失联 → Suspected→Partitioned（trading_frozen==true + island.activate + cache_event×N）→ 心跳恢复 → Recovering（仍冻结）→ sync 全传 → complete_recovery → Connected（解冻 + deactivate + 缓存清空）→ 断言状态序列/计数全对
  - 验证：`cargo test -p eneros-federation recovery` 全过

- [x] **T5：模块接线 + 配置 + 设计文档**
  - [ ] 5.1 `lib.rs`：`pub mod cache; pub mod detector; pub mod partition; pub mod recovery;`（按字母序插入）+ 全类型重导出 + crate 文档追加 v0.101.0 段与 D1~D10 偏差表（既有 10 模块零改动）
  - [ ] 5.2 `Cargo.toml` description 追加 "v0.101.0 断网处理与孤岛模式"（依赖不变）
  - [ ] 5.3 `configs/federation-island.toml`：`[island]` heartbeat_timeout_ms / cache_max_size + 中文注释 ≥6 点（四态状态机 §4.3 / quorum 判据 D8 / 冻结交易 §7.3 / 溢出丢弃 §4.4 / 冲突仲裁 §4.4 / 重同步 §8.5 / 检测 <5s §7.2）
  - [ ] 5.4 `docs/agents/island-mode-design.md`：12 章节 + ≥2 Mermaid（状态机图按蓝图 §4.3 重绘含 Suspected 回退/Recovering 回退分支 + 断网→自治→恢复时序图）+ D1~D10 偏差表 + 与 v0.84.0 grid_agent `IslandDetector`（电网物理并离网，PCC 层）的层次区分声明
  - 验证：`cargo test -p eneros-federation` 全过（既有 190 + 新增 36 = 226）

- [x] **T6：版本同步 0.101.0 + 全量构建验证**
  - [ ] 6.1 根 `Cargo.toml` version = "0.101.0"；`Makefile` VERSION；`ci.yml` 注释；`gate.rs` 注释串尾追加 v0.101.0 类型清单（2 处 replace_all：PartitionState/PartitionDetector/IslandMode/EventCache/RecoverySync/SyncSink/MockSyncSink/SyncError/SyncReport）
  - [ ] 6.2 §2.4.2 构建校验：C6 metadata / C7 federation 226 + 全 workspace 回归 / C8 aarch64 交叉编译 / C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T7：checklist 逐项核验收工**
  - [ ] 7.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工

- [x] **T8（核验回补）：C45/C46 边界场景补测试**
  - [ ] 8.1 td16 内追加 n=1 场景（quorum=1，唯一节点失联 → Partitioned，partition_count=1）
  - [ ] 8.2 td18 内追加 n=7 场景（quorum=5，4 活跃 → Partitioned；5 活跃 → Recovering）
  - 验证：`cargo test -p eneros-federation detector` 12 测试全过（不新增测试函数，场景内嵌，总数保持 226）
  - 验证：`cargo fmt --all -- --check` + `cargo clippy -p eneros-federation --all-targets -- -D warnings` 通过

# Task Dependencies

- T1、T2 独立（可并行）
- T3 depends on T1（IslandMode 持有 EventCache）
- T4 depends on T1 + T2 + T3（sync 消费缓存；e2e 组合全部）
- T5 depends on T4
- T6 depends on T5
- T7 depends on T6
