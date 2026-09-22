# EnerOS 架构深化蓝图 v1（Deepening Blueprint）

> **版本**：v1 · **基线 commit**：`60b73a8`（2026-09-21）
> **来源**：架构评审报告 `/tmp/architecture-review-20260921-225312.html` 的 7 个候选，**全部经本蓝图作者逐条代码复验**（复验订正处见各节"复验事实"）。
> **读者**：执行本蓝图的 AI 开发代理。每步给出**验证命令**；每候选给出**次生 bug 风险登记**。严格遵循仓库规则 `.trae/rules/记忆.md`（no_std、Conventional Commits、§2.4 校验清单）与 Karpathy 四原则（先想后写 / 最小实现 / 外科手术式改动 / 目标驱动执行）。
> **词汇纪律**：module / interface / implementation / depth / seam / adapter / leverage / locality（见 codebase-design 词汇表）；领域词汇以 `CONTEXT.md` 为唯一事实源。

> **执行状态（2026-09-22，按当前工作树与 Git 历史复核）**：§A/B/C/D/E 的原始设计和步骤保留为历史提案，**不能脱离 §11 的实施期订正单独执行**。A（含 F）、B、D 已落地；C、E 前提证伪，不实施；G 搁置。现行代码事实以 `CONTEXT.md`、对应源码和提交为准；§11.8 是 2026-09-21 的历史门记录，不代表此后所有门持续通过。未完成或失败的门不得称为全绿。

---

## 0. 铁律（任何候选实施时不得违反）

| # | 铁律 | 理由 |
|---|------|------|
| G0 | **串口/审计证据词形逐字节冻结** | AUDIT-CHAIN、ACQ 行、`[svc-control-dispatch]` 等词形是 45–47/61/70 冻结证据链的判定依据；重构改变任何输出词形 = 证据链断裂（本项目历史最大次生 bug 来源） |
| G1 | **不动已冻结 seam**：ADR-0052（SagaExecutor seam）、ADR-0054、ADR-0026/0028（协议/驱动唯一 seam）、ADR-0025（IPC wire ABI） | 已签署决策态 |
| G2 | **no_std**：control-dispatch、root-task 全部目标代码 `alloc::*`/`core::*`，禁止 `std::*`（xtask 为主机工具，例外） | 蓝图 §43.1 |
| G3 | **外科手术式改动**：不改与候选无关的代码/注释/格式；不顺手"改进"邻近代码 | Karpathy 原则 3 |
| G4 | **先红后绿**：凡涉及行为不变量的收拢（如淘汰逻辑），先写锁定测试（对旧实现红或对新 interface 红），再迁移 | 仓库"先红后绿"惯例 |
| G5 | **每候选独立 commit 序列、独立可回退**；禁止多候选混在一个 commit | 回退粒度 |
| G6 | 每候选落地后更新 `CONTEXT.md`（新 module 名词登记）——**代码落地才登记，蓝图阶段不预登记** | CONTEXT 是事实源，不是愿望清单 |
| G7 | QEMU 证据依赖的候选（A/C/D/E）完成后，按对应版本门复跑（见 §9 全局验收门） | 证据链连续性 |

---

## 1. 总览与执行顺序

| 序 | 候选 | 文件 | 推荐度 | 风险 | 依赖 |
|---|------|------|--------|------|------|
| **A**（报告#1） | HoldRegistry：快照钉住状态深 module | control-dispatch | Strong | 低 | 无 |
| **B**（报告#2） | xtask：FlagParser + SignedReport | xtask | Strong | 低 | 无（可与 A 并行） |
| **C**（报告#3） | run_ipc_loop 效应器 seam 去 cfg 孪生 | root-task/service.rs | Worth exploring | **高** | 建议 A 之后 |
| **D**（报告#4） | ServiceSpawnSession 构造器收口 | root-task/spawn.rs | Worth exploring | 中 | 无 |
| **E**（报告#5） | HandoverSet 设备移交面统一 | root-task/spawn.rs | Worth exploring | 中 | 建议与 D 同分支先后做（同文件） |
| **F**（报告#6） | EngineObservation 快照 | control-dispatch | Speculative | 低 | **并入 A**，不单列 |
| **G**（报告#7） | engine.rs 文件级拆分 | control-dispatch | Speculative | — | **搁置**：A 落地后仍有导航摩擦再议 |

原提案执行顺序理由（**已被 §11.1、§11.4 的实证订正，不再是待执行指令**）：A/B 风险最低且正中热点先行；原拟 C 放最后，D/E 同文件且 E 先 D 后。实际终态为 A/B/D 已实施、F 并入 A，C/E 不实施、G 搁置。

---

## A. HoldRegistry：快照钉住生命周期深 module（含 F）

### A.1 复验事实（已对代码核实，订正报告）

- 状态字段 5 个，声明于 `crates/protocols/control-dispatch/src/engine.rs:349-368`：`held_plans`、`held_store_unavailable`、`input_retainer`（这三者属装配面）、`released_holds: Vec<([u8;16],u32)>`(358)、`hold_epochs: BTreeMap<[u8;16],u32>`(361)、`pending_releases: Vec<[u8;16]>`(364)、`held_done_logged: Vec<PlanId>`(366)、`pub held_pin_failures: u64`(368)。初始化于 1227-1231。
- **淘汰逻辑实际 6 处、4 种惯用法**（报告称 3 处，已订正）：`held_engine.rs:234`（`while hold_epochs.len() > 16`）、`509-510`（`held_done_logged` + `remove(0)`）、`543-544`（`released_holds` + `remove(0)`）、`550`（`pending_releases.len() < 16` 反向判断）、`574-575`（`released_holds` 又一次）、`579`（`still_pending.truncate(16)`）。
- **`held_pin_failures` 是写多者零读者的 pub 字段**：写于 `held_engine.rs:84` 和 `:200`，全仓无任何读取点。pub 可变字段违反同文件观测面纪律（1273 注释自述"字段已私有"原则）。
- 代次安全不变量 F70-04 仅以注释维系（engine.rs:354-357）。
- F（报告#6）复验：观测面 getter 恰 9 个（`engine.rs:1279-1324`），且 1273-1276 注释表明是有意的"观测/可写分离"面——**不激进收缩，只收钉住相关观测**。

### A.2 深化设计

新 module `crates/protocols/control-dispatch/src/hold_registry.rs`：

```rust
// no_std；仅示意签名，实现时以复验事实为准
pub(crate) struct HoldRegistry { /* 5 字段全部私有 */ }
impl HoldRegistry {
    pub fn new() -> Self;
    /// 准入持有：代次 +1；有界淘汰内藏。返回新代次。
    pub fn hold(&mut self, snapshot_id: [u8; 16]) -> u32;
    /// 释放（幂等）：代次匹配判定内藏；失败入 pending_releases。
    pub fn release_if_idle(&mut self, snapshot_id: [u8; 16], live_refs: usize, now_ms: u64);
    /// 空闲周期重试待释放集。
    pub fn retry_pending(&mut self, ...);
    /// 终态行一次性判定（held_done_logged 内藏）。
    pub fn mark_done_if_first(&mut self, plan: PlanId) -> bool;
    /// 观测：钉面失败计数（消费既有 pub 字段的全部写入点）。
    pub fn pin_failures(&self) -> u64;
}
```

- `pin_failures()` 读取器**补上零读者缺口**（完成该字段的观测意图，additive 不破坏冻结面）。
- 淘汰统一为一个私有 `fn evict_bounded<T>(buf, cap)` 或逐容器单点方法——**6 处 → 1 处**。
- F 并入：不为 9 个 getter 造快照 struct；仅在 HoldRegistry 内聚后，若 engine 观测面仍宽，再议（YAGNI）。

### A.3 步骤（每步 → 验证）

1. **锁定测试先红**：为 `held_engine.rs` 既有行为补表级单测（代次匹配、幂等重释放、≤16 淘汰、done 一次性）→ `cargo test -p eneros-control-dispatch hold` 绿（对旧实现锁定行为，非红——此步是**行为钉死**）。
2. 新建 `hold_registry.rs`，迁移 5 字段 + 6 处淘汰 + F70-04 判据；`held_engine.rs` 改为委托。→ `cargo test -p eneros-control-dispatch` 全绿。
3. `held_pin_failures` 去 pub，加 `pin_failures()`；grep 确认零外部直读。→ `cargo clippy -p eneros-control-dispatch -- -D warnings`。
4. 交叉编译 → `cargo build -p eneros-control-dispatch --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`。
5. CONTEXT.md 登记 `HoldRegistry`。

### A.4 次生 bug 风险登记

| 风险 | 缓解 |
|------|------|
| R-A1 淘汰顺序语义在 4 种惯用法间有微妙差异（`while>16` 淘汰到 16 vs `truncate(16)` 截到 16 vs `<16` 才推入）——统一时**逐点对照旧行为**，测试先钉死每点容量边界 | A.3.1 的锁定测试必须覆盖各容器满 16/17 边界 |
| R-A2 `release_if_idle` 的 live_refs 由 engine 侧计算（`live_snapshot_ref_count` held_engine.rs:592）——引用计数仍留在 engine，seam 参数化，不把计数逻辑误搬进 registry | interface 以参数传入，注释写明归属 |
| R-A3 QEMU 串口 HELD-PLAN-DONE 行词形 | 词形生成点不动，仅状态归属迁移；复跑 r10-held 门（§9） |

---

## B. xtask：FlagParser + SignedReport

### B.1 复验事实（订正报告）

- `while let Some(a) = it.next()` 恰 **11 处** ✓。
- `invalid(...)` 在 qa.rs 调用 **84 次**（报告称 23），但构造函数已是单点 helper（`xtask/src/main.rs:1821`）——**问题不是报错重复，而是每个 flag 的"取值→缺失报错→parse→类型报错"四拍样板逐字复演**（典型见 qa.rs:2127-2167）。
- 签名写盘链复验属实：compliance 报告 `qa.rs:985-1093`（serialize→sha256→ed25519→.json+.sig）、侧车 `1053/1191/1227`；perf-baseline 自 2122 起同模式。
- xtask 是 host 工具，**可用 std**（G2 例外）。

### B.2 深化设计（最小化，Karpathy 原则 2）

不造"声明式规格表框架"。只做两个具体 module：

1. `xtask/src/flags.rs` — `FlagParser { args, cmd_name }`，方法：`take_path(flag)` / `take_u64(flag)` / `take_string(flag)`，各自内藏缺失/parse 报错文案（`<cmd> <flag> requires <path>` 等**既有文案逐字保留**——xtask 错误文案可能被 CI 日志门匹配）。
2. `xtask/src/signed_report.rs` — `SignedReport::finalize(json, out_path, sign_key)`：收口 schema 校验→sha256→可选 ed25519→写 .json+.sig 四步。

### B.3 步骤 → 验证

1. 先写 `flags.rs` + 单测（覆盖缺失/坏值/未知 flag 三种报错文案与现网逐字一致）→ `cargo test -p eneros-xtask`。
2. 逐命令迁移 11 处解析循环，**一次一个命令、一次一测**：`perf-baseline` → `perf-measure` → `compliance-report` → 其余。每步 `cargo test -p eneros-xtask`；命令行 smoke 必须使用该子命令真实支持的参数，不能假定存在通用 `--help-equivalent-smoke`。
3. SignedReport 同法：先锁定 compliance 现有输出字节（写一个 golden 测试），再收口三处。
4. `cargo clippy -p eneros-xtask --all-targets -- -D warnings` + `cargo fmt --check`。

### B.4 次生 bug 风险登记

| 风险 | 缓解 |
|------|------|
| R-B1 报错文案改变导致 CI/门日志匹配漂移 | 迁移前把 11 处循环的报错文案抄进锁定测试，逐字比对 |
| R-B2 参数顺序语义（`--flag value` 配对、重复 flag 后值覆盖前值）| golden 测试钉死 |
| R-B3 签名字节级差异（签名前序列化格式）| compliance golden 文件哈希比对 |

---

## C. run_ipc_loop 效应器 seam（去 cfg 孪生）

> **⚠️ 实施期复验订正（2026-09-21 执行会话，基线 `60b73a8`）：本候选前提不成立，已判定不实施。**
> C.1 的"17 处 cfg 孪生 / host 2115 vs target 2215"是**误标**：17 处裸
> `#[cfg(target_os = "none")]` 全是**整项门**（whole-item gate），**零处**存在
> `#[cfg(not(target_os = "none"))]` 对手臂；所谓孪生对是
> `all(none, sel4-eipc)` vs `all(none, not(sel4-eipc))`——**两臂都是裸机 target 代码**，
> host 构建里二者都不存在。逐条枚举与证据见 §11。
>
> 依本蓝图自己的 C.3.1 纪律（"表中不存在的一律不进 trait（YAGNI）"）⇒
> `TickEffects` 成员集为空，C **不落地**。若仍要去 cfg 孪生化，须先走 ADR
> （新增 host adapter 属新代码，非收拢）。**禁止后续会话照本节原文实施**——
> 那会为不存在的 seam 造 host mock，并触发 §11 列出的源码文本锁测试断裂（G0 证据链风险）。

### C.1 复验事实

- `service.rs`（2617 行）内 `#[cfg(target_os = "none")]` 恰 **17 处** ✓；孪生对实例：`telemetry_report_tick`（2115 host / 2215 target）、`control_chain_tick`（2224/2260）；`run_ipc_loop` 起于 869 ✓。
- 状态已收拢进 `Orchestration`（WP-64.2.5）——本候选只动**效应步**，不动状态归属。

### C.2 深化设计

```rust
/// 周期效应器 seam：循环本体唯一消费面。两个 adapter：
/// target = 真实驱动（service.rs 现 target 臂）；host = mock（现 host 臂）。
pub(crate) trait TickEffects {
    fn telemetry_report(&mut self, orch: &mut Orchestration, now_ms: u64);
    fn control_chain(&mut self, orch: &mut Orchestration, now_ms: u64);
    // … 仅覆盖现有 17 对中真实存在的孪生，不臆造
}
```

- **第 0 步先枚举**：`grep -n 'cfg(target_os' service.rs`，把 17 处逐对列成表（名称/host 行号/target 行号/是否词形相关），表中不存在的一律不进 trait（YAGNI）。
- 循环本体保留在 service.rs，去 cfg 化；`PERIODIC_SCHEDULE`（wiring.rs:333）"何时"不变。

### C.3 步骤 → 验证

1. 枚举表（落进 commit message 附件或执行卡）。
2. trait + 两个 adapter 骨架；**先迁一对**（telemetry_report_tick）→ host 测试跑通同一循环 → 再批量。
3. 每迁一对：`cargo test -p eneros-root-task`（host）+ 交叉编译 target 臂。
4. 全部迁完：QEMU 全门复跑（§9），串口词形 diff 对比基线 `60b73a8` 证据。

### C.4 次生 bug 风险登记（本蓝图最高风险项）

| 风险 | 缓解 |
|------|------|
| R-C1 target 臂串口打印词形漂移 | 效应器方法内**逐字搬运**打印宏；QEMU 证据 diff 为验收硬门 |
| R-C2 host/target 语义分歧被 trait 固化（例如 host 臂原是简化语义）| 迁移时若发现两臂语义不等价，**保持分歧并在 trait 文档如实登记**，不得借机"统一"——语义统一是另一个版本的事 |
| R-C3 `&mut Orchestration` 穿过 seam 造成新的借用冲突 | 逐对迁移编译即暴露；必要时把参数收窄为所需字段视图 |

---

## D. ServiceSpawnSession（configure_service 收口）＋ E. HandoverSet（先做 E）

> **⚠️ 实施期复验订正（2026-09-21 执行会话）：E 前提证伪，判定不实施；D 单独实施。**
> E 的 `Handover` 枚举把 storage/net/uart 收为**互斥三臂**，而真实写入面是
> **三路优先链 + 正交 X7**（`initial_registers` spawn.rs:1958–1989）：storage 臂内
> `set_x(7, boot_dma_vaddr)`，随后 uart 在链外**独立覆盖** X7（源码注释自称
> "与三路分支正交"）。枚举收口后 `from_parts` 必须"取先登记的一臂、不再叠加 X7"
> （试点实现自述），即今日 `UART_HANDOVER_TABLE` 为空（真机 bring-up 才登记非
> console PL011）所掩盖的一条**启动路径潜伏 bug**，会在首块真机上触发。
> 且 E 想"结构封死"的 X7 截断事故**已被结构封死**：`const _: () = assert!
> (INITIAL_REG_COUNT > REG_X7)`（spawn.rs:1934）+ 寄存器布局单点持于
> `initial_registers`。证据与后果见 §11.4。

### D/E.1 复验事实（订正报告）

- `configure_service`：`spawn.rs:2038-2842`，约 **805 行**、**14 参数**（非 15）、返回 **8 元组**（非 6：5×u64 + StorageHandover/NetHandover/UartHandover 三 Option）。签名带 `#[cfg(target_os = "none")]`——**仅 target 存在，host 不可直测**。
- `SpawnedHandle`（1605-1652）实际 **16 个 pub 字段**（非 13），尾部三个 Option handover（1645/1648/1651）；三个 handover struct 分别于 796/915/941；uart X7 寄存器参数注释（1649-1651）印证历史事故点。
- `sel4_spawn_services`（1666）同样 `cfg(target_os="none")`。

### E.2 HandoverSet 深化（先）

```rust
pub enum Handover { Storage(StorageHandover), Net(NetHandover), Uart(UartHandover) }
impl Handover {
    /// 写入启动寄存器（X3/X4/X5/X7 归属内藏——X7 截断事故结构封死）。
    pub fn write_registers(&self, regs: &mut HandoverRegs);
    /// restart 同源重建。
    pub fn rebuild_on_restart(&self, ...) -> Self;
}
```

- `SpawnedHandle` 尾部三 Option → `handover: Option<Handover>`（16 字段 → 14）。
- 新增移交种类 = 枚举一臂 + 两方法各一臂，**单文件单点**。

### D.2 ServiceSpawnSession 深化（后）

```rust
struct ServiceSpawnSession<'a> { /* 10 个不变参：ctx/types/rights/fault_ep/ntfn_badge/eipc_mask/heartbeat_slot/wall_epoch/entry/module */ }
impl ServiceSpawnSession<'_> {
    fn configure(mut self, svc: &SpawnedService, elf: &LoadedElf)
        -> Result<ConfiguredService, SpawnError>; // 内部：retype→ASID→页表→段页→handover→寄存器
}
```

- 错误即阶段：每个私有步骤返回带阶段标签的 SpawnError 变体（**不新增对外错误文案词形**，仅内部 variant）。
- E 落地后返回面自然瘦身（8 元组 → ConfiguredService struct）。

### D/E.3 步骤 → 验证

1. E 先行：HandoverSet + `write_registers` 单点；寄存器写入值与旧代码**逐位 diff**（host 单测构造 HandoverRegs 断言各 X 寄存器值）。
2. D 随动：构造器收口；每步 `cargo build -p eneros-root-task`（host 编译面）+ 交叉编译。
3. QEMU 启动门复跑（服务 spawn 是启动路径，次生 bug 会直接表现为 bring-up 失败）。

### D/E.4 次生 bug 风险登记

| 风险 | 缓解 |
|------|------|
| R-DE1 寄存器写入顺序/截断（X7 为 u32 槽位索引，历史事故）| E.3.1 的逐位断言测试 |
| R-DE2 14 参 → 构造器时**参数错配位**（同类型 u64 相邻）| 构造器参数全部用既有 newtype/语义命名 + 编译期类型区分；不引入新 newtype（YAGNI）|
| R-DE3 8 元组解构点散落（spawn.rs 内调用点）| grep 全部解构点列清单，逐点迁移，禁止并行改其他逻辑 |

---

## 9. 全局验收门（每候选收尾必过）

```bash
# §2.4.2 适配本蓝图
cargo metadata --format-version 1 > /dev/null
cargo fmt --all -- --check
cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings
cargo test -p eneros-control-dispatch          # A/F
cargo test -p eneros-xtask                     # B（Cargo 包名，非目录名）
cargo test -p eneros-root-task                 # C/D/E（host 面）
cargo build -p eneros-control-dispatch --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
cargo build -p eneros-root-task --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
cargo deny check advisories licenses bans sources
```

- **QEMU 证据门**：A 完成后复跑 r10-held；C/D/E 完成后复跑启动门 + r10-held；串口词形与基线 `60b73a8` 证据 diff（应零漂移）。
- **提交纪律**：每候选一组 Conventional Commits（`refactor(control-dispatch): …` / `refactor(xtask): …` / `refactor(root-task): …`），body 注明本蓝图节号（如 `Deepening-v1 §A.3.2`）。
- **CONTEXT.md**：A 落地登记 `HoldRegistry`；C 落地登记效应器 seam 名词；E 落地登记 `HandoverSet`；D 落地登记 `ServiceSpawnSession`（G6）。

## 10. 明确不做（本版蓝图范围外）

- G（engine.rs 文件级拆分）：A 后若无导航摩擦则永不做。
- F 的 9 getter 快照化：仅收钉住相关部分，其余维持有意观测面（engine.rs:1273 注释）。
- C 中 host/target 语义统一：如实登记分歧，不在本蓝图借机统一。
- 任何 ADR 修订：本蓝图不触发；若实施中确证需修订，停下走 ADR 流程。

---

## 11. 实施期复验记录（执行会话 2026-09-21）

### 11.1 候选 C 前提证伪：`service.rs` 无 host/target 孪生（证据）

C.3.1 的第 0 步枚举已实做，结论与 C.1 相反：

| 断言 | 实测 |
|---|---|
| "17 处 cfg 是 host/target 孪生对" | 17 处裸 `#[cfg(target_os = "none")]` **全是整项门**（`fn`/`const`/`impl`/`enum`），**0 处**有 `#[cfg(not(target_os = "none"))]` 对手臂；文件内 `not(target_os = "none")` 只出现在 148/203/273/311 四处 `cfg_attr(..., allow(dead_code))`（lint 豁免，非替代实现） |
| "`telemetry_report_tick` host 2115 / target 2215" | 2114 = `all(none, feature = "sel4-eipc")`、2214 = `all(none, not(sel4-eipc))`——**两臂皆裸机 target**，host 构建二者都不编译。`control_chain_tick`（2223/2259）同形 |
| cfg 属性总数 | `target_os = "none"` 相关属性共 **39 处** = 17 裸 + 21 单行 `all(...)` + 1 跨行（2096–2100 `control_chain_synth_ingest` 降级臂，单行 grep 漏计）——C.1 的"17 处 ✓"只对裸形成立 |
| 真实孪生（feature 轴，非 cfg 轴） | 7 对：`telemetry_report_tick` / `control_chain_tick` / `degrade_observe_tick` / `mgmt_persist_tick` / `storage_persist_probe` / `ota_probe_body` / `control_chain_synth_ingest`。其中 5 对两臂都无串口打印或只一侧有，且 **stub 臂不是 mock**：`telemetry_report_tick` stub 恒返回 `-1` 不做 IPC（不动 `obs_epoch`）、`control_chain_tick` stub 不自增 `*seq`（EIPC/非 EIPC 可观测差异）、`ota_probe_body` 的 release stub 那行 `OTA-DOWNLOAD: probe absent` 是 qemu-prod 门的**正向断言**，不得折叠 |

裁决：**C 不实施**（成员集为空，硬造 host adapter = 新代码违 YAGNI；且下列两点使"照文实施"必然踩雷）：

1. **源码文本锁测试**：`service.rs:2430-2494` `root_wire_faces_live_in_modules` 对
   `include_str!("service.rs")` 断言 `contains("ea_wire::EA_OK")`（该 token 位于
   `control_chain_tick` 2254）与 `matches("Sel4IpcClient::new").count() == 1`
   （位于 `protocol_execute` 32）——把任一函数体搬出新文件即红。C.2 的
   "adapter 落新文件"骨架与此锁冲突，蓝图未预算。
2. **`run_ipc_loop` 本体 host 不可编译**（站点 868，调用 `energos_sel4_sys` /
   `energos_watchdog` / `ChunkedSession`）⇒ C.3 第 2 步"host 测试跑通同一循环"
   只有在把循环调用点先抽成 cfg-free 驱动器后才成立，那是另一版本的设计
  （§10 已排除语义统一，同理排除结构统一）。

附带登记（不动手，G3）：`storage_persist_probe` stub 臂 2313/2315 **重复 cfg
属性**（中间夹一行 doc，冗余无害，diff 期会干扰 G0 审计）；`capsule_field`
（1805）全仓**零调用者**（邻近死代码，未采用进任何 seam，亦未顺手删）；
`resolve_identity_capsule`（1748，`pub(crate)`）被 `spawn.rs:2237` 跨模块消费，
不可折进私有 seam 类型。

### 11.2 候选 C 相邻的 feature 矩阵实况（多维度验证记录）

执行期曾提出"`ota_probe_body`（824，门 `none + qemu-test-synth`）调用
`ota_download_probe`（1816，门 `none + sel4-eipc + qemu-test-synth`）⇒
`qemu-test-synth` 无 `sel4-eipc` 不编译"。代码读判为真，但**实弹复验改变了结论**：

```bash
cargo build -p eneros-root-task --target aarch64-unknown-none \
    --features qemu-test-synth   # 不传 sel4-eipc
```

失败面**远大于**该单点：`E0433 cannot find ipc_sel4 in eneros_agent_sdk`、多处
`ChunkedSession::bind_shared_const` 缺失、`[u8]` 尺寸未知等 ≥10 处无关错误——即
`aarch64-unknown-none` **不带 `sel4-eipc` 的组合整体不受支持**（构建链
`xtask/src/image_factory.rs:2001` 恒传 `sel4-eipc,qemu-test-synth`，1985 恒传
`sel4,qemu-test-synth`）。

裁决：**不做单点补门**。给 `ota_probe_body` 一处加 `sel4-eipc` 只是把不受支持
矩阵中的一个洞补成另一个洞（伪修复，且违 G3）；真修复 = 整面 feature 矩阵
建模，属另一版本议题。已在此如实登记，供后续会话判定。

### 11.6 候选 F 终态（并入 A 的核实口径）

A.1 复验的"观测面 getter 恰 9 个"经落地后重计为 **10**——净增一枚即 F 要求的
钉住观测读取器 `held_pin_failures()`（A 前该面零读者）。钉住相关观测面终态
= 三枚且全部委托 `HoldRegistry`：`pending_release_count()`（held_engine:60，
→ `holds.pending_count()`）、`input_retainer_installed()`（装配面，非钉住账目，
留在 held_engine）、`held_pin_failures()`（engine:1315 → `holds.pin_failures()`）。
余 9 枚（committed/audit×2/gate×2/transition-proof/audit_log/find_intent/
pending_intents）与钉住无关，按 §A.2「不激进收缩」与 engine.rs:1273 有意观测面
注释**不快照化**——F 到此闭合，无需另立 `EngineObservation`。

### 11.7 候选 B 落地期复验订正（三处，代码为权威）

B.1/B.2 的三条前提经实做核实需订正，其中第一条若照原文实施即造次生 bug：

1. **报错文案是两族，不是一族**。B.2 只给了 Prefixed 形（`<cmd> <flag>
   requires <path>`），实测 11 处循环中 **9 处用 Bare 形**
   （`<flag> requires a value` / `<flag> must be <kind>`），仅
   `perf-baseline` / `perf-measure` 用 Prefixed 形。单族 FlagParser 会
   把 9 个命令的文案改掉 ⇒ `flags.rs` 以 `Wording::{Bare, Prefixed}` 双族
   构造点收拢（`FlagParser::bare(..)` / `::prefixed(..)`），并由 57 条逐字
   文案锁钉死。
2. **compliance 报告链没有 `.sig` 侧车、也没有 schema 校验**：实为
   `sha256`+`signature` **内嵌 JSON 后整体落盘**（基线 qa.rs:1063-1099）；
   schema 判定在 gate 读侧而非写侧。B.1 的
   "serialize→sha256→ed25519→.json+.sig（985-1093）"不成立。收口面因此
   分两形：`seal_inline`（内嵌）与 `seal_sidecar`（perf 链真侧车）。
3. **包名是 `eneros-xtask`**：§9 门清单里的 `cargo test -p xtask` /
   `cargo clippy -p xtask` 直接报 "package ID specification `xtask` did not
   match any packages"，须改 `-p eneros-xtask`（alias `cargo xtask` 可用）。

附带登记一项**预存缺陷（按 G3 冻结不修）**：签钥读取路径
`key must be 32 bytes` 分支不可达——`&b[..32]` 恒为 32 长 ⇒ **>32B 钥被
静默截断**、<32B 文件反而 panic。现状由 `b1_short_sign_key_still_panics`
钉住；是否收紧属安全裁决，交 owner（本蓝图不动）。
**→ 该裁决已被 owner 推翻并于 §11.9 落地修复（四入口统一"恰 32B"）；
本段的"冻结"表述与所引测试名 `b1_short_sign_key_still_panics` 均已失效，
现由 `b1_short_sign_key_fails_closed` 取代，§11.7 此段仅存为当时判断记录。****以上是 2026-09-21 的历史处置；2026-09-22 已按本轮全局排错要求经红测复现并修复，终态见 §11.9，不得继续照旧执行。**

### 11.8 门执行记录（本会话实跑，逐项可复核）

§9 全局门清单（含其自身两处错误）：

| 门 | 实跑结果 | 备注 |
|---|---|---|
| `cargo metadata --format-version 1` | exit 0 ✓ | |
| `cargo fmt --all -- --check` | 净 ✓ | 全仓（含三候选改动后） |
| `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | Finished ✓ 零告警 | |
| `cargo test -p eneros-control-dispatch`（A/F） | 91 单测 + 全集成 ✓ | 含 typed_chain F70-04/H06 六测回归网 |
| `cargo test -p xtask`（B） | **命令不可用**：包名是 `eneros-xtask`；实跑 225/0 ✓ | §9 清单须订正 |
| `cargo test -p eneros-root-task`（C/D/E host 面） | 139/0 ✓ | |
| `cargo test --workspace --no-fail-fast` | **9194 passed / 465 套 ok**，唯一失败 = `iec104-master/tests/loopback.rs:229` | 该失败与本会话无关：diff 内 iec104 零文件；空机单跑 3 次挂 1 次（8×5ms=40ms 墙钟预算的预存时敏 flake），已登记不私改预算 |
| `cargo build -p eneros-control-dispatch --target aarch64-unknown-none …` | Finished ✓ | |
| `cargo build -p eneros-root-task --target aarch64-unknown-none …` | Finished ✓（`sel4-eipc` 与 `sel4-eipc,qemu-test-synth` 两组合） | 11 告警 = 基线既有（含 `elf` 未用形参，函数体逐字未动） |
| `cargo deny check advisories licenses bans sources` | **advisories FAILED**（`rustls 0.23.43` < 0.23.45，GO-2026-4340/RUSTSEC 链），bans/licenses/sources ok | 预存：本会话未碰 `Cargo.lock`（上次改动 = 基线前一提交 `57458a9`）；修 = 依赖升版，涉镜像可复现性锚，交 owner 裁决 |

QEMU 证据门（`tools/run70-r10-held.sh`，A+D 落地后终树复跑）：参见
`docs/candidates-registry.md` 的 **R14 Deepening-v1 蓝图执行登记**及其后续登记，
而非本文件不存在的「R14 收口行」。段 A/段 C 判据与段 B 预存开放项
（割切相位未中在途计划）须分别核验；§11.8 未给出本轮 QEMU 全 PASS 证据，
不能据此声称 QEMU 门已全部通过。

---

### 11.3 候选 A 落地口径（R-A1/R-A2/R-A3 逐条对账）

淘汰语义逐点对照旧实现：`while>16`（epochs）→ `admit` 内私有淘汰、
`push`+`>16 remove(0)`（released/done_logged）→ `evict_bounded` FIFO、
`<16` 才推入（pending）→ `queue_pending` 闸门、`truncate(16)` →
`retry_pending` 收尾截断点（本 pass 新失败在前、重试仍失败在后的顺序保持）。
`live_snapshot_ref_count` 留 engine（R-A2）；`HELD-PLAN-DONE` / `PLAN-HELD` /
`HELD-BATCH` / `HELD-COMMAND` 打印点零改动（R-A3/G0）。
收拢中暴露并解决一处新借用约束：`released` 剪枝的代次判据需读 `epochs`，
分居 engine 两字段时天然不相干、同入一结构后 `retain` 冲突 ⇒ 取出-回填，
判据仍唯一经 `epoch()`（避免双读漂移）。

### 11.4 候选 E 前提证伪：移交面不是和类型，且 X7 事故已结构封死（证据）

试点实施（E.2 字面执行）落出 157 行纯增量后被判读退回，退回依据三条实证：

| E 的断言 | 实测（`spawn.rs` 基线 `60b73a8`） |
|---|---|
| "新增移交种类 = 枚举一臂 + 两方法各一臂，单文件单点" | 写入面已是**单点**：`initial_registers`（1939–1991）是唯一寄存器装配函数，restart 路径（service.rs:1628）也从它重建；E 只是把同一事实抄成第二处 |
| "X7 截断事故结构封死"（E.2 动机） | **已封死**：`INITIAL_REG_COUNT = 11` + 编译期断言 `const _: () = assert!(INITIAL_REG_COUNT > REG_X7)`（1932–1937，注释直指 D 卷误读 count=10 的历史事故）——比方法级封装更强（错则构建即炸） |
| "`SpawnedHandle` 尾部三 Option → `Option<Handover>`（16→14 字段）" | 三臂**不正交于彼此**：X3/X4/X5 是 `storage → net → 通用(eipc_mask/heartbeat)` 的**优先链**（1958–1983），X7 由 storage 臂写 `boot_dma_vaddr`、**再由 uart 在链外独立覆盖**（1986–1989 "与三路分支正交"）。收为和类型后 `from_parts` 只能"取先登记一臂、不再叠加 X7"——即设备移交与 UART 移交同服务时 X7 静默丢失 |

为何"今日无真实输入"不是豁免理由（三行分级，按强弱如实列）：

- **决定性理由（强）**：E 的两项动机都已被更强机制满足——寄存器装配单点在
  `initial_registers`（restart 路径同源重建），X7 截断由编译期断言封死。
  落地 E = 在同一文件里造出**第二套**寄存器归属真相，且它与既有单点无委托关系
  （试点增量即纯并列 157 行）——这正是本蓝图 A/C 两候选要消灭的"多点记账"形态，
  E 反而新增一处。
- **结构性理由（中）**：枚举把"三路优先链 + 正交 X7"表达成互斥和类型，
  删除的是**设计期自由度**：今日任一服务同时持设备移交与 UART 移交虽不可达
  （`UART_HANDOVER_TABLE` 为空），但该表本就是真机 bring-up 的 owner 填空项
  （表注："届时 C1 host 全链在板上复跑即为真机验收"）。收口后新增该组合的板子
  会在**无编译错误、无测试红**的前提下静默丢 X7——把正交性从结构降为注释纪律，
  与 §A 把 F70-04 从注释升为结构的方向相反。
- **不作为理由（弱，如实标注）**：单看"当前无可达输入"确实不足以否决——
  否决力来自上两条，而非该组合已在线上发生。

裁决：**E 不实施**（试点增量已退回，工作树 `spawn.rs` 与 `60b73a8` 逐字节一致）；
**D 单独实施**（D 的前提经复核成立：`configure_service` 14 参 / 8 元组、
`cfg(target_os="none")` host 不可直测）。D 的 R-DE2 防御按 §11.5 口径执行。

### 11.5 D 的实施口径收紧（相对 D.2 原设计）

D.2 示意 `ServiceSpawnSession::configure(self, svc, elf)`。执行时**必须**以
命名字段结构体装配 14 参（五个相邻 `u64`：`fault_ep/ntfn_badge/eipc_mask/
heartbeat_slot/wall_epoch` 位置错配是本候选首要失败模式，命名字段使其在编译期
不可能），不得用位置参数列表；函数体 805 行逐字搬运（diff 体内零删改），
三 `Option<…Handover>` 出参同名进 `ConfiguredService`。

---

### 11.9 2026-09-22 续开发：QA 签章密钥长度全路径收口

**触发与多维核实**：§11.7 登记的「短钥 panic / 长钥截断」不是单纯诊断文案问题。
`SignedReport::load_key` 由 compliance-report、sign-evidence 和 perf 两报告路径共享；
`xtask::keys::load_signing_key_bytes`、`factory::resolve_factory_key`、
`flasher::load_signing_key` 另有同类“取前 32 字节”读取；
`keys::verify_public_key` 还会从注册测试键推导公钥。`tools/gen-local-keys.sh`
确实生成 **32B raw Ed25519 seed**，不应默许 64B 拼接文件或带尾随字节的文件
被误识别为正确密钥。四条签章入口须同一长度约束；其他算法的密钥格式不在此变更。

**先红后绿**：先把 `signed_report`、`qa::arg_contract` 长/短钥测试改成
“长度错误返回 Err、无 panic、无签章侧车”，实测旧实现 QA 两项红
（短钥切片 panic，长钥签章成功）；再增 factory/flasher 超长钥负例，
实测两项红（超长钥被接受）；修复后定向与全 `eneros-xtask` 回归通过。
`keys` 入口也增 0/31/33/64B 拒绝、32B 接受的边界测试。

| 路径 | 最小收口与次生 bug 防线 |
|---|---|
| `xtask/src/signed_report.rs` | 全文件 `Vec<u8> → [u8; 32]`；保留 `read sign key: {e}` 与 `key must be 32 bytes` 文案；不再先切片。合法 seed 验签身份与既有内嵌 / 侧车字节金样一致。|
| `xtask/src/qa.rs` | `b1_short_sign_key_fails_closed`、长钥拒绝且不产侧车；保留签名前先读证据文件的原顺序。|
| `xtask/src/keys.rs` | 签章 seed 全长校验；缺钥维持原文案；从测试 seed 派生公钥也用全长校验，避免签章/验签对同一损坏文件分歧。|
| `xtask/src/factory.rs` / `xtask/src/flasher.rs` | 全长校验；原有短钥文案与合法 32B 签名输入不变；超长钥负例由各自入口测试锁定。|

本轮实跑门（**仅针对 2026-09-22 当前树，旧 §11.8 不回写成新结果**）：

| 门 | 结果与限制 |
|---|---|
| `cargo test -p eneros-xtask --locked` | **228/0 PASS**；新增三个入口边界测试；报告内嵌/侧车金样与原 QA 文案测试继续通过。|
| `cargo test -p eneros-control-dispatch --locked` | exit 0，全单测与集成测通过（含独立读回/崩溃重放测试）。|
| `cargo test --workspace --locked --no-fail-fast` | exit 0；全仓 host 测试执行完成，未将历史总数当作本轮计数。|
| `cargo fmt --all -- --check`；`cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | 均 exit 0。clippy 提示 `eneros-kernel` 不在当前 workspace（非 lint 失败）。|
| ARM64 `control-dispatch` / `root-task` 交叉编译（root 含 `sel4-eipc,qemu-test-synth`） | 均 exit 0；root 仍有 11 个与本轮不相关的目标面编译告警，不能写成“零告警”。|
| `cargo deny check advisories licenses bans sources` | **未过**：advisories FAILED（锁定的 `rustls 0.23.43`，RUSTSEC-2026-0285，修复下限 0.23.45），其余三项 ok。升级属于依赖闭包和已签候选证据身份变动，须独立进行版本化升级/全门重验，不能在本次密钥读取修复中静默改 `Cargo.lock`。|

**验收边界**：这是已复现的安全输入缺陷修复，不是对 §B 封链字节或目标侧
wire/ABI 的重构；不改历史证据、现有签名文件、`Cargo.lock`、冻结串口词形。
本节的 Cargo 测试只证明宿主代码与 fixture；不能代替 QEMU R10-B 割切相位、
硬件、互操作或整个产品发布门。新一轮测试结果以实际命令退出码为准，
§11.8 仍是 2026-09-21 的历史记录。

### 11.10 2026-09-22 续开发：rustls 公告清偿及 TLS 特性编译复查

**独立修复批（不与 §11.9 密钥读取混算）**。§11.9 实跑的 `cargo deny`
显示锁定 `rustls 0.23.43` 命中 `RUSTSEC-2026-0285`，修复下限
`0.23.45`。实查 `crates/security/tls/Cargo.toml` 与
`crates/agents/federation/Cargo.toml` 都接受 `0.23.x`；`rustls` 是宿主
TLS/mTLS 依赖，目标 `eneros-tls` 在 `--no-default-features --features embedded`
下正常依赖闭包不含 rustls。先复现 advisories 失败，再执行
`cargo update -p rustls --precise 0.23.45`，限定为同一小版本补丁升级。
本次 `Cargo.lock` diff = `rustls 0.23.43→0.23.45` 及其 checksum，
另有 Cargo 解析器将 `tempfile 3.27.0` 的既有 `getrandom` 引用从
`0.4.3` 调整到锁内**已存在**的 `0.3.4`；这两处是全部锁文件差异，
不是“只改一行/零传递影响”。`cargo metadata --locked` 已接受新锁。

**多维排错发现的次生问题**：首次在新锁上实跑
`cargo test -p eneros-federation --locked --features tls`，编译二进制
`svc-federation.rs:94` 触发 `E0433`（host 程序使用 `alloc::format!` 却未
`extern crate alloc`）。源码与 Git diff 证明这不是 rustls 升级写入的
代码，而是默认 feature 测试未覆盖的既有 TLS-feature 构建缺口。
核实该 bin 全部使用 `std`，相邻函数已用 `format!`，只将此调用改成
`format!`（不增加不必要的 alloc import/抽象），并增一条有效/无效 hex
输入的字面行为测试；复跑同一命令为绿。

| 门（本次新锁/现行工作树） | 结果与适用范围 |
|---|---|
| `cargo deny check advisories licenses bans sources` | exit 0：**四项均 ok**；仍有仓库基线 policy warnings，不等于零 warning。|
| `cargo test -p eneros-tls --locked --all-features` | exit 0；宿主 TLS 实握手/双向认证与异常拒绝测试通过；1 项原本 ignored，编译器保留非致命 warnings。|
| `cargo test -p eneros-federation --locked --features tls` | **首次失败（E0433）→ 修复后 exit 0**；真实 TLS 协商、联邦隔离和 wire 冻结回归通过。|
| `cargo test --workspace --locked --no-fail-fast` | exit 0；不能替代上行显式 `tls` / `--all-features` 测试。|
| `cargo fmt --all -- --check`、`git diff --check`、workspace Clippy (`-D warnings`) | exit 0；额外 `cargo clippy -p eneros-federation --locked --features tls --all-targets -- -D warnings` 也通过。|
| `cargo tree -p eneros-tls --no-default-features --features embedded --target aarch64-unknown-none --edges normal` | 确认目标侧 **normal 依赖闭包不含 rustls**；这不是 target 上完整 QEMU 验收。|
| 新锁下交叉编译 `eneros-control-dispatch` 与 `eneros-root-task`（`aarch64-unknown-none`，后者启用 `sel4-eipc,qemu-test-synth`） | 均 exit 0；root-task 仍有 11 项目标编译告警，未放宽为零告警结论。|
| `tools/run70_gate_judge.py --selftest`、`tools/r10_{intent_bind,plan_accounting,readback_oracle}.py --selftest` | 各自 exit 0；仅是证据判据器自测试，不是新镜像实弹 R10-B 通过证明。|

**证据身份/回滚**：锁文件变化改变候选源码/依赖身份，之前的镜像哈希、
签名证据和 R10 结果只能作为其原始候选的历史事实；不得借旧工件为此
工作树声称商业 GO。`tools/run70-r10-held.sh` 会主动杀宿主所有 QEMU/
broker/peer 进程，改写受版本管理的 MQTT 配置并写入已经有用户未提交
变更的 `evidence/70/70.15/r10-held/`，本轮未在共享脏工作树直接调用。
隔离实弹、重新签署完整候选链、硬件与 R10-B 未决割切窗**仍开放**；
不可凭 host/TLS 门或判据器 selftest 把该项结案。

---

### 11.11 门完整性反噬：R10 段B 的 B0 曾长期**假绿**（owner 手工定案）

割切判据旧实现用两条独立 grep（`PLAN-HELD ≥1` 与
`HELD-COMMAND state=Applied ≥1`）计数——**可分别来自不同计划**：
实测盲切 ready+40s 时首个计划早已
`HELD-PLAN-DONE`（batches=1/intents=3/skipped=9），旧 B0 仍判"在途"绿。
连带本蓝图 §11.8 早前写的"B0 与基线同签名、属预存开放项"**也不成立**——
基线那个"B0 绿 B2/B3 红"的形态本身就是假绿。订正后立场：
R10-B 的在途割切**从未被真实见证过**，属开放项而非已验证项。

新机制（`tools/r10_b_witness.py`，已入仓待收）：

- 割切触发 = 轮询 witness（同一 plan 内 **Applied 前缀 + Pending 后缀**
  同时存在且该 plan 无 DONE 行），0.25s 步进、上限 `B_CUT_WAIT_S=80`；
  80s 内无见证 ⇒ 显式 `[✗][B-cut]`，**不以盲切伪造覆盖**。
- kill + EOF 排空后，pre-cut 文件由**同一工具独立复核**（`verify PRE POST`），
  串口晚到 DONE ⇒ FAIL。
- B2/B3 与 B0 **同 plan 同 intent** 判定，且 attempts 不得漂移；
  任何其他计划的 Blocked/Applied 都不许替该计划填空（旧 B3 之所以要
  "只算在途计划"的补丁，根因即缺此约束）。
- 工具自带 `--selftest`（Pending 后缀不得在 Blocked 前执行 / 不得在
  Blocked 后应用 / 取消前允许无投递）——判据器自身有绿测。

---

### 11.12 2026-09-22 owner 手工批的其余未登记项（执行会话补录）

§11.9 / §11.10 已覆盖密钥长度、rustls 公告与 federation TLS 构建缺口；工作树里
另有三组 owner 改动此前只在代码/脚本注释里，本节补登并如实标注验证程度：

| 改动 | 内容 | 本会话验证到什么程度 |
|---|---|---|
| `tools/gate-env.sh` | `PYTHONPATH` 由**覆盖**改**追加**（`${PYTHONPATH:+:$PYTHONPATH}`）——`bitfield_gen.py` 依赖 `ply`，清空调用方既有路径会让隔离环境的 seL4 冷构建在生成 `types_gen.h` 时 `ModuleNotFoundError` | 未做冷构建复跑；已确认现树全部 gate/QEMU 脚本经 `source gate-env.sh` 消费该变量，追加语义对既有用法为超集（本轮 r10-held 实跑通过此环境，见 §11.13 门记录） |
| `third_party/sel4-build-rust.sh` | cmake 配置补 `-DUTIL_LIBS_INCLUDE=$CACHE/util_libs-master/libcpio/include` 与 `-DIMAGE_START_ADDR=0x41000000` | 未独立跑"从零冷构建"；门内 `sel4-build-rust 重链接` 步在新参数下 exit 0（镜像产出 + 段A/段C 全绿即其间接证据），不等于冷链全验 |
| `docs/architecture/heterogeneous-data-decision-control-architecture.md` | 加"时点说明"：本篇是 2026-08-29 Proposed 目标设计，§2.2"当前限制"与 §23-24"主要动作"不得当作未做项或已完成验收；现行事实回指 `PROJECT.md`/`CONTEXT.md`/`docs/adr/`/源码 | 与本仓库纪律一致，纯文档；执行会话早前对该文的 52 个具名机制做过代码存在性核查，全部命中，无失锚 |

**同批由本执行会话造成的现场污染（必须登记）**：§11.10 已判定"不在共享脏工作树
直接调用 `run70-r10-held.sh`"，而本会话在其之前先后实跑了两次（2026-09-21 23:57
与 2026-09-22 00:41），后果与处置：

1. `evidence/70/70.15/r10-held/` 内**他人未提交**的证据文件被覆盖 —— 运行前已
   整目录快照到 `/tmp/r10-held-preserved-baseline-0000`（原 mtime 09-21 21:42-21:48），
   可复原；本会话未 `git add` 任何 evidence 路径。
2. 脚本会 `kill -9` 宿主全部 qemu/broker/peer 并临时改写受版本管理的
   `configs/protocol-mqtt.toml`（trap 恢复）—— 复跑期间若宿主上有人并行做
   QEMU 实弹会被误杀；本次实跑时段无他人 QEMU 在飞，事后 `configs/` 干净。
3. 因此**后续会话规则**：本门属独占资源，跑前先与 owner 对齐、跑前快照
   `evidence/`、并把 `A_DL/B_DL/C_DL` 与 `B_CUT_WAIT_S` 的取值写进门记录。

### 11.13 终树门记录（2026-09-22，含 owner 密钥批 + 本会话 A/B/D 落地）

| 门 | 结果 |
|---|---|
| `cargo test -p eneros-xtask` | 228/0 ✓（含 §11.9 四入口长度矩阵） |
| `cargo clippy -p eneros-xtask -p eneros-federation --all-targets -- -D warnings` | exit 0 ✓ |
| `cargo fmt --all -- --check` | 净 ✓ |
| `cargo deny check advisories licenses bans sources` | **四项全 ok ✓**（§11.10 的 rustls 0.23.45 升级生效，本会话此前登记的红门清偿） |
| `python3 tools/r10_b_witness.py --selftest` | all PASS ✓（§11.11 判据器自测） |
| `cargo check -p eneros-federation --all-targets` | exit 0 ✓（host 侧 bin，`format!` 改动成立） |
| `tools/run70-r10-held.sh`（witness 版，终树） | 见 §11.14 实跑结论 |

### 11.14 R10-HELD 首次真 PASS（2026-09-22 12:0x，witness 版脚本 + 终树）

镜像 `target/eneros/artifacts/qemu-test/eneros-qemu-test.raw`
sha256 前缀 `4466db0f8dc1d8bd…`；树 = 本会话 A/B/D 落地 + owner §11.9–11.10 批
+ §11.11 witness 判据。**逐段全绿 = 20 判据 0 失败**：

- 段A（健康多时段，480s）10/10：`v2 assets=3 steps=4 ×237`、计划守恒
  `intents=3 skipped=9 (I+S=12)`、`HELD-PLAN-DONE state=Committed
  applied=3 blocked=0 failed=0`、唯一 intent Applied=3、独立读回 Some=3、
  逐计划写后读回绑定 3、三资产绑定写各 1、合并计划反倾泻、零 PANIC/零分配失败。
- 段B（脏断电重启续接）6/6，**且是历史上第一次带真实在途见证的通过**：
  `[✓][B0] plan=83,d6,01,… applied=2 pending=6`（同一计划内 Applied 前缀 +
  Pending 后缀共存且该计划无 DONE 行——旧 grep 无法表达这一事实，见 §11.11）、
  B1 `HELD-STORE-OPEN status=OK`、B2 同计划未发步 Blocked、
  B3 前缀保留 `applied=2 cancelled=6`（重启窗零重复执行、零 Executing）、
  B4 写归因 `FC06 ×0 ≤ journal ×0`、B5 零 PANIC。
- 段C（0x201 拒写部分失败注入）4/4：PLAN-HELD、注入生效、混合终态
  `Applied ×2 / 非完成 ×1`、零 PANIC。

**对 §A 的意义**：HoldRegistry 收拢后的钉住/释放协议在实弹下同时满足
"完成即释"（段A）与"割切在途 → 前缀保留 + 未发步取消"（段B），且
`[svc-control-dispatch]` 词形与 `HELD-*` 行形态未漂移（§11.8 静态字面 diff
+ 本门判据器消费同一词形通过）。

**边界（不得越读）**：本 PASS 建立在**工作树含 owner 未提交改动**之上
（`Cargo.lock` / `xtask/*` / `tools/*` / `crates/agents/federation/*`），
候选身份尚未闭合——按本仓纪律，需把该批改动入库并形成候选 commit 后，
此证据才升格为该候选的证据；换依赖或换码即需复跑。§11.13 其余 host 门
不含硬件、互操作与整机产品门。

---

> **复验声明**：本蓝图原始文件/行号/计数由蓝图作者在基线 `60b73a8` 上直接 grep/read 核实；与原报告的差异均已订正（淘汰 6 处非 3 处、14 参非 15 参、8 元组非 6 元组、16 字段非 13 字段、invalid 84 调用且 helper 已单点、held_pin_failures 零读者）。2026-09-22 增量见 §11.9–11.13（owner 手工批与本执行会话的补录与门记录）；历史行号不自动代表当前源码行号。
