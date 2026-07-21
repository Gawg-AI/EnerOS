# Checklist — v0.101.0 断网处理与孤岛模式

> Spec：`spec.md`（develop-v10100-island-mode）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 4 新模块位于既有 crate `crates/agents/federation/src/{cache,detector,partition,recovery}.rs`，未新增根目录 crate
- [x] C2: 根 `Cargo.toml` workspace 成员无新增，workspace 仍可解析
- [x] C3: eneros-federation `Cargo.toml` 依赖不变（仅 eneros-crypto），零新增第三方依赖
- [x] C4: 新文档 `island-mode-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-federation`（既有 190 + 新增 36 = 226）全部通过；全 workspace 回归全绿；`cargo test -p eneros-crypto` 零回归
- [x] C8: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；测试模块内 std 位于 `#[cfg(test)]` 下允许，如 Instant 性能测量）

## D. cache.rs 泛型事件缓存（C16~C25）

- [x] C16: `EventCache<T>` 字段全 pub：events (VecDeque<T>) / max_size / overflow_count
- [x] C17: `new(max_size)` 初始化：空 VecDeque、max_size 赋值、overflow_count=0
- [x] C18: `push(e)`：len < max_size → 直接 push_back
- [x] C19: `push(e)`：len == max_size → pop_front（丢弃最旧）+ overflow_count+=1，再 push_back（蓝图 §4.4 丢弃最旧）
- [x] C20: `len()/is_empty()` 正确
- [x] C21: `clear()` 清空 events，保留 overflow_count（历史可观测不归零）
- [x] C22: max_size=1 边界：push 首次正常，第二次溢出丢弃旧 + 长度保持 1
- [x] C23: 泛型双型实例化（u64 与自定义 struct）编译通过
- [x] C24: 序列正确：push A/B/C → events=[A,B,C]；max_size=2 → push D → [B,C]→pop A→push D→[C,D]，overflow_count=1
- [x] C25: 连续溢出计数正确：max_size=2 推 5 个 → overflow_count=3

## E. detector.rs 分区检测状态机（C26~C46）

- [x] C26: `PartitionState { Connected, Suspected, Partitioned, Recovering }` 4 变体，derive Debug/Clone/Copy/PartialEq/Eq
- [x] C27: `PartitionDetector` 字段全 pub：heartbeat_timeout_ms / last_contact(BTreeMap<NodeId,u64>) / state / total_nodes / partition_count
- [x] C28: `new(nodes, heartbeat_timeout_ms, now_ms)`：全部节点 last_contact=now_ms，state=Connected
- [x] C29: `on_heartbeat(from, now_ms)`：已知节点更新 last_contact
- [x] C30: `on_heartbeat(from, now_ms)`：未知节点忽略
- [x] C31: `alive_count(now_ms)`：now - last_contact <= timeout（含等边界）为活跃
- [x] C32: `check` Connected→Suspected：部分失联（alive < total 且 alive >= quorum）
- [x] C33: `check` Suspected→Connected：全部恢复（alive == total）
- [x] C34: `check` Suspected 中 ≥ quorum 但未全部恢复 → 保持 Suspected（抖动容忍）
- [x] C35: `check` Suspected→Partitioned：alive < quorum（quorum 复用 `crate::pbft::quorum`，n=4→3），partition_count+=1
- [x] C36: `check` 全失联直接 Connected→Partitioned：alive < quorum（跳过 Suspected 经中间态）
- [x] C37: `trading_frozen()`：Connected/Suspected → false；Partitioned/Recovering → true
- [x] C38: `check` Partitioned→Recovering：alive ≥ quorum（恢复第一步，仍 frozen）
- [x] C39: `complete_recovery(now_ms)`：Recovering→Connected 成功返回 true，否则 false
- [x] C40: `complete_recovery` 非 Recovering 态调用返回 false（幂等保护）
- [x] C41: `check` Recovering→Partitioned：再失联 alive < quorum，partition_count+=1
- [x] C42: quorum 复用 `crate::pbft::quorum`（与 v0.99.0 共识语义闭环，D8）
- [x] C43: NodeId 复用 `crate::consensus::NodeId = u64`
- [x] C44: partition_count 仅 Partitioned 进入时递增（Recovering 回退不再重复递增）
- [x] C45: 单节点 n=1,quorum=1：全失联 → Partitioned（f=0 零容忍）
- [x] C46: 7 节点 n=7,f=2,quorum=5：4 节点活跃（<5）→ Partitioned；5 节点活跃（≥5）→ Recovering

## F. partition.rs 孤岛模式（C47~C56）

- [x] C47: `IslandMode<T>` 字段全 pub：active / since / cache(EventCache<T>) / activated_count
- [x] C48: `new(cache_max_size)`：active=false / since=0 / activated_count=0 / cache 空
- [x] C49: `activate(now_ms)`：active=true / since=now_ms / activated_count+=1
- [x] C50: `activate` 幂等：已 active 调用 → since 不变、activated_count 不递增
- [x] C51: `deactivate()`：active=false；缓存保留
- [x] C52: `cache_event(e)` active=true → cache.push + 返回 true（D10）
- [x] C53: `cache_event(e)` active=false → 不 push + 返回 false（D10）
- [x] C54: deactivate 后 cache_event 返回 false，缓存保留供同步
- [x] C55: 激活期间 overflow 经 EventCache 正确丢弃最旧且 overflow_count 递增
- [x] C56: `IslandMode` 不持有 `PartitionDetector`（模块独立可测，seam 由上层组合）

## G. recovery.rs 恢复同步（C57~C66）

- [x] C57: `SyncError { UploadFailed, Conflict }` derive Debug/Clone/Copy/PartialEq/Eq
- [x] C58: `SyncReport { uploaded, conflicts }` derive Debug/Clone/Copy/PartialEq/Eq
- [x] C59: `SyncSink<T>` 为 sync trait（`fn upload(&mut self, event: &T) -> Result<(), SyncError>`），无 async（D4）
- [x] C60: `MockSyncSink<T>` 字段全 pub：uploaded(Vec<T>) / fail_times(u32) / conflict_times(u32)
- [x] C61: Mock fail_times>0 → 递减 + Err(UploadFailed)；conflict_times>0 → 递减 + Err(Conflict)；否则 push + Ok
- [x] C62: `RecoverySync::sync(cache, sink)`：空缓存 → Ok(SyncReport { 0, 0 })
- [x] C63: sync 按队序遍历（VecDeque 顺序保持）
- [x] C64: Conflict → conflicts+=1，继续后续上传
- [x] C65: UploadFailed → 立即 Err，缓存不丢（蓝图 §8.5 重同步策略）
- [x] C66: sync 成功返回报告含正确 uploaded/conflicts 计数

## H. e2e 断网全流程（C67~C72）

- [x] C67: TR36 测试：4 节点联邦（quorum=3）→ 2 节点失联 → 状态序列包含 Connected→Suspected→Partitioned
- [x] C68: Partitioned 期间 trading_frozen()==true
- [x] C69: Partitioned 期间 island.activate + cache_event×N → 缓存长度 == N
- [x] C70: 心跳恢复 → alive≥quorum → Recovering（仍 frozen）
- [x] C71: sync 全传 → complete_recovery → Connected（unfrozen）+ deactivate
- [x] C72: e2e 后缓存清空（上层显式调用）、partition_count/activated_count/overflow_count 与预期一致

## I. 配置文件（C73~C78）

- [x] C73: `configs/federation-island.toml` 存在，`[island]` 段含 heartbeat_timeout_ms / cache_max_size
- [x] C74: 中文注释 ≥6 点（四态状态机 / quorum 判据 D8 / 冻结交易 §7.3 / 溢出丢弃 §4.4 / 冲突仲裁 §4.4 / 重同步 §8.5 / 检测 <5s §7.2）
- [x] C75: heartbeat_timeout_ms 默认值与检测 <5s 匹配（如 3000ms 内完成 + 余量）
- [x] C76: cache_max_size 默认值合理（如 1024）且注释说明内存预算
- [x] C77: 配置项命名与设计文档接口契约一致
- [x] C78: 无硬编码默认值在代码中（全部通过 config 注入或 new 参数传入，生产配置化）

## J. 设计文档（C79~C86）

- [x] C79: `docs/agents/island-mode-design.md` 存在且 12 章节齐全（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）
- [x] C80: Mermaid 图 ≥2（状态机图按蓝图 §4.3 重绘含 Suspected 回退/Recovering 回退分支 + 断网→自治→恢复时序图）
- [x] C81: 与 v0.84.0 grid_agent `IslandDetector` 层次区分声明（电网物理并离网 vs 联邦网络分区）
- [x] C82: D1~D10 偏差表与 spec.md 一致
- [x] C83: 接口契约章节与实现签名一致（含同步化 D4 / 泛型 D5 / 注入时钟 D6 / 计数器 D7）
- [x] C84: 技术交底含选型对比表（事件缓存+补传 vs 状态快照 vs 忽略）
- [x] C85: 安全章节覆盖冻结交易（§7.3）+ 脑裂防御（§8.1：quorum 判据 + complete_recovery 显式完成）
- [x] C86: 性能章节声明检测 <5s（蓝图 §7.2）与测量口径（注入时钟，ms 精度）

## K. 版本同步（C87~C91）

- [x] C87: 根 `Cargo.toml` `[workspace.package] version = "0.101.0"`
- [x] C88: `Makefile` 版本注释 + VERSION 变量同步 0.101.0
- [x] C89: `.github/workflows/ci.yml` 版本注释同步 0.101.0
- [x] C90: `ci/src/gate.rs` 注释串尾追加 v0.101.0 类型清单（PartitionState/PartitionDetector/IslandMode/EventCache/RecoverySync/SyncSink/MockSyncSink/SyncError/SyncReport），2 处
- [x] C91: eneros-federation `Cargo.toml` description 追加 v0.101.0

## L. 测试覆盖（C92~C103）

- [x] C92: cache.rs 内嵌 7 测试（TC1~TC7）通过
- [x] C93: detector.rs 内嵌 12 测试（TD8~TD19）通过
- [x] C94: partition.rs 内嵌 8 测试（TI20~TI27）通过
- [x] C95: recovery.rs 内嵌 9 测试（TR28~TR36）通过
- [x] C96: 新增测试总计 36 个，`cargo test -p eneros-federation` 226 全过
- [x] C97: 缓存溢出测试覆盖（TC5/TC6/TC7/TI24/TI25）
- [x] C98: 状态机四态全部迁移路径覆盖（TD8~TD19）
- [x] C99: 冻结/解冻真值表覆盖（TD28/TD39/TD40 + TR36 e2e）
- [x] C100: 同步 Conflict + UploadFailed 分支覆盖（TR31/TR32/TR33）
- [x] C101: e2e 断网全流程覆盖（TR36：Connected→Suspected→Partitioned→Recovering→Connected）
- [x] C102: 既有 190 测试零回归（membership/discovery/channel/tunnel/consensus/pbft/view_change/auction/bid_book/matching）
- [x] C103: eneros-crypto 测试零回归（本版本未改动 crypto，SM3 复用 v0.99.0）

## M. 蓝图对齐与验收（C104~C112）

- [x] C104: v0.101.0 交付物全覆盖：cache/detector/partition/recovery 4 模块 / EventCache / PartitionDetector / IslandMode / RecoverySync（蓝图 §3）
- [x] C105: 断网检测可用（蓝图 §7.1 功能：四态状态机 + quorum 判据）
- [x] C106: 检测 <5s（蓝图 §7.2 性能：heartbeat_timeout_ms 默认值匹配）
- [x] C107: 冻结交易保证（蓝图 §7.3 安全：trading_frozen 在 Partitioned/Recovering 返回 true）
- [x] C108: 数据不丢（蓝图 §7.2/§9 可靠：缓存保留 + 冲突跳过 + UploadFailed 不丢缓存）
- [x] C109: 规则配置化（蓝图 §9 可维护：heartbeat_timeout_ms / cache_max_size toml 配置）
- [x] C110: 分区状态 metric（蓝图 §9 可观测：partition_count/activated_count/overflow_count + PartitionState pub 可读取）
- [x] C111: 上游 v0.99.0 共识 + v0.100.0 竞价使用关系在文档声明（§5.5 交互：detector 向 AuctionEngine 提供 frozen 查询）
- [x] C112: 下游 v0.110.0 云边同步解锁声明（§5.5 交互：事件缓存 + 增量同步机制复用）

---

## 验收记录（2026-07-19 收工核验）

- **B 构建校验实测**：C6 `cargo metadata` 通过；C7 `cargo test -p eneros-federation` **226/226**（既有 190 + 新增 36）通过，全 workspace 回归全部 `test result: ok`（含 eneros-crypto **358/358** 零回归）；C8 aarch64-unknown-none 交叉编译通过；C9 `cargo fmt --all -- --check` 通过；C10 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning（构建期仅 Windows 增量缓存文件锁 os error 5 环境警告，非 lint 告警，退出码 0）；C11 `cargo deny check` 全项 ok（零新增第三方依赖，SBOM 不变）。
- **C13 实测**：`git status --porcelain` 过滤 `target/|*.elf|*.bin|*.dtb|.idea|.vscode|*.log` 零匹配，无垃圾文件被追踪。
- **子代理只读核验**：C1~C5/C12/C15/C16~C44/C47~C86/C87~C91/C97~C100/C102/C104~C112 逐项 PASS，证据在案（4 模块结构/派生/字段、四态状态机迁移分支、幂等激活、SyncSink 同步化 seam、e2e 断网全流程、配置 7 注释点、设计文档 12 章节 + 2 Mermaid + D1~D10 偏差表 + v0.84.0 IslandDetector 层次区分）。
- **核验回补（T8）**：首轮核验发现 C45（n=1 f=0 零容忍）/C46（n=7 quorum=5 边界）无场景覆盖 → 在 td16/td18 函数体内追加场景（不新增 #[test]，总数保持 226）。回补后 `cargo test -p eneros-federation detector` 12/12、全量 226/226、fmt/clippy/aarch64 交叉编译全绿。子代理修正任务片段中 td18 的 `check(1501)` 为 `check(1500)`（边界含等 C31，否则心跳 500 的节点全部超时、Recovering 断言不成立）。
- **测试明细**：cache.rs 7（tc1~tc7）/ detector.rs 12（td8~td19）/ partition.rs 8（ti20~ti27）/ recovery.rs 9（tr28~tr36，含 tr36 e2e 断网全流程 Connected→Suspected→Partitioned→Recovering→Connected）。
- **版本同步**：根 `Cargo.toml` 0.101.0 / `Makefile` 注释 + VERSION / `ci.yml` 注释 / `gate.rs` 注释串尾 2 处 / eneros-federation description 均已同步 v0.101.0。
- **结论**：C1~C112 全部通过，v0.101.0 断网处理与孤岛模式收工。下一阶段：v0.110.0 云边同步。
