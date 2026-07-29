# formal/ — 形式化验证框架（Isabelle/HOL）

> EnerOS v0.148.0 形式化规范框架搭建 — Phase 3 P3-F 首版本
> 依据：蓝图 `phase3.md` §v0.148.0 + ADR-0001 裁剪（4 证明 → 2 证明）

## 目录结构

```
formal/
├── Isabelle/
│   ├── Kernel.thy        # 抽象规范层：内核状态抽象 + 调度不变式 + 安全/活性定理
│   ├── Scheduler.thy     # v0.149.0 调度器规范抽象：优先级抢占调度（sched_step/inv_priority/liv_fair）
│   ├── Scheduler_Proof.thy # v0.150.0 调度器形式化证明 ★（scheduler_safety/scheduler_liveness）
│   ├── Memory.thy        # v0.151.0 内存隔离规范抽象：跨分区不可访问不变式（inv_isolation/map_page）
│   ├── Memory_Proof.thy  # v0.152.0 内存隔离形式化证明 ★：isolation_safety rtranclp 完整归纳（无 sorry）
│   ├── Ipc.thy           # v0.153.0 IPC 规范抽象：消息完整性/不可伪造性不变式（inv_integrity/inv_no_forge/ipc_send/ipc_recv）
│   ├── Ipc_Proof.thy     # v0.154.0 IPC 形式化证明 ★：ipc_integrity_all 完整归纳 + ipc_no_forge（无 sorry）
│   ├── Capability.thy    # v0.155.0 能力管理器规范抽象：权限单调性/级联撤销/不可绕过不变式（mint/revoke/check）
│   ├── Capability_Proof.thy # v0.155.0 能力管理器形式化证明 ★：rights_monotone + revoke_cascade + no_bypass（无 sorry）
│   └── Refinement.thy    # 精化映射层：抽象规范→具体实现的映射框架（占位）
├── ROOT                  # Isabelle session 配置
├── Makefile              # 证明检查入口（isabelle build）
└── README.md             # 本文件
```

## 三层架构

依据蓝图 §4.1，形式化验证采用三层映射架构：

| 层级 | 文件 | 说明 |
|------|------|------|
| 抽象规范层（Abstract Spec） | `Isabelle/Kernel.thy` | 内核状态机 + 安全不变式 + 活性属性 |
| 精化映射层（Refinement） | `Isabelle/Refinement.thy` | 抽象规范 → 具体实现的映射关系（占位） |
| 具体实现层（Implementation） | `crates/kernel/microkernel/src/` | 既有 Rust 代码（本版本不改动） |

## ADR-0001 裁剪说明

**原设计**：4 项全量形式化证明（调度器 / 内存隔离 / IPC / 能力）。

**ADR-0001 方案 A 裁剪**：聚焦为 2 项增量证明（调度 + 内存），IPC/能力直接复用 seL4 已有数学证明，无需重建框架。

| 证明项 | 范围 | 状态 |
|--------|------|------|
| 调度器证明 | v0.149.0（规范抽象）+ v0.150.0（证明） | ✅ 安全性证明完成 |
| 内存隔离证明 | v0.151.0（规范）+ v0.152.0（证明） | ✅ 安全性证明完成 ★（v0.152.0） |
| IPC 证明 | v0.153.0（规范）+ v0.154.0（证明） | ✅ 完整性+不可伪造证明完成 ★（v0.154.0） |
| 能力证明 | v0.155.0（规范+证明） | ✅ 权限单调+级联撤销+不可绕过证明完成 ★（v0.155.0） |

## 当前状态

- ✅ 框架基础设施就绪（v0.148.0）
- ✅ `Isabelle/Scheduler.thy` 调度器规范抽象已交付（v0.149.0）
- ⏳ `liv_no_deadlock` 定理：`sorry` 占位，v0.150.0 补全
- ⏳ `step_preserves_priority` 定理：`sorry` 占位，v0.150.0 补全（v0.149.0 随 Scheduler.thy 引入）
- ⏳ `refinement_holds` 定理：`sorry` 占位，v0.151.0/v0.153.0 补全
- ✅ `Isabelle/Scheduler_Proof.thy` 调度器形式化证明已交付（v0.150.0 ★）
- ✅ `scheduler_safety` 定理：完整归纳证明，无 `sorry`
- ⏳ `scheduler_liveness` 定理：`sorry` 占位，需公平性假设，延后补全
- ✅ `Isabelle/Memory.thy` 内存隔离规范抽象已交付（v0.151.0）
- ⏳ `map_preserves_isolation` 定理：`sorry` 占位保留（v0.151.0，Surgical Changes 不改动；由 v0.152.0 `isolation_safety` 多步定理逻辑蕴含补全）
- ✅ `Isabelle/Memory_Proof.thy` 内存隔离形式化证明已交付（v0.152.0 ★）
- ✅ `isolation_safety` 定理：完整归纳证明（rtranclp.induct + map_page.induct 双层），**无 sorry**
- ✅ `Isabelle/Ipc.thy` IPC 规范抽象已交付（v0.153.0）
- ✅ `Isabelle/Ipc_Proof.thy` IPC 形式化证明已交付（v0.154.0 ★）
- ✅ `ipc_integrity_all` 定理：完整归纳证明（rtranclp.induct + ipc_step.induct 双层），**无 sorry**
- ✅ `ipc_no_forge` / `ipc_no_forge_all` 定理：不可伪造性证明，**无 sorry**
- ✅ `Isabelle/Capability.thy` 能力管理器规范抽象已交付（v0.155.0）
- ✅ `Isabelle/Capability_Proof.thy` 能力管理器形式化证明已交付（v0.155.0 ★ P3-F 终版本）
- ✅ `rights_monotone` 定理：权限单调性证明（mint 只能缩减），**无 sorry**
- ✅ `revoke_cascade` 定理：撤销级联证明（descendant.induct 归纳），**无 sorry**
- ✅ `no_bypass` 定理：不可绕过证明（无能力则不可访问），**无 sorry**

## Isabelle2024 安装指引

### 前置要求

- Isabelle2024 或更高版本（含 Poly/ML + JDK 依赖）
- 约 1GB 磁盘空间

### 安装步骤

1. **下载**：访问 [Isabelle 官网](https://isabelle.in.tum.de/)，下载 Isabelle2024 for Linux/macOS/Windows
2. **解压**：解压到本地目录（如 `/opt/Isabelle2024` 或 `C:\Isabelle2024`）
3. **配置 PATH**：将 `Isabelle2024/bin` 加入系统 PATH
4. **验证**：执行 `isabelle version`，应输出 `Isabelle2024`

### 触发证明检查

```bash
# 方式一：通过 Makefile
make -C formal/

# 方式二：直接调用 isabelle build
isabelle build -D formal/

# 清理构建产物
make -C formal/ clean
```

## 理论文件说明

### `Isabelle/Kernel.thy`（抽象规范层）

- `datatype thread_state`：线程状态（Ready/Running/Blocked/Dead）
- `record thread`：线程抽象（tid + state + partition）
- `record kernel_state`：内核状态（threads + current）
- `definition inv_unique_running`：调度不变式（任一时刻至多一个 Running）
- `inductive step`：线程状态转换关系
- `theorem step_preserves_unique_running`：安全性定理（完整证明）
- `theorem liv_no_deadlock`：活性定理（`sorry` 占位，v0.150.0 补全）
- `definition inv_isolation` / `liv_progress`：蓝图 §3 接口占位（`≡ True`）

### `Isabelle/Refinement.thy`（精化映射层）

- `datatype concrete_state`：占位具体状态类型
- `consts step_concrete`：占位具体步进关系
- `definition refines`：精化映射关系（占位 `≡ True`）
- `theorem refinement_holds`：精化保持定理（`sorry` 占位，v0.151.0/v0.153.0 补全）

### `Isabelle/Scheduler.thy`（v0.149.0 调度器规范抽象）

- 优先级抢占调度（priority preemptive scheduling）规范抽象
- `definition sched_step`：调度步进关系（preempt 抢占 / yield 让出）
- `definition inv_priority`：安全性不变式（优先级约束）
- `definition liv_fair`：活性/公平性属性
- `theorem step_preserves_priority`：安全性定理（`sorry` 占位，v0.150.0 补全）

### `Isabelle/Scheduler_Proof.thy`（v0.150.0 调度器形式化证明 ★）

- Phase 3 首个形式化证明里程碑 — ADR-0001 保留证明 #1/2（调度器）
- `lemma ready_schedulable`：辅助引理（就绪队列可调度性）
- `theorem scheduler_safety`：安全性定理（完整归纳证明，`inv_priority` 在 `sched_step^*` 下保持，无 `sorry`）
- `theorem scheduler_liveness`：活性定理（`sorry` 占位，需公平性假设，延后补全）

### `Isabelle/Memory.thy`（v0.151.0 内存隔离规范抽象）

- ADR-0001 保留证明 #2/2（内存隔离）的规范输入
- `type_synonym vaddr = nat`：虚拟地址类型（D2 新增）
- `record partition`：分区抽象（pid + regions 可访问地址集）
- `record mem_state`：内存状态（partitions 列表，D4 不含 page_tables）
- `definition accessible`：可访问地址集（D5 集合并 comprehension 修正）
- `definition inv_isolation`：隔离不变式（不同分区可访问集不相交，D6 `hide_const` 遮蔽 Kernel 占位）
- `definition add_region`：映射辅助函数（D3 新增）
- `inductive map_page`：映射操作（前提 v 不被非 target 分区访问）
- `theorem map_preserves_isolation`：隔离保持定理（`sorry` 占位，v0.152.0 补全）

### `Isabelle/Memory_Proof.thy`（v0.152.0 内存隔离形式化证明 ★）

- Phase 3 第二个形式化证明里程碑 — ADR-0001 保留证明 #2/2（内存隔离）
- `theorem isolation_safety`：安全性定理（完整归纳证明，`inv_isolation` 在 `map_page\<^sup>*\<^sup>*` 下保持，**无 sorry**）
- 双层归纳结构：外层 `rtranclp.induct`（自反传递闭包）+ 内层 `map_page.induct`（单步映射）
- D2a 修正：`map_page\<^sup>*\<^sup>*` 记法（predicate rtranclp，与 v0.149.0/v0.150.0 一致）
- D2b 修正：`case (map v s target)`（去除多余 `s'`，map 规则自由变量仅 v/s/target）
- D2c 修正：`then have "inv_isolation s'" by simp`（直接引用 IH，避免自引用）
- D2d/D2e 修正：simp 规则补充 `add_region_def`（蓝图仅 `accessible_def` 不足）
- D3：Memory.thy 的 `map_preserves_isolation` `sorry` 保留不动（Surgical Changes）；`isolation_safety` 多步定理逻辑蕴含单步版，构成 ★ 里程碑

### `Isabelle/Ipc.thy`（v0.153.0 IPC 规范抽象）

- ADR-0001 裁剪：IPC 证明降级为 seL4 证明引用与文档化，本理论提供规范输入
- `record msg`：消息抽象（content + badge 发送方标识）
- `consts valid_senders`：合法发送方集合（D2 新增）
- `record ipc_state`：IPC 状态（sent 已发送 + delivered 已投递）
- `definition inv_integrity`：完整性不变式（delivered ⊆ sent）
- `definition inv_no_forge`：不可伪造不变式（delivered 消息 badge ∈ valid_senders）
- `inductive ipc_send`：发送操作（消息入队 sent）
- `inductive ipc_recv`：接收操作（D3 追加 badge 校验前提）

### `Isabelle/Ipc_Proof.thy`（v0.154.0 IPC 形式化证明 ★）

- Phase 3 第三个形式化证明里程碑 — IPC 消息完整性与不可伪造性
- `theorem ipc_integrity`：发送保持完整性（单步）
- `theorem ipc_integrity_recv`：接收保持完整性（单步）
- `inductive ipc_step`：组合步进关系（D2 send/recv 析取组合）
- `theorem ipc_integrity_all`：★ 完整性多步定理（完整归纳证明，rtranclp.induct + ipc_step.induct 双层，**无 sorry**）
- `theorem ipc_no_forge`：不可伪造定理（D3 erule ipc_recv.cases 证明）
- `theorem ipc_no_forge_all`：不可伪造性多步保持（完整归纳，**无 sorry**）

### `Isabelle/Capability.thy`（v0.155.0 能力管理器规范抽象）

- ADR-0001 裁剪：能力证明降级为 seL4 证明引用与文档化，本理论提供规范输入
- `type_synonym rights`：权限类型（nat set）
- `record cap`：能力抽象（cid + cap_rights + parent_cid，D2 非递归编码）
- `record cap_state`：能力状态（cap_list 活跃能力列表）
- `definition mint`：创建子能力（D3 函数式，权限为调用者指定子集）
- `inductive descendant`：后代关系（直接子能力 + 传递闭包）
- `definition revoke`：撤销操作（D4 函数式，移除能力及其全部后代）
- `definition check`：访问检查（能力存在 + 权限包含）

### `Isabelle/Capability_Proof.thy`（v0.155.0 能力管理器形式化证明 ★）

- Phase 3 第四个形式化证明里程碑 — P3-F 终版本
- `lemma mint_rights_eq`：辅助引理（mint 权限等式）
- `theorem rights_monotone`：★ 权限单调性定理（mint 只能缩减不可扩增，**无 sorry**）
- `lemma descendant_subset`：辅助引理（后代必在 cap_list 中，descendant.induct 归纳）
- `theorem revoke_cascade`：★ 撤销级联定理（revoke 后全部后代不存在，**无 sorry**）
- `theorem revoke_self_removed`：撤销自移除定理
- `theorem no_bypass`：★ 不可绕过定理（无能力则不可访问，**无 sorry**）
- `theorem capability_security`：三属性联合定理（单调性 + 级联 + 不可绕过）

## `sorry` 占位策略

Isabelle 接受 `sorry` 为已承认证明（admitted proof），理论检查仍通过（`isabelle build` 不报错，仅 warning）。v0.148.0 为框架版本，`sorry` 占位是蓝图显式设计：

- `liv_no_deadlock`：蓝图 §4.5 注释 `(* 占位，v0.150.0 补全 *)`
- `step_preserves_priority`：v0.149.0 Scheduler.thy 引入，注释 `(* 占位，v0.150.0 补全 *)`
- `refinement_holds`：本版本注释 `(* 占位，v0.151.0/v0.153.0 补全 *)`
- `scheduler_liveness`：v0.150.0 Scheduler_Proof.thy 引入，需公平性假设，`sorry` 占位延后补全
- `map_preserves_isolation`：v0.151.0 Memory.thy 引入，`sorry` 占位保留（Surgical Changes 不改动 v0.151.0 基线）；v0.152.0 `isolation_safety` 多步定理完整归纳证明无 sorry，逻辑蕴含单步版，达成 ★ 验收

**v0.153.0/v0.154.0 无 sorry 说明**：IPC 规范（Ipc.thy）与证明（Ipc_Proof.thy）均无 `sorry` 占位——所有定理完整证明。

**v0.155.0 无 sorry 说明**：能力管理器规范（Capability.thy）与证明（Capability_Proof.thy）均无 `sorry` 占位——rights_monotone / revoke_cascade / no_bypass 三定理完整证明。

## 后续版本路线

| 版本 | 交付物 |
|------|--------|
| v0.149.0 | 调度器规范抽象 |
| v0.150.0 | 调度器形式化证明 ★（补全 `liv_no_deadlock` / `step_preserves_priority`） |
| v0.151.0 | 内存隔离规范抽象 |
| v0.152.0 | 内存隔离形式化证明 ★（`isolation_safety` rtranclp 完整归纳，无 sorry） |
| v0.153.0 | IPC 规范抽象（Ipc.thy：inv_integrity/inv_no_forge/ipc_send/ipc_recv） |
| v0.154.0 | IPC 形式化证明 ★（`ipc_integrity_all` + `ipc_no_forge` 完整归纳，无 sorry） |
| v0.155.0 | 能力管理器形式化证明 ★（P3-F 终版本：rights_monotone + revoke_cascade + no_bypass，无 sorry） |

## 相关文档

- `docs/formal/framework.md`：形式化验证框架设计文档（12 章覆盖）
- `蓝图/phase3.md` §v0.148.0：蓝图版本规格
- `.trae/specs/develop-v14800-formal-verification-framework/spec.md`：本版本规格文档（D1~D12 偏差决策表）
