# EnerOS 架构深化蓝图 v1（Deepening Blueprint）

> **版本**：v1 · **基线 commit**：`60b73a8`（2026-09-21）
> **来源**：架构评审报告 `/tmp/architecture-review-20260921-225312.html` 的 7 个候选，**全部经本蓝图作者逐条代码复验**（复验订正处见各节"复验事实"）。
> **读者**：执行本蓝图的 AI 开发代理。每步给出**验证命令**；每候选给出**次生 bug 风险登记**。严格遵循仓库规则 `.trae/rules/记忆.md`（no_std、Conventional Commits、§2.4 校验清单）与 Karpathy 四原则（先想后写 / 最小实现 / 外科手术式改动 / 目标驱动执行）。
> **词汇纪律**：module / interface / implementation / depth / seam / adapter / leverage / locality（见 codebase-design 词汇表）；领域词汇以 `CONTEXT.md` 为唯一事实源。

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

执行顺序理由：A/B 风险最低且正中热点先行；C 改动面最大、证据链风险最高，放最后且单独成批；D/E 同文件，E 的 HandoverSet 会改变 configure_service 的返回面，故 **E 先 D 后**（先收移交面，构造器返回元组随之瘦身）。

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

1. 先写 `flags.rs` + 单测（覆盖缺失/坏值/未知 flag 三种报错文案与现网逐字一致）→ `cargo test -p xtask`。
2. 逐命令迁移 11 处解析循环，**一次一个命令、一次一测**：`perf-baseline` → `perf-measure` → `compliance-report` → 其余。每步 `cargo test -p xtask && cargo run -p xtask -- qa <cmd> --help-equivalent-smoke`。
3. SignedReport 同法：先锁定 compliance 现有输出字节（写一个 golden 测试），再收口三处。
4. `cargo clippy -p xtask --all-targets -- -D warnings` + `cargo fmt --check`。

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
cargo test -p xtask                            # B
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

### 11.3 候选 A 落地口径（R-A1/R-A2/R-A3 逐条对账）

淘汰语义逐点对照旧实现：`while>16`（epochs）→ `admit` 内私有淘汰、
`push`+`>16 remove(0)`（released/done_logged）→ `evict_bounded` FIFO、
`<16` 才推入（pending）→ `queue_pending` 闸门、`truncate(16)` →
`end_retry_cycle` 截断点（本 pass 新失败在前、重试仍失败在后的顺序保持）。
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

为何"今日无真实输入"不是豁免理由：`UART_HANDOVER_TABLE`（955 起）当前为空是
**真机 bring-up 的 owner 填空项**（表注："真机 bring-up = owner 在此登记板级非
console pl011 的 reg 基址……届时 C1 host 全链在板上复跑即为真机验收"）。E 落地
= 把"第一个登记 UART 的板子"变成 X7 丢失的引爆点，且 svc-storage 的
`boot_dma_vaddr`（X7）与 UART 的 X7 争夺将变成设计期问题而非编译期问题——
违本蓝图 R-DE1 自身口径。

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

> **复验声明**：本蓝图全部文件/行号/计数由蓝图作者在基线 `60b73a8` 上直接 grep/read 核实；与原报告的差异均已订正（淘汰 6 处非 3 处、14 参非 15 参、8 元组非 6 元组、16 字段非 13 字段、invalid 84 调用且 helper 已单点、held_pin_failures 零读者）。
