# EnerOS v0.142.0 能力管理器设计文档（capability / CSpace / 授权链派生树 / 四错误判定链）

> 版本：v0.142.0
> 适用范围：EnerOS 自研内核能力子系统（能力令牌 + 每分区能力表 CapSpace + 能力管理器授权/转移/撤销/校验；host 单测/集成测试 + QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` 冒烟 CNTVCT 实测）
> 蓝图依据：`蓝图/phase3.md` §v0.142.0（§1 版本目标 / §3 交付物 / §4 详细设计（§4.1 数据结构 / §4.2 授权链 / §4.3 mermaid 判定链 / §4.4 错误语义 / §4.5 伪代码）/ §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项（§8.4 seL4 对照）/ §9 多角度要求）
> Phase 定位：**P3-D IPC 与能力线终版本**（v0.139.0 同步 IPC → v0.140.0 notification → v0.141.0 共享内存 → v0.142.0 能力管理器四版之四），内核成为**能力授予与校验的唯一权威**；为 **v0.148.0 形式化验证提供能力模型**，以「越权访问被拒」出口判定解锁 **P3-E 可启动**（蓝图 §1/§7.5）
> 配套文档：[ipc_design.md](./ipc_design.md)（v0.139.0 同步 IPC 设计，Endpoint 目标资源来源）、[notification_design.md](./notification_design.md)（v0.140.0 通知机制设计，Notification 目标资源来源）、[shm_design.md](./shm_design.md)（v0.141.0 共享内存设计，ShmCap 授权表过渡形态与统一 CSpace 接管预告来源）、[sel4_compat.md](./sel4_compat.md)（seL4 兼容层口径）、[../boot/qemu_run.md](../boot/qemu_run.md) §21（v0.142.0 QEMU 实测记录：首轮一次 PASS，无阈值调整）

---

## 1. 版本目标

**核心一句话**：实现 capability-based 访问控制，内核成为能力授予与校验的唯一权威（蓝图 §v0.142.0 核心一句话原样）。

- **业务价值**：每个 Agent/线程的能力令牌定义其能做什么，是安全隔离的核心机制（蓝图 §1 原样）。前三版交付了 IPC 通道（v0.139.0）、事件通知（v0.140.0）、共享资源（v0.141.0）三类受保护对象，本版本为其补上统一的安全底座——能力令牌化的「谁能对哪个对象做什么」判定权威。
- **下游衔接**：**为 v0.148.0 形式化验证提供能力模型**（蓝图 §1 Phase 定位原样）；「自研内核安全访问控制功能完整性」出口关联 **v0.155.0 形式化证明目标**；SM2 签名接入为 **v0.158.0**（蓝图 §3 注释原样，见 §7/§13 D12）；能力拦截接入 IPC/SHM 调用路径的统一接线属后续集成版本（本版本以 CPI7 语义等价证据衔接，见 §14）。
- **ADR-0001 裁剪口径**（蓝图 §1 引用段原样口径）：本版本原为自研内核组件，已按方案 A 调整为 seL4 深度定制/信创认证路径。seL4 本身就是 capability-based 微内核，方案 A 下直接复用 seL4 CSpace 能力模型，无需重写；**本内核按同语义自研实现**（蓝图 §8.4「与 seL4 capability 语义一致」），保持语义同构以降低 v0.156.0 迁移成本；差异点（parent 反向链接替代 CDT 正向表、定长数组替代 HashMap/Vec）属 D2/D3 决策，见 §5/§13。
- **依赖关系**：前序 v0.141.0 共享内存可用（蓝图 §2 前序版本原样）；蓝图 §2 假设「继承 seL4 capability 模型」落地为本版本语义同构设计；蓝图 §2 阻塞项「能力校验失败阻塞迁移」由本版本越权拒绝三路径（CPI1）+ 出口判定（§7.5）解除。类型引用既有 `ipc::{EndpointId, NotificationId, ShmId}` / `sched::Tid` / `mem::partition::PartitionId`（CapTarget 五变体来源）。

## 2. 交付物清单

对照蓝图 §3 逐项标注偏差（D 编号与 [src/cap/mod.rs](../../crates/kernel/microkernel/src/cap/mod.rs) 头注 D1~D12 一一对应，全局表见 §13）：

| 蓝图 §3 交付物 | 实际落位 | 偏差 |
|---------------|---------|------|
| 代码：`kernel/src/cap/token.rs`（能力令牌） | [crates/kernel/microkernel/src/cap/token.rs](../../crates/kernel/microkernel/src/cap/token.rs)（CapSlot / CapId / CapRights / CapType / CapTarget / ConstraintPack / Capability / MAX_CAPS_PER_PARTITION） | D1 路径归口；D3 parent 第九字段；D4 手写位字；D11/D12 约束与签名口径 |
| 代码：`kernel/src/cap/table.rs`（能力表） | [crates/kernel/microkernel/src/cap/table.rs](../../crates/kernel/microkernel/src/cap/table.rs)（CapSpace + new/get/set/clear/find_free_slot/children_of + ChildrenIter） | D1 路径归口；D2 定长化；D3 无 children 数组 |
| 代码：`kernel/src/cap/manager.rs`（能力管理器） | [crates/kernel/microkernel/src/cap/manager.rs](../../crates/kernel/microkernel/src/cap/manager.rs)（CapabilityManager trait 八方法 / CapabilityManagerImpl / CapStats 九计数） | D1 路径归口；D2/D5/D9/D10；check `&mut self` 落地修正 |
| 代码：模块挂载与公开导出 | [crates/kernel/microkernel/src/cap/mod.rs](../../crates/kernel/microkernel/src/cap/mod.rs)（`pub mod manager/table/token` + `pub use` 导出 + D1~D12 头注 + Task 3 落地修正五条汇总 + BSS 预算声明） | 无（蓝图未单列 mod.rs） |
| 代码：KernelError 三错误落码（§4.4） | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs)：`InvalidCapability = 3` 既有复用（v0.127.0 起语义一致）+ `ConstraintViolation = 11` / `CapExpired = 12` 追加；判别值 10 留保留空洞；既有 1~9 零改动 | D8（同名变体不可重复定义的编译约束 + 追加规则） |
| 接口：`CapSlot`/`CapId`/`Capability`/`CapType`/`CapRights`/`CapTarget`/`ConstraintPack`/trait | 蓝图 §3 原样导出（`CapSlot = u32` / `CapId = u64`；Capability 八字段 + D3 parent 第九字段；CapRights 五位值原样；trait 六方法含 pid 首参 + D9 补 create_space/destroy_space） | D2/D3/D4/D9/D12；check `&mut self` |
| 文档：`docs/kernel/capability_design.md` | 本文档 | 无（路径同蓝图） |
| 测试：`kernel/tests/cap_test.rs`（越权访问被拒 + 能力传递与撤销） | [crates/kernel/microkernel/tests/cap_test.rs](../../crates/kernel/microkernel/tests/cap_test.rs)（CPI1~CPI9；CPI9 `#[ignore]` 故障注入） | D1 路径归口 |
| 测试：lib 内嵌单测 | CAP1~CAP4（token.rs）/ CT1~CT5（table.rs）/ CM1~CM12（manager.rs）（新增 21 个） | 蓝图 §6.1 单元测试落点 |
| 测试：QEMU 冒烟 | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `cap_smoke` 段（§3.16：create_space → 根能力种子 → mint 缩减/越权拒绝 → check 双路径 → derive 窗内/窗外 → 过期 → revoke 级联 → 100 轮 check CNTVCT 夹取） | 蓝图 §6.2 集成测试的目标机落点；D-C1 有效化口径 |
| 配置：MAX_CAPS_PER_PARTITION=4096 | [token.rs](../../crates/kernel/microkernel/src/cap/token.rs) `MAX_CAPS_PER_PARTITION = 4096` | 无（蓝图原样） |
| 回归断言：QEMU 关键串 | [tests/boot_test.sh](../../tests/boot_test.sh) 第十五条 `grep -q "\[cap\] ok"`（既有十四条原样） | 无（surgical 追加同 v0.140.0/v0.141.0 先例） |

**接口一览**（蓝图 §3 接口类签名 + D2~D12 修正，[cap/mod.rs](../../crates/kernel/microkernel/src/cap/mod.rs) 公开导出）：

| 项 | 定义 | 说明 |
|----|------|------|
| `CapSlot` | `pub type CapSlot = u32` | 蓝图 §3 原样；取值范围 0..4096（= 分区能力表内槽位下标） |
| `CapId` | `pub type CapId = u64` | 蓝图 §3 原样；mint/copy/derive 时管理器 saturating 自增分配，首分配 id=1（0 留调试视觉哨兵） |
| `Capability` | struct 九字段 | `id / cap_type / rights / target / constraints: Option<ConstraintPack> / issued_at / expires_at / signature: [u8; 64] / parent: Option<CapSlot>`（蓝图八字段 + D3 parent 反向链接；D12 signature 恒零） |
| `CapType` | enum 七变体 | `Endpoint / Notification / Shm / Thread / VSpace / Irq / Device`（蓝图 §3 原样） |
| `CapRights` | `pub struct CapRights(pub u32)` | 位值 READ=1/WRITE=2/INVOKE=4/GRANT=8/DERIVE=16 原样；D4 手写 const + `contains`/`is_superset_of`（替代 bitflags! 宏，零新依赖） |
| `CapTarget` | enum 五变体 | `Endpoint(EndpointId) / Notification(NotificationId) / Shm(ShmId) / Thread(Tid) / Partition(PartitionId)`（蓝图 §3 原样，引用既有子系统标识类型） |
| `ConstraintPack` | struct 两字段 | `max_power: f32 / time_window: (u64, u64)`（蓝图 §3 原样；D11：time_window 判定落地、max_power 仅存储不判定） |
| `CapSpace` | struct 单字段 | `pub slots: [Option<Capability>; MAX_CAPS_PER_PARTITION]`（D2 定长 4096 槽；D3 无 children 数组；pub 便于审计遍历） |
| `ChildrenIter` | 迭代器 | `children_of(parent)` 全表扫描 parent 链接惰性产出子槽位（D3；revoke 级联与审计用，按槽位升序） |
| `CapabilityManager` trait | 八方法 | `create_space / destroy_space`（D9 补充）+ `mint / move_cap / copy_cap / revoke / check / derive`（蓝图 §3 六方法，签名含 pid 首参）；check 为 `&mut self`（§9.6 计数与 plain 字段互斥之落地修正） |
| `CapabilityManagerImpl` | struct 四字段 | `pub spaces: [Option<CapSpace>; 16] / next_cap_id: u64（私有）/ now: fn() -> u64（私有，D5 注入）/ pub stats: CapStats`（D2 定长 + D5 时钟注入；AtomicU64 口径留待 SMP 集成版本） |
| `CapStats` | struct 九字段 | `mints / moves / copies / revokes / derives / checks / denied / expired_violations / constraint_violations`（§9.6 审计；全部 saturating_add 饱和累加，同 ShmStats 先例） |
| `MAX_CAPS_PER_PARTITION` | `= 4096` | 蓝图 §3 配置类原样；槽满 → `OutOfMemory` |

## 3. 能力模型选型（蓝图 §5.1）

蓝图 §5.1 三维对照表原样口径 + 本版本落地注解：

| 维度 | capability（本实现） | ACL | RBAC | 选择 |
|------|---------------------|-----|------|------|
| 粒度 | **对象级**（CapTarget 精确到单个 Endpoint/Notification/Shm/Thread/Partition 实例） | 列表级（每对象一张访问列表） | 角色级（权限挂角色而非对象） | capability |
| 委托 | **原生 mint**（持权者直接派生弱化子能力转授，内核无需预置全局表项） | 难（需修改对象 ACL） | 难（需变更角色成员） | capability |
| 撤销 | **revoke 级联**（沿派生边撤掉整棵子树，O(受影响子树) 语义） | 需遍历（找全部引用） | 需遍历（找全部会话） | capability |

**选型理由**（蓝图 §5.1/§5.2 + §8.4）：令牌化权限 = **最小授权 + 可委托 + 可撤销** 三性质合一——能力即「对象引用 + 权限位 + 约束」的不可伪造令牌，持有即有权、无令牌即无权（§7.3 权限不可绕过的模型基础）；mint 子集规则保证委托路径上权限单调不增；revoke 级联保证撤销路径上无悬挂授权。ACL/RBAC 的「按身份查表」模型把权限判定分散到每个对象的列表/角色成员关系上，与本内核「分区隔离 + 内核唯一权威」的安全底座（蓝图 §1）不匹配；seL4 同为 capability-based（蓝图 §8.4/ADR-0001），选型同构直接降低 v0.156.0 迁移成本。实现路径（蓝图 §5.3 原样）：令牌结构 → mint → check → revoke → derive。

## 4. 令牌结构（Capability 九字段）

### 4.1 九字段逐项（蓝图 §3 八字段 + D3 parent）

| 字段 | 类型 | 语义 | 决策/口径 |
|------|------|------|----------|
| `id` | `CapId = u64` | 能力全局标识（管理器 saturating 自增分配，首分配 id=1） | 0 留调试「无效 id」视觉哨兵，无强制语义；溢出饱和不回绕保唯一性 |
| `cap_type` | `CapType` | 能力对象类型（七变体） | mint/derive 继承父；蓝图 §3 原样 |
| `rights` | `CapRights` | 权限位字（R/W/INVOKE/GRANT/DERIVE 五位） | D4 手写；mint 按请（⊆ 父，D6）；derive 继承父不变 |
| `target` | `CapTarget` | 能力目标（五变体，指向既有 IPC/SHM/线程/分区对象） | mint 按请；derive 继承父不变（蓝图 §4.5「权限不变但增加约束」） |
| `constraints` | `Option<ConstraintPack>` | 附加约束包（None = 无约束） | mint 继承父；derive 置 `Some(新约束包)`；D11 判定口径 |
| `issued_at` | `u64`（ns） | 签发时刻 | mint/copy/derive 时 `now()`（D5 注入时钟） |
| `expires_at` | `u64`（ns） | 过期时刻 | **0 = 永不过期哨兵**；判定 `expires_at > 0 && now() > expires_at`（严格大于，now==expires_at 不过期，CM5 锚定）；mint/derive 继承父 |
| `signature` | `[u8; 64]` | SM2 签名占位 | **D12：本版本恒 `[0u8; 64]`**，v0.158.0 接入（蓝图 §3 注释原样） |
| `parent` | `Option<CapSlot>` | 父能力槽位（派生边） | **D3 反向链接**：Some = mint/derive 建边；None = 根能力或 copy 副本（D10） |

### 4.2 D3：parent 反向链接 vs 蓝图 §4.5 children 正向表

蓝图 §4.5 伪代码 `children: [Vec<CapSlot>; 4096]`（每槽正向子表）在 no_std 下不可编译（Vec 需堆），且即使定长化也不划算。BSS 开销核算对照：

| 方案 | 每分区表额外开销 | 级联遍历 | no_std | 结论 |
|------|-----------------|---------|--------|------|
| 蓝图 `children: [Vec<CapSlot>; 4096]` | Vec 头 4096×24B ≈ 96KB + 堆数据（不可编译） | O(子数) 直查 | ❌ Vec 无堆 | 否决 |
| 定长 children 数组（cap = 每槽子上限，如 4） | **4096 × cap × 4B**（cap=4 → 64KB/分区表，16 分区 ≈ 1MB） | O(cap) 扫描槽内数组 + 全表递归仍 O(n) | ✅ 但浪费 | 否决（绝大多数槽无子，空间利用率极低） |
| **D3 `Capability.parent: Option<CapSlot>` 反向链接** | **每能力 8B**（Option<u32> 对齐后 8B → 4096×8B = 32KB/分区表，已计入 Capability ≈152B 尺寸） | 全表扫描 O(4096)（`children_of` 迭代器） | ✅ 零堆 | **采纳** |

采纳理由（D3 原样口径）：revoke 为**低频操作**，蓝图 §8.1/§9.4 要求级联**正确性**而非性能；反向链接把派生关系存储从「每槽一张子表」压缩为「每能力一个指针」，且 revoke 级联本就需要全表一致性（防遗漏），全表扫描迭代在 O(1) 栈下完成（见 §5.3 终止性论证）。

### 4.3 D2 定长化与 D4 手写位字

- **D2 定长化**：蓝图 §4.1 `slots: Vec<Option<Capability>>` / `parent_map: HashMap` / `spaces: HashMap` → 全定长化（`spaces: [Option<CapSpace>; 16]` + `slots: [Option<Capability>; 4096]`；蓝图 §4.5 伪代码已部分定长，本决策完成剩余部分）。no_std 无堆（同 ipc D2/notify D9/shm D16 先例链）；`parent_map` 随 D3 反向链接整体取消。
- **D4 手写 CapRights**：蓝图 `bitflags!` 宏 → 手写 `CapRights` const + `contains`/`is_superset_of`（READ=1/WRITE=2/INVOKE=4/GRANT=8/DERIVE=16 位值原样）。零新依赖（cargo deny 无风险），同 MemAttrs 手写 attrs_to_bits 先例；位公式 `(a & b) == b` 统一表达「包含/超集」判定（CAP1 真值表锚定，含空权限 0 与全位 31 边界）。

### 4.4 D12：signature 恒零占位

`signature: [u8; 64]` 为 SM2 签名占位（蓝图 §3 注释「SM2 签名（v0.158.0 接入）」原样）。本版本恒 `[0u8; 64]`——字段先行定型保证九字段布局稳定（v0.158.0 接入时令牌结构零改动），签名校验与蓝图 §6.5「伪造能力令牌」故障注入**一并后置 v0.158.0**；本版本故障注入 = 过期能力（CPI6）+ 槽位耗尽（CPI9）。

## 5. 授权链与判定链

### 5.1 mint 子集规则（D6 位公式修正 + 派生树）

- **D6 位公式**：蓝图 §4.5 伪代码 `!parent_cap.rights.0 & rights.0 == rights.0` 为运算符优先级 bug（Rust 中不编译——`!` 按位取反优先级高于 `&`，且 `!u32` 结果非布尔）→ 修正为 `parent.rights.is_superset_of(requested)` 即 **`(parent.0 & requested.0) == requested.0`**，语义 = rights 只能缩减（蓝图 §4.2/§5.2 原样）。子集与相等成功且子能力 rights 如请；超集 → `PermissionDenied` + `stats.denied+1`，且**越权拒绝在 find_free_slot 之前判定——零副作用不占槽**（CM2/CPI3 双锚定，§7.3 权限不可绕过）。
- **派生树构建**：mint 成功 → 子能力 `cap_type/constraints/expires_at` 继承父、`id` 自增、`issued_at = now()`、`parent = Some(parent)` **建派生边**；derive 同理（权限/target 不变、附加约束包，蓝图 §4.5「权限不变但增加约束」，CM6 锚定）。仅 mint/derive 建边（D10，seL4 CDT 语义，见 §7）。
- **根能力来源**：全图无 mint 根 API——根能力（`parent = None`）由内核/集成测试经 **pub spaces 预置种子**（QEMU 冒烟槽 0 种子 id=0xFFFF 视觉哨兵与管理器自增 id 恒不冲突，R|W|INVOKE Endpoint(0)；同 ShmManager pub regions 先例）。

### 5.2 move/copy 槽位操作（D10 不建派生边）

| 操作 | 语义 | 派生边 | 计数 |
|------|------|--------|------|
| `move_cap(pid, src, dst)` | src 有效、dst 为空槽 → src 内容移至 dst、src 清空；本体 parent 指针随迁移；**既有子代 parent 指针 src→dst 改写**（保「parent 恒指向存活槽」不变式，revoke 级联正确性依赖之；迁移既有边非新建边，不违 D10） | 不建边 | `moves+1` |
| `copy_cap(pid, src, dst)` | src 有效、dst 为空槽 → dst 立副本（**新 id、`parent = None`**；其余字段含 issued_at/expires_at/constraints 原样复制） | 不建边（revoke 不级联到 copy，CM8 锚定） | `copies+1` |

错误路径（CM10 锚定）：src 无效/越界 → `InvalidCapability`；dst 越界或已占用（含 dst==src 自移）→ `InvalidArgument`；失败**零副作用**（源槽不动、统计不计）。move 后级联保持（CM9：children_of(dst) 命中、children_of(src) 空、revoke(dst) 连带仍正确）。

### 5.3 revoke 级联（D3 多轮全表扫描迭代）

蓝图 §4.2「revoke：撤销能力及其所有派生」+ §4.5 递归伪代码 → D3 反向链接下落地为**多轮全表扫描迭代**：

```text
revoke(pid, slot):
  槽校验（无效 → InvalidCapability）
  ① 先清根（slots[slot] = None）
  ② loop 全表扫描：清除全部 parent 已悬空者
     （dangling ⇔ cap.parent = Some(p) 且 slots[p] 已空）
     本轮无清除 → break
  ③ stats.revokes+1（仅顶层计 1，级联清除不重复计数）
```

- **终止性论证**：每轮迭代若未清除任何槽位则终止；否则本轮至少清除一个槽 → 被占槽位数严格单调递减、下界为 0 → **必然在有限轮内终止**（最坏 4096 轮，实际级联深度轮）。
- **循环防护**：清除动作使被清槽的 parent 信息随槽位一并丢失；即使存在异常 parent 环（a.parent=b ∧ b.parent=a），环上任一槽被清后环即瓦解，下一轮必清除其余环节点——不会死循环。
- **O(1) 栈**：无递归、无 16KB work 数组——蓝图 §8.1「revoke 级联的原子性」难点在单核口径下由「管理器临界区独占 + 迭代不中断」满足（SMP 原子化阶段再评估锁粒度）。
- **语义锚定**：三级链 祖父→父→子 revoke 中间级「父」→ 父与子槽位全清、祖父不动；再 revoke 祖父 → 仅清祖父（CM7/CPI2 双锚定，§9.4）；「parent 恒指向存活槽」不变式下 parent 悬空 ⇔ 本轮/前轮被撤之根的后代。

### 5.4 check 四段判定链（蓝图 §4.3 mermaid 落地）

蓝图 §4.3 mermaid 判定链原样语义 → 代码落地为**固定顺序四段**（manager.rs `check`，顺序不可调换——CM4 锚定「rights 不足且窗外 → 先 rights」「窗外且过期 → 先约束」）：

```text
check(pid, slot, required) -> Result<&Capability, KernelError>:
  ① 槽有效？  无效/越界 → InvalidCapability            （不计任何统计）
  ② rights 包含 required？  否 → PermissionDenied       （stats.denied+1）
  ③ 约束满足？  time_window 窗外 → ConstraintViolation  （stats.constraint_violations+1）
  ④ 过期？  expires_at > 0 && now() > expires_at → CapExpired （stats.expired_violations+1）
  通过 → stats.checks+1，返回 &Capability
```

每错误路径**仅累加对应计数，互不重复**（CM12 九计数精确核算锚定）；判定路径无锁无分配直线执行（§10 性能实测的路径基础）。蓝图 §4.3 mermaid 中 mint 侧「child.rights ⊆ parent.rights → 记录派生关系」一支对应 §5.1 D6 子集规则 + parent 反向链接建边。

## 6. 四错误语义（D7/D8）

### 6.1 错误码落位表

| 蓝图 §4.4 文字口径 | KernelError 落位 | 判别值 | 判定链段位 | 说明 |
|-------------------|-----------------|--------|-----------|------|
| 无效槽位 | `InvalidCapability`（**既有复用**） | **3** | ① 槽 | v0.127.0 起既有，语义即「Capability 引用不存在或已被撤销」，与 §4.4 一致（D8） |
| 权限不足 | `PermissionDenied`（既有） | 4 | ② rights | 能力存在但不包含所请求权限位；mint 超集同码 |
| 约束违反 | `ConstraintViolation`（**本版本追加**） | **11** | ③ 约束 | time_window 窗外（D7/D11） |
| 能力过期 | `CapExpired`（**本版本追加**） | **12** | ④ 过期 | `expires_at > 0 && now() > expires_at`（D7） |

### 6.2 D7：伪代码与 §4.4 文字口径矛盾的最小化解

蓝图 §4.5 伪代码「约束违反 → `PermissionDenied`、过期 → `InvalidCapability`」与 §4.4 文字口径（四错误一一对应）矛盾 → 采 **§4.4 文字口径**：约束违反 → `ConstraintViolation`、过期 → `CapExpired`。佐证：§4.3 mermaid 判定链四段与四错误一一对应（槽→`InvalidCapability` / rights→`PermissionDenied` / 约束→`ConstraintViolation` / 过期→`CapExpired`），判定链顺序与错误语义同源一致。

### 6.3 D8：InvalidCapability 复用与判别值 10 空洞

- **落地修正（Task 1 实测）**：spec 曾规划 `InvalidCapability = 10` 追加，但该语义（无效能力引用）已由 v0.127.0 既有 `InvalidCapability = 3` 覆盖；Rust 枚举**不可重复定义同名变体** → 复用不重复定义，仅追加 `ConstraintViolation = 11` / `CapExpired = 12`。
- **判别值 10 留作保留空洞不再分配**（[error.rs](../../crates/kernel/microkernel/src/error.rs) 头注记录；遵循「新增必须追加、只追加不重排」规则，既有 1~9 零改动）。
- ERR1 测试同步锚定 1~9 + 11~12 互异（CAP4 同锚 3 复用 / 11 / 12 判别值）。

## 7. seL4 CSpace/CDT 语义对照（蓝图 §8.4）

### 7.1 逐项对照表

| 维度 | seL4（ADR-0001 方案 A 参照系） | 本内核 v0.142.0 | 语义一致性 |
|------|------------------------------|----------------|-----------|
| 能力空间 | CSpace（每线程 CNode 树寻址） | **CapSpace**（每分区一张 4096 定长槽表，下标即 CapSlot） | 一致：分区粒度的能力持有容器；寻址形态差异（槽位下标 vs CNode radix 寻址）属 D2 定长化 |
| 派生关系 | CDT（Capability Derivation Tree，正向树边） | **`Capability.parent` 反向链接**（D3）+ `children_of` 全表扫描重建 | 一致：派生树语义等价（§9.5 派生树清晰）；存储形态差异（反向指针 vs 内核侧 CDT 节点） |
| mint 派生 | `seL4_CNode_Mint`（派生弱化 cap，**建 CDT 边**） | `mint`（rights ⊆ 父 D6，`parent = Some(parent)` 建边） | 一致：弱化派生 + 建边 |
| derive 附加约束 | seL4 无对应原语（本蓝图扩展：权限不变附加约束包） | `derive`（权限/target 不变 + `parent = Some(parent)` 建边） | 蓝图扩展项，建边口径与 mint 一致 |
| copy/move | `seL4_CNode_Copy` / `seL4_CNode_Move`（**不建 CDT 边**） | `copy_cap`（parent=None 新 id）/ `move_cap`（本体迁移 + 子代指针改写） | **一致（D10）：仅 mint/derive 建派生边，copy/move 不建边**——revoke 不级联到 copy |
| revoke | `seL4_CNode_Revoke`（撤 CDT 子树全部后代） | `revoke`（多轮全表扫描级联，先清根再逐轮清 parent 悬空者） | 一致：撤「该槽及其全部派生」；实现形态差异（CDT 直查 vs 全表扫描迭代，D3 低频口径） |
| check | seL4 内核态 syscall 入口能力查找 + rights 校验 | `check` 四段判定链（§5.4） | 一致：槽有效 → rights →（扩展）约束 → 过期；约束/过期两段为蓝图扩展（时间窗/时效语义） |
| 签名 | 无（seL4 能力不可伪造性由内核对象引用保证） | `signature` 占位恒零（D12），SM2 签名 v0.158.0 接入 | 蓝图扩展项（跨分区令牌可验证性），后置 |

### 7.2 语义一致性声明与衔接路线

- **声明**：能力模型（令牌 + 每分区 CapSpace + mint 子集规则 + 派生树 revoke 级联 + copy/move 不建边）与 seL4 capability 语义**同构**（蓝图 §8.4 兼容口径原样），降低 v0.156.0 迁移成本；与 ADR-0001 的衔接：方案 A 下不重写 seL4 CSpace，自研内核侧同构实现即「复用 seL4 CSpace 能力模型」的落地形态。
- **差异点①（D2/D3 存储形态）**：seL4 CNode 树 + CDT 正向边 vs 本实现定长槽表 + parent 反向链接——no_std 无堆约束下的等价语义落地，revoke 级联结果一致（CM7/CPI2 锚定）。
- **差异点②（蓝图扩展项）**：derive 约束包（time_window/max_power）、expires_at 时效、signature 签名三者为蓝图在 seL4 模型上的能源/时效/可验证性扩展，不改变 mint/revoke 核心语义同构性。
- **形式化证明衔接路线**（蓝图 §1/§5.5/§9.7）：本版本能力模型 → **v0.148.0 形式化规范框架**（Isabelle/HOL 或 Lean）→ **v0.155.0 形式化验证**（能力模型安全性证明目标，蓝图 §1 出口关联）→ **v0.158.0 SM2 签名接入**（signature 字段激活 + 伪造令牌故障注入，D12 后置声明）。判定链四段固定顺序（§5.4）与九计数审计（§9.6）为后续规范抽象提供确定性语义锚点。

## 8. 时钟注入与约束语义（D5/D11）

### 8.1 D5 构造注入时钟

- **偏差动机**：蓝图 §4.5 伪代码 `crate::time::now()` 在全图**不存在**（time 子系统无此函数）→ 管理器构造注入 `now: fn() -> u64`（ns 口径），可测性口径同既有注入式先例（D10 调度器注入等）。
- **三侧接线**：aarch64 = `cap_now()` = CNTVCT 经 `cycles_to_ns(read_cycle_counter(), read_counter_freq())` 换算（复用 v0.138.0 wcet 设施原样，不重复定义）；host 单测/集成 = `std::thread_local!` Cell<u64> mock 时钟（cargo test 多线程并发各测试时间轴互不干扰）；QEMU 冒烟 = 同 aarch64 CNTVCT 路径。
- **消费点**：`issued_at = now()`（mint/copy/derive 签发时刻）；check ③ 时间窗判定与 ④ 过期判定（单次调用读取一次 `now`，四段判定共用同一时刻样本）。

### 8.2 time_window 判定（D11 落地）

- **判定公式**（check ③ 段）：`constraints = Some(pack)` 且 `now < pack.time_window.0 || now > pack.time_window.1` → `ConstraintViolation` + `stats.constraint_violations+1`；**闭区间端点通过**（now == start / now == end 均窗内，CPI5 含端点锚定）。
- **约束继承链**：mint 继承父 constraints（子能力约束不放宽）；derive 置 `Some(新约束包)`（蓝图 §4.5「权限不变但增加约束」——时间窗收窄即生效，CM6 窗内通过/窗外拒绝锚定）。
- **max_power 仅存储不判定**（D11）：蓝图 §4.3 判定链只列时间窗；功耗约束判定属后续能源建模层；no_std f32 可存储不计算（CPI5 锚定 `max_power = 999.9` 不触发任何拒绝）。

### 8.3 expires_at 过期语义

- **0 哨兵永不过期**（蓝图伪代码 `expires_at > 0` 口径原样）：expires_at = 0 的能力在 now = u64::MAX 仍通过（CM5 锚定）。
- **严格大于口径**：`expires_at > 0 && now() > expires_at` → `CapExpired`；now == expires_at 边界**不过期**（CM5 锚定），过后即拒（CPI6 mock 时钟推进 expires_at=1000 过后命中）。

## 9. 内存预算（仓库 §5.6 合规声明）

### 9.1 BSS 核算（NOBITS 口径，同 IpcManager ≈1.6MB / ShmManager ≈94KB 先例链）

| 项 | 核算 | 尺寸 |
|----|------|------|
| `Capability` | id 8 + cap_type 1 + rights 4 + target 16 + constraints 32（Option<ConstraintPack> 含 f32+(u64,u64)）+ issued_at 8 + expires_at 8 + signature 64 + parent 8 + 对齐 padding | **≈152B**（spec D2 核算口径；mod.rs 头注 ≈160B 为含 padding 保守口径，两口径同量级、结论一致） |
| `CapSpace` | 4096 槽 × ≈152B | **≈623KB**（D-C1 汇编实证口径：create_space 内 623KB `CapSpace::new()` 经 memcpy 消除目的地直写） |
| `CapabilityManagerImpl` | 16 分区 × 623KB + next_cap_id 8 + now 8 + stats 72 + Option 标签 | **≈10MB BSS（NOBITS 零初始化段，不占 ELF 体积）** |

- **NOBITS 实证**：QEMU 侧 `CAP_MANAGER: SpinLock<MaybeUninit<CapabilityManagerImpl>>` 经 `MaybeUninit::zeroed()` 全零初值落 BSS；release ELF **148,104B（≈144.6KB）< 512KB 预算**（双镜像 bootloader 70,440B / kernel 148,104B，qemu_run.md §21 实测）——≈10MB 驻留区零初始化不占镜像，D2 预算口径成立。
- **QEMU virt RAM 256MB 可行**（spec D2 记录）；能力表全驻 BSS，PMM 零消耗（冒烟与既有十四类输出正交）。
- **host 测试手法**：host 默认测试线程 2MB 栈无法容纳 `Box::new` 构造期栈临时 → 用例统一经 **64MB 大栈线程 + Box 持有**（manager.rs `with_big_stack` / cap_test.rs `run_big_stack`，ShmManager 32MB 方案之放大版）。

### 9.2 真机优化路线（列入后续版本）

- **按分区懒实例化**：16 分区全量静态实例化 → 按需激活（当前 spaces 数组元素 `Option<CapSpace>` 已具形态，真机可将 CapSpace 移出管理器内联、改为分区配额内页分配）。
- **槽数分级**：4096 槽/分区按角色分级（系统分区满配、Agent 分区降配），削减常驻 BSS。
- **静态分区配额**：与 v0.132.0 分区内存配额联动，能力表占用计入分区内存预算账本。
- 以上列入后续 BSP/集成版本（spec D2 记录：真机静态区优化留待 BSP 版本）。

## 10. 性能实测

### 10.1 实测数据汇总（三口径）

| 平台 | 口径 | 指标 | 实测值 | 来源 |
|------|------|------|--------|------|
| host（Windows，x86_64，debug） | std Instant 注入时钟 | 1000 轮 check（存活能力 R\|W 请 R）总耗时 | **≈7.1µs ≈ 7ns/轮** | CPI8（`-- --nocapture` eprintln 记录口径，不设硬断言；功能计数复核 checks==1000、denied==0） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 100 轮 check(槽0, R) 延迟 avg | **162 ns** | `[cap] ok avg=162 max=4384`（qemu_run.md §21 PASS 轮原样收录） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 100 轮 check(槽0, R) 延迟 max | **4,384 ns ≈ 4.4µs** | 同上（单样本尖峰系 TCG 翻译块冷启动/仿真抖动，avg/max 相差 ≈27×，形态同 IPC/notify/shm 先例） |
| 真机（飞腾/鲲鹏） | CNTVCT 同口径 | check < 100ns（蓝图 §6.3/§7.2） | —（未测） | **留待硬件验证**（见 §11 阈值对照） |

### 10.2 cap_smoke 100 轮夹取方法学

- **夹取点**：每轮 `t0 = read_cycle_counter()` → `mgr.check(pid, 0, CapRights::READ)` → `t1 = read_cycle_counter()`，单调用夹取；`cycles_to_ns(t1-t0, freq)` 换算（复用 v0.138.0 wcet 设施）；avg = 100 轮总和/100（u128 累计防溢出），max 逐轮取大。
- **被测路径**：check 四段判定链全路径（槽命中 → rights 位判定 → 无约束跳过 → 0 哨兵不过期 → 计数累加）——无锁（临界区已在冒烟段持有）、无分配、无切换（主流程单线程直线执行，同 shm_smoke 口径，异于 IPC/notify 双线程原子发布口径）。
- **兜底**：freq/10 = 100ms 全程超时守护（超限打印 `[cap] err timeout round=<n>` 返回不挂死）；判定 = 功能全对（err_code==0）∧ max < `CAP_CHECK_LAT_MAX_NS` → `[cap] ok avg=<ns> max=<ns>`，失败走 `[cap] err code=/round=/aux=0x/max=` 不挂死（十四类错误码语义见 main.rs cap_smoke 段文档注释）。
- **QEMU 放宽口径决策记录**（qemu_run.md §21 原样引用并展开）：蓝图 §6.3「check < 100ns」为**真机目标**（单核无竞争、L1 命中、硬件计数器直读口径）；QEMU 侧放宽至 1.5ms 的依据为 (a) WSL2 下 qemu-system-aarch64 为 **TCG 纯软件仿真**（无 aarch64 KVM 加速），指令翻译使每轮 check 全路径墙钟代价远高于真机；(b) **CNTVCT 虚拟计数器粒度/递送开销**（v0.138.0 已记录同类坑点：同轮 wcet 上下文代理 avg=99ns 即纯计数器读 + 稳态路径的 QEMU 下界——本版本 avg=162ns 已逼近该下界）；(c) 1.5ms 阈值沿袭 v0.139.0 IPC 段「3 次实测 worst × 2 取整上浮」放宽先例的统一量级，对真机回归仍有约束力。

## 11. 阈值对照（蓝图 §6.3/§7.2 验收标准逐项判定）

| 蓝图锚点 | 口径 | 阈值/要求 | 实测/落点 | 判定 |
|---------|------|----------|----------|------|
| §6.3/§7.2 性能 | **真机 CNTVCT 目标** | check < 100ns | —（真机未测） | **留待飞腾/鲲鹏硬件验证**同口径重测（QEMU 数值不可作为真机性能证据，见 §12 坑点⑤） |
| §6.3 性能（QEMU 判定口径） | QEMU 冒烟硬判定 | `CAP_CHECK_LAT_MAX_NS` = 1,500,000 ns（1.5ms 放宽，依据见 §10.2） | max = 4,384 ns | ✅ PASS（**余量 ≈342×**；avg=162ns 逼近 QEMU 计数器读下界；**首轮一次通过，未触发放宽路径，阈值零调整**） |
| §6.3 性能（host 记录口径） | host CPI8 | 无硬断言（仅记录量级供人工对照） | ≈7ns/轮（1000 轮总 ≈7.1µs，debug） | 口径内记录 |
| §7.1 功能 | 越权访问被拒 | 三路径全锚定（CPI1：无能力槽 InvalidCapability / rights 不足 PermissionDenied / 超集 mint PermissionDenied 零副作用） | CPI1 + CM2/CM4 全过 | ✅ PASS |
| §7.3 安全 | 权限不可绕过 | mint 子集规则 D6（子集/相等成功、超集拒绝零副作用）+ check 四段无旁路 | CPI3 + CM2 + CM12 全过 | ✅ PASS |
| §7.5 出口判定 | **越权访问被拒 → P3-E 可启动** | checklist CL6 锚定（revoke 级联正确 + 判定链四错误精确） | CL6 满足 | ✅ **P3-E 可启动** |
| §9.4 可靠 | revoke 级联正确 | 三级链中间级撤销子连带失效、祖父不动、revokes 仅计 1 | CPI2 + CM7 全过 | ✅ PASS |
| §9.6 可观测 | 能力授予/撤销审计 | CapStats 九计数 saturating 精确核算 | CM12 全过 | ✅ PASS |
| §6.4 回归 | IPC/SHM 受能力保护 | CPI7 语义等价（D17 三场景双表达同结果）+ workspace 全量零回归 + QEMU 既有十四类串全中 | 全过（§14） | ✅ PASS |
| 构建预算 | release ELF < 512KB | ≈10MB BSS NOBITS 不占镜像 | kernel 148,104B（bootloader 70,440B） | ✅ PASS |

## 12. 坑点与风险（蓝图 §8.5 + 本版本实测坑点）

| 坑点/风险 | 本版本处置 |
|----------|-----------|
| ① **bin crate MaybeUninit 整体 write vs 逐字段有效化（D-C1）** | `next_cap_id`/`now` 为**私有字段**，bin crate（main.rs 与 lib 分属两 crate）不可经 `addr_of_mut!` 逐字段派生裸指针——Ipc/Shm/NotificationManager 逐字段先例**不可复用**；采 `MaybeUninit::write` **整体 write** 有效化（write 未有效化区不读旧值，安全方法）。决策记录全文见 main.rs cap_smoke 段文档注释 |
| ② **release sret/memcpy 消除免栈临时** | ≈10MB 值语义**严禁上 64KB 内核栈**：release（opt-level=3 + LTO + codegen-units=1）下 `new()` 大返回值经 **sret 隐藏返回指针直写目的地**、`create_space` 内 623KB `CapSpace::new()` 同经 LLVM **memcpy 消除**（MemCpyOpt slot 转换）免栈临时；aarch64 release 汇编实证 cap_smoke 及其内联全程无 10MB/623KB 级 sub sp 栈预留 |
| ③ **revoke 迭代终止性论证** | 多轮全表扫描迭代：每轮至少清一个（否则 break）→ 被占槽位严格单调递减有下界 → 必然终止；清除使 parent 信息随槽丢失，异常 parent 环逐轮瓦解（循环防护）；O(1) 栈无递归（蓝图 §5.4 难点「revoke 的递归原子性」在单核临界区口径下满足，见 §5.3） |
| ④ **find_free_slot 空槽复用对测试槽位编号的影响（CPI4 修正记录）** | find_free_slot 线性扫描**首个空槽**（CT3 口径：clear 后槽位复用）——revoke 槽 1 后下一 mint **复用槽 1** 而非顺序递增；CPI4 move 段槽位编号锚定该复用语义（落地修正，tests/cap_test.rs 头注记录）。编写派生链测试时不得以「槽位号单调递增」为隐含假设 |
| ⑤ **host/QEMU/真机三口径不可互证** | host std Instant（debug 注入时钟，≈7ns/轮）仅记录量级；QEMU TCG + 虚拟计数器（avg=162ns）为功能验证记录，**不可作为真机性能证据**（蓝图 §8.5 坑点对照，qemu_run.md §21 决策记录）；真机 <100ns 目标留待飞腾/鲲鹏同口径重测（同 v0.138.0~v0.141.0 先例链） |
| 蓝图 §8.5 坑点：能力表内存泄漏（revoke 遗漏） | D3 级联全表扫描无遗漏（悬空 parent 必被清除）+ CT5/CM7/CPI2 三级链锚定；`children_of` 迭代器供审计遍历全表重建派生关系 |
| 蓝图 §8.5 坑点：约束检查的性能开销 | check 四段无锁无分配直线判定；约束为 `Option` 单分支（None 直接跳过）；QEMU 实测 avg=162ns 逼近计数器读下界，机制路径代价可忽略 |
| 蓝图 §8.2 依赖：依赖 IPC/SHM/线程 | CapTarget 五变体仅**引用既有标识类型**（EndpointId/NotificationId/ShmId/Tid/PartitionId），不触碰既有模块实现；既有 ipc/sched/mem/syscall 零改动（spec 禁区），CPI7 以公开 API 验证等价语义 |
| 大对象驻留（≈10MB） | 全体 BSS NOBITS 不胀 ELF（§9.1）；QEMU 侧 MaybeUninit 整体 write（坑点①②）；host 侧 64MB 大栈线程 + Box（§9.1） |

## 13. D1~D12 偏差表（全局）

> 文档级编号与 [src/cap/mod.rs](../../crates/kernel/microkernel/src/cap/mod.rs) 头注 D1~D12 一一对应（自 spec.md 蓝图偏差决策表原样整理）；末五条为 Task 3 落地修正（manager.rs/mod.rs 头注记录）。

| 编号 | 蓝图口径 | 实际决策 | 理由 |
|------|---------|---------|------|
| D1 | 路径 `kernel/src/cap/` | `crates/kernel/microkernel/src/cap/` | 仓库规范 §2.3.1 强制子系统归口（同 ipc D1 先例） |
| D2 | §4.1 `slots: Vec` / `parent_map: HashMap` / `spaces: HashMap` | 定长化（`spaces: [Option<CapSpace>; 16]` + `slots: [Option<Capability>; 4096]`；§4.5 伪代码已部分定长，本决策完成剩余部分） | no_std 无堆（同 D2/D9/D16 先例链） |
| D3 | §4.5 `children: [Vec<CapSlot>; 4096]` 正向子表 | `Capability.parent: Option<CapSlot>` 反向链接 + revoke O(n) 全表扫描级联 | Vec 无堆不可编译；每槽定长 children 数组 = 4096×cap×4B 额外 BSS 且级联遍历仍 O(n)；revoke 为低频操作，蓝图 §8.1/§9.4 要求级联**正确性**而非性能 |
| D4 | `bitflags!` 宏 | 手写 `CapRights` const + `contains`/`is_superset_of`（READ=1/WRITE=2/INVOKE=4/GRANT=8/DERIVE=16 位值原样） | 零新依赖（cargo deny 无风险），同 MemAttrs 手写 attrs_to_bits 先例 |
| D5 | 伪代码 `crate::time::now()` | 构造注入时钟 `now: fn() -> u64`（ns 口径，aarch64 = CNTVCT+cycles_to_ns 复用 v0.138.0 设施；host 注入 mock） | 该函数不存在；注入式同既有可测性口径 |
| D6 | mint 伪代码 `!parent_cap.rights.0 & rights.0 == rights.0` | 修正为 `parent.rights.is_superset_of(requested)` 即 `(parent.0 & requested.0) == requested.0` | 伪代码运算符优先级 bug（Rust 中不编译）；语义 = rights 只能缩减（§4.2/§5.2 原样） |
| D7 | 伪代码约束违反→`PermissionDenied`、过期→`InvalidCapability` | 采 §4.4 文字口径：约束违反→`ConstraintViolation`、过期→`CapExpired` | §4.4 与伪代码矛盾；§4.3 mermaid 判定链（槽有效→rights→约束→过期）四错误一一对应 |
| D8 | §4.4 三个新错误 | **落地修正（Task 1 实测）**：`InvalidCapability` 自 v0.127.0 既有（`= 3`，语义即无效槽位/能力引用，与 §4.4 一致）→ 复用不重复定义（Rust 枚举禁同名）；仅追加 `ConstraintViolation=11 / CapExpired=12`；判别值 10 留作保留空洞不再分配（error.rs 头注已记录） | error.rs「新增必须追加、只追加不重排」规则；同名变体不可重复定义的编译约束 |
| D9 | trait 六方法（mint/move_cap/copy_cap/revoke/check/derive） | trait 原样六方法 + 补充 `create_space(pid)` / `destroy_space(pid)` | 伪代码 `spaces[pid].ok_or(InvalidArgument)` 但全图无 space 注册接口——蓝图接口缺失的最小补全（v0.141.0 grant_shm 补充先例） |
| D10 | §4.2「move/copy：能力在槽位间转移」未言派生关系 | copy/move **不建立**派生关系（revoke 不级联到 copy）；仅 mint/derive 建派生边 | seL4 CDT 语义（仅 mint 建边），对齐 §8.4「与 seL4 capability 语义一致」 |
| D11 | `ConstraintPack { max_power: f32, time_window }` | time_window 判定落地（check 拒绝窗外）；max_power **仅存储不判定** | §4.3 判定链只列时间窗；功耗约束判定属后续能源建模层；no_std f32 可存储不计算 |
| D12 | `signature: [u8; 64]` SM2 签名 | 本版本恒 `[0u8; 64]`；§6.5「伪造能力令牌」故障注入**后置 v0.158.0**，本版本故障注入 = 过期能力 + 槽位耗尽 | 蓝图注释「SM2 签名（v0.158.0 接入）」原样 |

**Task 3 落地修正五条**（mod.rs/manager.rs 头注记录，spec 内部矛盾的最小化解）：

| # | 修正 | 理由 |
|---|------|------|
| 1 | `check` 由 spec trait 代码块 `&self` 改为 `&mut self` | spec 同时要求 check 累加 checks/denied/违规计数（§9.6）而 CapStats 为 plain u64 字段——safe Rust 下 `&self` 不可改 plain 字段，两约束互斥；采 `&mut` 单核口径，返回 `&Capability` 语义不变，SMP 原子化阶段再评估 `&self`+原子计数 |
| 2 | move 既有派生边随槽迁移：子代 parent 指针 src→dst 改写 | `children_of(dst)` 命中；保「parent 恒指向存活槽」不变式，revoke 级联正确性依赖之；迁移既有边非新建边，不违 D10 |
| 3 | revoke 级联选型：多轮全表扫描迭代（先清根，再逐轮清除 parent 已悬空者直至无变化） | O(1) 栈；每轮至少清一个必然终止；parent 环逐轮瓦解循环防护；顶层 revokes 仅计 1，级联清除不重复计数 |
| 4 | `destroy_space` 未创建 → `InvalidArgument` | 与 pid 越界同口径「无 space 可毁」；区别于 `create_space` 重复之 `BadState` 状态冲突语义 |
| 5 | `spaces`/`stats` 字段 pub（`next_cap_id`/`now` 私有） | 审计遍历 + 内核/集成测试预置根能力种子——全图无 mint 根 API，同 ShmManager pub regions/stats 先例 |

## 14. 测试矩阵

host 侧执行 `cargo test -p eneros-microkernel`：**lib 176 passed / 0 failed**（含本版本新增 CAP1~4/CT1~5/CM1~12 共 21 例）+ 集成测试 14 套件零回归（含 cap_test **8 passed / 1 ignored**，CPI9 为 `#[ignore]` 故障注入按设计跳过，`-- --ignored` 单独运行 1 passed）。构造 CapabilityManagerImpl 的用例统一经 64MB 大栈线程 + Box 持有（≈10MB 内联防 host 默认栈溢出，§9.1）。

### 14.1 单元测试（src/cap 内嵌，新增 21 个）

| 编号 | 用例位置 | 验证点 | 蓝图锚点 |
|------|---------|--------|---------|
| CAP1 | token.rs | CapRights 位运算真值表：contains/is_superset_of 同语义（D6 位公式）、空权限 0 与全位 31 边界、五位值 1/2/4/8/16 锚定 | §3/§5.2 |
| CAP2 | token.rs | Capability 九字段构造逐字段读回 + parent Option 语义 + signature 恒零（D12） | §3/§4.1 |
| CAP3 | token.rs | CapType 七变体互异 + CapTarget 五变体持有各 Id 类型 | §3 |
| CAP4 | token.rs | MAX_CAPS_PER_PARTITION==4096 锚定 + KernelError 新变体判别值（ConstraintViolation=11/CapExpired=12/InvalidCapability=3 复用/10 空洞，既有 1~9 不动） | §3 配置/§4.4/D8 |
| CT1 | table.rs | 空表全 None + get 越界（4096/u32::MAX）None 不 panic + Default 同语义 | §4.1 |
| CT2 | table.rs | set/get/clear 往返 + 覆盖式 set + 越界 set 返 false 不写 + 越界 clear 静默 | §4.1 |
| CT3 | table.rs | find_free_slot 递增与复用（占 0→返 1；清 0→复用返 0） | §4.5/坑点④ |
| CT4 | table.rs | 4096 连 find_free_slot+set 占满 → 第 4097 次 OutOfMemory；clear 后复用 | §4.5/§6.5 |
| CT5 | table.rs | children_of 全表扫描 parent 链接（三级链直子/孙/叶子边界、升序、clear 后重建） | D3/§9.4/§9.5 |
| CM1 | manager.rs | create_space 生命周期（成功/重复 BadState/pid 越界 InvalidArgument）+ 无 space 六方法统一 InvalidArgument（D9） | D9/§4.4 |
| CM2 | manager.rs | mint 子集/相等/超集三态（子能力 rights 如请、cap_type/constraints/expires_at 继承、id 自增互异、parent 建边、issued_at=now）+ 超集 PermissionDenied+denied+1 零副作用不占槽 | §5.2/§6.1/§7.3/D6 |
| CM3 | manager.rs | mint 父槽无效（空/越界）InvalidCapability + 满槽 OutOfMemory（管理器路径 4095 连 mint 后第 4096 次拒绝） | §4.4 |
| CM4 | manager.rs | check 四段判定链（四错误精确 + 计数各归各位）+ **顺序固定**（rights 不足且窗外→先 rights；窗外且过期→先约束） | §4.3/§4.4/§6.1 |
| CM5 | manager.rs | expires_at=0 哨兵永不过期（now=u64::MAX 仍过）+ now==expires_at 边界不过期（严格大于） | §6.5/§8.3 口径 |
| CM6 | manager.rs | derive 权限/target 不变 + 附加约束 + 建边 + 窗内过/窗外拒 + 错误路径 | §4.5/D11 |
| CM7 | manager.rs | revoke 三级级联（中间级撤销子连带、祖父不动、再撤祖父仅清祖父）+ revokes 仅顶层计 1 + 错误路径 | §8.1/§9.4 |
| CM8 | manager.rs | copy 不受级联（D10：副本 parent=None 新 id 字段复制；revoke 父副本不动且 check 通过） | §8.4/D10 |
| CM9 | manager.rs | move 源清目标立 + parent 随本体迁移 + 子代 parent src→dst 改写（children_of(dst) 命中）+ 迁移后级联保持 | D10/§9.4 |
| CM10 | manager.rs | move/copy 错误路径（dst 占用/越界、src 无效/越界、dst==src 自移）+ 失败零副作用 | §4.4 |
| CM11 | manager.rs | destroy_space 语义（未创建 InvalidArgument 选型、销毁后操作 InvalidArgument、重建为空表） | D9 |
| CM12 | manager.rs | CapStats 九计数精确核算（已知操作串逐字段断言；无效槽 check 不计任何统计） | §9.6 |

### 14.2 集成测试（tests/cap_test.rs，CPI1~CPI9，仅以公开 API 端到端验证）

| 编号 | 用例 | 蓝图对照 | 验证点 | 结果 |
|------|------|---------|--------|------|
| CPI1 | `cpi1_unauthorized_access_denied_three_paths` | §6.2 越权访问被拒 + §7.1/§7.3 | 无能力槽 check → InvalidCapability / rights 不足 check → PermissionDenied+denied / 超集 mint → PermissionDenied 零副作用 | PASS |
| CPI2 | `cpi2_transfer_chain_and_revoke_cascade` | §6.2 能力传递与撤销 + §9.4 | 三级 mint 链 root→A→B→C revoke 中间级 → B/C 连带失效、祖父不动、revokes 仅顶层计 1 | PASS |
| CPI3 | `cpi3_mint_rights_subset_equal_superset` | §5.2/§7.3（D6） | 子集/相等成功 rights 如请、超集拒且零副作用 | PASS |
| CPI4 | `cpi4_move_copy_semantics` | D10（seL4 CDT 口径） | move 源清目标立、copy 双立 parent=None 不受 revoke 级联、move 子代 parent src→dst 改写级联保持（槽位编号锚定 find_free_slot 复用语义，坑点④） | PASS |
| CPI5 | `cpi5_derive_constraint_pack_time_window` | D7/D11 | derive 附加 time_window：窗内（含闭区间端点）通过/窗外 ConstraintViolation 精确计数；max_power=999.9 仅存储不判定 | PASS |
| CPI6 | `cpi6_expired_capability_mock_clock` | §6.5 故障注入 | mock 时钟推进 expires_at=1000 过后 CapExpired；0 哨兵 u64::MAX 不过期 | PASS |
| CPI7 | `cpi7_ipc_shm_targets_and_d17_semantic_equivalence` | §6.4 回归 IPC/SHM 受能力保护 | 真实 Endpoint(7)/Shm(3) 目标 mint+check 全链路 + shm D17 三场景语义等价（owner 隐式 RW 双过/RO 授权读过写拒 PermissionDenied/未授权 InvalidCapability；shm.rs 零改动、不实例化 ShmManager、跨三分区空间表达） | PASS |
| CPI8 | `cpi8_check_performance_smoke` | §6.3 对照 | std Instant 1000 轮 check eprintln 记录口径不设硬断言（实测 ≈7ns/轮）；功能计数 checks==1000、denied==0 复核 | PASS |
| CPI9 | `cpi9_slot_exhaustion_out_of_memory`（`#[ignore]`） | §6.5 故障注入 | 种子根 + 4095 连 mint 占满后第 4096 次 → OutOfMemory 不 panic、表满状态不破坏、mints 不计失败（伪造令牌故障注入后置 v0.158.0，D12） | ignore（口径内；`-- --ignored` 1 passed） |

### 14.3 QEMU 冒烟（main.rs §3.16 `cap_smoke` 段 + [qemu_run.md](../boot/qemu_run.md) §21）

| 实测项 | 值 |
|--------|-----|
| 结果 | **PASS**（boot_test.sh **十五**关键串断言：`[boot]` 横幅 / `[kernel] booting` / `[pmm] alloc ok` / `[vmm] mmu on` / `[part] iso ok` / `[irq] ok` / `[sched] ok` / `[psched] ok` / `[smp] ok` / `[sync] ok` / `[wcet] ok` / `[ipc] ok` / `[notify] ok` / `[shm] ok` / **`[cap] ok`**；v0.142.0 起追加第十五条断言 `grep "[cap] ok"`，既有十四条原样） |
| 串口关键行 | **`[cap] ok avg=162 max=4384`**（PASS 轮原样收录） |
| 轮次 | 首轮一次 PASS，无排障记录，无阈值调整（未触发放宽路径，`CAP_CHECK_LAT_MAX_NS` 零调整） |
| 冒烟流程 | ① CAP_MANAGER 整体 write 有效化（D-C1）→ ② create_space(PartitionId(0)) → ③ pub spaces 预置根能力种子（槽 0，id=0xFFFF，R\|W\|INVOKE，Endpoint(0)，expires_at=0，parent=None）→ ④ mint 两级缩减（R\|W\|INVOKE→R\|W 槽1→R 槽2）→ ⑤ 超集越权 mint → PermissionDenied 精确判别 → ⑥ check 双路径（槽2,R 过/槽2,W 拒）→ ⑦ derive 窗内（0,u64::MAX）通过 → ⑧ derive 窗外（0,1）→ ConstraintViolation → ⑨ 过期种子（槽 100，expires_at=1）→ CapExpired → ⑩ revoke(槽1) 级联核验（槽2 连带 InvalidCapability、根槽 0 不动）→ ⑪ CNTVCT 夹取 100 轮 check(槽0,R) → ⑫ 判定打印 |
| 实测环境 | 2026-07-27，WSL2 Ubuntu-22.04 + QEMU 8.2.2 + nightly-2026-04-04，`-M virt,gic-version=3 -smp 4`；端到端 15s（构建 4.22s + QEMU timeout 10s 守护窗口） |
| 回归 | host 全量 lib 176 passed + 14 集成套件零回归；release ELF kernel 148,104B < 512KB（≈10MB BSS NOBITS 不占镜像）；与既有十四类输出正交（不触碰 timer/GIC/SGI/ARINC/IPC/Notification/SHM 设施，PMM 零消耗） |

回归（蓝图 §6.4「IPC/SHM 受能力保护」）：CPI7 语义等价验证（§14.2）+ workspace 全量零回归 + QEMU 既有十四类关键串（`[boot]`~`[shm]`）全中（qemu_run.md §21 串口全文）。

## 15. 源码链接

| 项 | 路径 |
|----|------|
| 能力子系统挂载与 D1~D12 头注 | [crates/kernel/microkernel/src/cap/mod.rs](../../crates/kernel/microkernel/src/cap/mod.rs) |
| 能力令牌（九类型 + CAP1~CAP4 单测） | [crates/kernel/microkernel/src/cap/token.rs](../../crates/kernel/microkernel/src/cap/token.rs) |
| 能力表（CapSpace + ChildrenIter + CT1~CT5 单测） | [crates/kernel/microkernel/src/cap/table.rs](../../crates/kernel/microkernel/src/cap/table.rs) |
| 能力管理器（trait 八方法 + Impl + CapStats + CM1~CM12 单测） | [crates/kernel/microkernel/src/cap/manager.rs](../../crates/kernel/microkernel/src/cap/manager.rs) |
| KernelError 追加两变体（11/12 + 10 空洞头注） | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs) |
| lib 挂载（`pub mod cap;`） | [crates/kernel/microkernel/src/lib.rs](../../crates/kernel/microkernel/src/lib.rs) |
| QEMU 冒烟段（cap_smoke ①~⑫ + D-C1 决策记录 + 十四类错误码注释） | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `cap_smoke`（rust_main §3.16 调用点） |
| host 集成测试 CPI1~CPI9 | [crates/kernel/microkernel/tests/cap_test.rs](../../crates/kernel/microkernel/tests/cap_test.rs) |
| QEMU 测试脚本（第十五条断言 `[cap] ok`） | [tests/boot_test.sh](../../tests/boot_test.sh) |
| QEMU 实测记录（串口全文/十五断言/延迟判定口径决策记录/首轮 PASS） | [../boot/qemu_run.md](../boot/qemu_run.md) §21 |
| v0.141.0 共享内存设计文档（ShmCap 授权表过渡形态 D17 + CSpace 接管预告 §3.3） | [./shm_design.md](./shm_design.md) |
| v0.140.0 通知机制设计文档（Notification 目标资源来源） | [./notification_design.md](./notification_design.md) |
| v0.139.0 同步 IPC 设计文档（Endpoint 目标资源来源 + 1.5ms QEMU 放宽阈值先例来源） | [./ipc_design.md](./ipc_design.md) |
