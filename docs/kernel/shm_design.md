# EnerOS v0.141.0 共享内存 IPC 设计文档（shm / 连续帧游 / 能力控制映射 / 缓存同步）

> 版本：v0.141.0
> 适用范围：EnerOS 自研内核 IPC 子系统共享内存层（共享内存对象 + 连续物理帧游 + 能力控制映射 + ARM64 缓存维护；host 集成测试 + QEMU virt aarch64 目标机 `-M virt,gic-version=3 -smp 4` 双 VA 映射零拷贝 CNTVCT 实测）
> 蓝图依据：`蓝图/phase3.md` §v0.141.0（§1 版本目标 / §3 交付物 / §4 详细设计 / §5 技术交底 / §6 测试计划 / §7 验收标准 / §8 风险与注意事项 / §9 多角度要求）
> Phase 定位：**P3-D IPC 与能力线第 3 版**（v0.139.0 ~ v0.142.0 四版之三），在 v0.140.0 通知机制之上提供**零拷贝大数据传输**能力，为 **v0.142.0 能力管理器提供共享资源粒度**（蓝图 §1 Phase 定位原样）
> 配套文档：[notification_design.md](./notification_design.md)（v0.140.0 通知机制设计，生产者-消费者事件协调与 1.5ms QEMU 放宽阈值先例来源）、[ipc_design.md](./ipc_design.md)（v0.139.0 同步 IPC 设计）、[partition_isolation.md](./partition_isolation.md)（v0.132.0 分区内存隔离，PartitionManager 与独立根页表设施来源）、[vmm_design.md](./vmm_design.md)（v0.131.0 虚拟内存，页表遍历与 MemAttrs 编码来源）、[../boot/qemu_run.md](../boot/qemu_run.md) §20（v0.141.0 QEMU 实测记录：首轮一次 PASS，无阈值调整）

---

## 1. 版本目标

**核心一句话**：实现受能力保护的共享内存 IPC，支持分区间大数据传输（蓝图 §v0.141.0 核心一句话原样口径）。

- **业务价值**：LLM 模型数据、时序数据等大数据通过共享内存**零拷贝**传输（蓝图 §1）。同步 IPC（v0.139.0，8 word 消息寄存器）只适合小载荷「请求-应答」；大批量数据走共享内存带外传递，notification（v0.140.0）负责「数据就绪/已消费」事件握手——三层分工与蓝图 §5.1 选型表「共享内存 + notification」折中口径一致。
- **下游衔接**：为 **v0.142.0 能力管理器提供共享资源粒度**——本版本的 `ShmCap` 授权表（D17）是能力管理的过渡形态，v0.142.0 将以统一 CSpace 能力模型接管 `caps` 授权判定（见 §3.3）。
- **ADR-0001 裁剪口径**（蓝图 §1 引用段原样口径）：本版本原为自研内核组件，已按方案 A 调整为 seL4 深度定制/信创认证路径。**共享内存语义借鉴 seL4 Frame + CSpace 映射机制，本内核按同语义自研实现**，保持语义同构以降低 v0.156.0 迁移成本；差异点（连续帧游替代逐帧 Vec、定长授权表替代 CSpace）属 D15~D17 决策，见 §6 与 §10。
- **依赖关系**：前序 v0.140.0 通知机制可用（蓝图 §2 前序版本：生产者-消费者配合路径 SHI2 锚定）；v0.131.0 虚拟内存可用（共享页映射的页表设施来源，蓝图 §2 外部依赖）；v0.132.0 分区管理可用（双分区地址映射的独立根页表设施来源，蓝图 §2 假设「共享内存需双分区地址映射」）。

## 2. 交付物清单

对照蓝图 §3 逐项标注偏差（D 编号与 [src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs) 头注 D15~D20 一一对应，全局表见 §10）：

| 蓝图 §3 交付物 | 实际落位 | 偏差 |
|---------------|---------|------|
| 代码：`kernel/src/ipc/shm.rs`（共享内存对象） | [crates/kernel/microkernel/src/ipc/shm.rs](../../crates/kernel/microkernel/src/ipc/shm.rs)（ShmId / ShmMapping / ShmCap / SharedMemory / ShmStats / ShmManager / ShmIpc trait / MAX_SHM / MAX_SHM_SIZE / MAX_MAPPINGS_PER_SHM） | D15 连续帧游化；D16 mappings 定长化；D17 授权表新增 |
| 代码：`kernel/src/ipc/shm_frame.rs`（共享页帧管理） | [crates/kernel/microkernel/src/ipc/shm_frame.rs](../../crates/kernel/microkernel/src/ipc/shm_frame.rs)（frames_for_size / alloc_shm_frames / free_shm_frames / sync_frames） | D15 alloc_pages 包装；D19 缓存维护移入 asm 助手 |
| 代码：模块挂载与公开导出 | [crates/kernel/microkernel/src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs)（`pub mod shm; pub mod shm_frame;` + `pub use` 导出 + D15~D20 头注，surgical 追加） | 无（蓝图未单列 mod.rs） |
| 代码：KernelError 追加 AlreadyMapped 变体 | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs) `AlreadyMapped = 9`（surgical 单变体追加，既有八变体零改动） | D18（蓝图 §4.4 文字口径落码） |
| 代码：PartitionManager 单帧映射三方法 | [crates/kernel/microkernel/src/mem/partition.rs](../../crates/kernel/microkernel/src/mem/partition.rs) `map_partition_frame / unmap_partition_frame / translate_partition_frame`（surgical 仅追加，既有方法零改动） | D20（蓝图 `map_shared_page`/`unmap_page` 不存在，适配既有页表设施） |
| 代码：ARM64 缓存维护助手 | [crates/kernel/microkernel/src/mem/asm/aarch64.rs](../../crates/kernel/microkernel/src/mem/asm/aarch64.rs) `dc_cvau / ic_ivau / dsb_ish / isb`（surgical 仅追加，既有 enable_mmu/tlbi_va 零改动） | D19（蓝图内联 asm → 助手函数） |
| 接口：`ShmId` / `SharedMemory` / `ShmIpc` trait | 蓝图 §3 原样导出（`ShmId = u32`；SharedMemory 七字段；trait 五方法 + grant_shm 补充） | D15 frames 类型；D16 mappings 类型；D17 trait 补充 grant_shm |
| 文档：`docs/kernel/shm_design.md` | 本文档 | 无（路径同蓝图） |
| 测试：`kernel/tests/shm_test.rs` | [crates/kernel/microkernel/tests/shm_test.rs](../../crates/kernel/microkernel/tests/shm_test.rs)（SHI1~SHI8；SHI8 `#[ignore]` 故障注入） | 无 |
| 测试：lib 内嵌单测 | SHM1~SHM3（shm.rs）/ SHF1~SHF5（shm_frame.rs）/ SGM1~SGM9（shm.rs 管理器）（新增 17 个） | 蓝图 §6.1 单元测试落点 |
| 测试：QEMU 冒烟 | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `shm_smoke` 段（4 页 shm 双 VA 映射零拷贝图样校验 + sync_shm + 100 轮 map/unmap 配对 CNTVCT 实测） | 蓝图 §6.2 集成测试的目标机落点 |
| 配置：MAX_SHM=128, MAX_SHM_SIZE=64MB | [shm.rs](../../crates/kernel/microkernel/src/ipc/shm.rs) `MAX_SHM = 128` / `MAX_SHM_SIZE = 64 * 1024 * 1024` | 无（蓝图原样） |
| 配置：映射数/授权数上限（蓝图未限定） | [shm.rs](../../crates/kernel/microkernel/src/ipc/shm.rs) `MAX_MAPPINGS_PER_SHM = 16`（= MAX_PARTITIONS，每分区至多一条） | D16/D17 有界化取值 |

**接口一览**（蓝图 §3 接口类签名 + D15~D17 修正，[ipc/shm.rs](../../crates/kernel/microkernel/src/ipc/shm.rs) 公开导出）：

| 项 | 定义 | 说明 |
|----|------|------|
| `ShmId` | `pub type ShmId = u32` | 蓝图 §3 原样；取值范围 0..128（= shm 表槽位下标） |
| `SharedMemory` | struct 七字段 | `id / start: PhysFrameNum / page_count: usize / size: usize / mappings / caps / owner`（蓝图 §3/§4.1 字段；frames → 连续帧游 D15，mappings 定长化 D16，caps 新增 D17） |
| `ShmMapping` | struct 三字段 | `pid / vaddr / writable`（D16：一条映射 = 分区 + VA + 可写位） |
| `ShmCap` | struct 三字段 | `pid / readable / writable`（D17：蓝图 §4.1 `ShmCapability` 的落码形态，sid 字段由宿主表位置隐含） |
| `ShmManager` | struct 两字段 | `regions: [Option<SharedMemory>; MAX_SHM] / stats: ShmStats`（蓝图 §4.1 原样 + §9.6 统计） |
| `ShmIpc` trait | 六方法 | `create_shm / map_shm / unmap_shm / destroy_shm / sync_shm`（蓝图 §3 五方法）+ `grant_shm`（D17 补充） |
| `ShmStats` | struct 八字段 | `creates / destroys / maps / unmaps / syncs / grants / denied / pages_mapped`（蓝图 §9.6 映射数/页数统计；全部 `saturating_add` 饱和累加） |

## 3. 共享内存模型

### 3.1 连续帧游（D15 vs 蓝图 Vec 对照）

蓝图 §3/§4.5 的 `frames: Vec<PhysFrameNum>` 为**逐页独立分配**模型（`for _ in 0..pages { frames.push(pmm.alloc(0,0)?) }`），本实现改为**连续帧游** `(start: PhysFrameNum, page_count: usize)`，一次性 `PhysMemManager::alloc_pages(page_count)` 分配：

| 维度 | 蓝图 Vec 逐页模型 | 本实现连续帧游（D15） | 选择 |
|------|------------------|----------------------|------|
| no_std 兼容 | 需 alloc（Vec 堆分配） | 两字段定长，零堆 | 连续帧游 |
| 分配失败回滚 | 逐页分配中途失败需逐页归还已得帧 | `alloc_pages` 原子成败，失败零帧泄漏（create_shm 失败路径无回滚负担，SGM1 锚定） | 连续帧游 |
| 帧号存储开销 | O(pages)（64MB shm = 16384 帧号 = 128KB） | O(1)（start + count 两字段） | 连续帧游 |
| 物理连续性 | 不保证（buddy 逐页分配可能离散） | **天然连续**（buddy 连续分配原生支持），DMA/大页优化留有余地 | 连续帧游 |
| 第 i 帧计算 | `frames[i]` 查表 | `start + i` 算术（map/unmap/sync 逐帧步进） | 连续帧游 |

物理连续性对「LLM 模型数据、时序数据大数据传输」（蓝图 §1 业务价值）无功能影响（虚拟地址连续即可），但为后续 DMA 直写共享区、2MB 大页映射等优化保留了物理布局前提（§8.5 坑点「大页共享的缓存对齐」的应对基础）。

### 3.2 对象生命周期（蓝图 §5.3 实现路径）

```
create_shm(size, owner)          map_shm(sid, pid, va, w)         unmap_shm(sid, pid)        destroy_shm(sid)
─────────────────────           ──────────────────────────        ─────────────────────      ─────────────────────
size 校验（0→InvalidArgument，   sid/VA 对齐校验                   找 pid 映射槽               mappings 非空→BadState
>64MB→OutOfMemory）             能力校验（§3.3）                  未映射→幂等 Ok              mappings 全空
alloc_pages 连续帧游（失败       同 pid 重复→AlreadyMapped        逐帧 unmap_partition_frame   free_pages 归还帧游
零帧泄漏）                      逐帧 map_partition_frame          mappings 清槽                槽位回收 + destroys+1
扫槽登记 + owner 隐式 RW         mappings 登记 + pages_mapped     stats.unmaps+1
creates+1                       +=pages + maps+1
```

- **create**：`frames_for_size` 页数换算（向上页对齐）→ `alloc_shm_frames` 一次性连续分配 → 线性扫 128 槽 → 登记 SharedMemory。size==0 → `InvalidArgument`；size > 64MB → `OutOfMemory`；槽满 → `OutOfMemory`（蓝图 §4.4 超限口径）。
- **destroy 安全回收**（蓝图 §9.4）：mappings 非空 → `BadState` 拒绝销毁——强制调用方先全量 unmap，杜绝「销毁后他分区仍持映射访问已回收帧」的 use-after-free（§4.2 坑点应对）；unmap 全量后 `free_pages` 归还整段帧游，SGM7/SHI6 锚定 PMM `free_pages_count` 精确复原（帧零泄漏）。

### 3.3 能力控制映射（D17 授权表 + owner 隐式 RW）

蓝图 §4.5 仅注释「简化：owner 可映射，其他需授权」但未给出授权接口（§4.1 `ShmCapability { sid, readable, writable }` 仅为数据形态）。本实现落码为**每 shm 定长授权表** `caps: [Option<ShmCap>; 16]`：

| 判定路径 | 规则 | 错误 |
|---------|------|------|
| owner 映射 | **隐式 RW**（不入 caps 表，`create_shm` 时即获全权，SGM2 锚定） | — |
| 他分区映射 | 查 caps 表：无该 pid 表项 → 拒绝 | `PermissionDenied` + `stats.denied+1`（SGM4/SHI3 锚定） |
| RO 授权 RW 请求 | caps 表项 `writable=false` 且 `map_shm(writable=true)` → 拒绝 | `PermissionDenied` + `stats.denied+1`（SGM4/SHI3 锚定） |
| 同分区重复映射 | 该 pid 已有映射表项 → 拒绝 | `AlreadyMapped`（D18，SGM5 锚定） |
| `grant_shm(sid, pid, r, w)` | sid 校验 + caps 空槽登记；满 16 → `OutOfMemory`（SHI8 故障注入锚定） | `OutOfMemory` |

**v0.142.0 接管路线**：本授权表是能力管理的过渡形态（蓝图 §1「为 v0.142.0 能力管理器提供共享资源粒度」）。v0.142.0 将以统一 CSpace 能力模型接管 `caps` 的登记与判定——`grant_shm` 对应 seL4 `seL4_CNode_Mint`（派生弱化权限），caps 表项的 `(pid, r, w)` 对应 CSpace 槽位中的 Frame capability + 权限位；`map_shm` 的能力校验点（当前查 caps 表）即 v0.142.0 统一能力查找的替换点。语义同构设计（owner 全权/授权弱化/按位校验）保证替换时映射层零改动。

## 4. 映射与撤销（D20 三方法）

### 4.1 PartitionManager 外科式追加

蓝图 §4.5 调用 `part_mgr.map_shared_page(pid, vaddr, frame, attrs)` / `unmap_page(pid, vaddr)`，两方法在 v0.132.0 交付的 PartitionManager 上**不存在**（既有 `map_partition_mem` 自带帧分配与配额扣减，与共享内存「帧由 ShmManager 持有」的语义冲突）。D20 追加三个 inherent 方法（**仅追加，既有方法零改动**，源码 [mem/partition.rs](../../crates/kernel/microkernel/src/mem/partition.rs)）：

| 方法 | 语义 | 与 `map_partition_mem` 的关键差异 |
|------|------|----------------------------------|
| `map_partition_frame(pid, vaddr, frame, attrs)` | 在该分区**独立根页表**上建单页 `vaddr→frame` 映射 | **跳过帧分配与配额扣减**（帧由 ShmManager 持有，生命周期属 shm 而非分区）；**不登记 allowed_regions**（避免 `destroy_partition` 回收非本分区持有的共享帧）；复用 `page_table::map_page` 页表遍历 |
| `unmap_partition_frame(pid, vaddr)` | 解除单页映射 | **不回收帧**（帧归还由 `destroy_shm` 统一负责）；`unmap_page` 的 BadState 统一转写 `InvalidArgument` |
| `translate_partition_frame(pid, vaddr) -> Option<PhysFrameNum>` | 遍历该分区根页表读 L3 PTE | 只读遍历；零拷贝判定的断言设施（SHI1 双侧同帧判定） |

属性编码（蓝图 §4.5 原样）：`{ readable: true, writable, executable: false, user_accessible: true, device: false, cacheable: true }`——Normal WB + Inner Shareable，可缓存是零拷贝性能前提，缓存一致性代价由 §5 维护序列承担。

### 4.2 撤销映射时他分区仍在访问（蓝图 §8.5 坑点应对）

| 坑点 | 本版本口径 | 锚定 |
|------|-----------|------|
| 撤销映射时其他分区仍在访问 | unmap 只解除**调用方 pid 自己**的映射，他分区映射不受影响（SHI5：三映射共存，unmap 其一余者 translate 不动）；ARM64 下解除映射后的 TLB 旧条目由后续集成层 TLB shootdown 覆盖（v0.136.0 SGI 设施已备，本版本单核冒烟不触发） | SHI5/SHF4 |
| 销毁时仍有映射持有 | `destroy_shm` 对 mappings 非空直接 `BadState` 拒绝——**先 unmap 全量后 destroy** 的协议级强制，从协议上杜绝「帧已回收而他分区页表仍指向」的悬挂映射 | SGM7/SHI6 |

## 5. 缓存同步（D19 维护序列）

### 5.1 ARM64 维护序列（蓝图 §4.5 原样序列，助手化）

`sync_shm` 对帧游逐 VA 执行（源码 [mem/asm/aarch64.rs](../../crates/kernel/microkernel/src/mem/asm/aarch64.rs) D19 助手，host 侧 `shm_frame::sync_frames` no-op 桩）：

```text
per VA（页对齐逐页步进）:
  dc cvau, VA   // D-cache 清至 PoU：脏行写回，使指令侧与外部观察者可见
  dsb ish       // 内在共享域屏障：保证 dc 完成后再做 ic
  ic ivau, VA   // I-cache 失效至 PoU：旧指令行失效，重新自取最新数据
  dsb ish       // 保证 ic 完成
  isb           // 冲刷流水线：后续取指见到维护后的最新指令
```

**PoU 语义**（Point of Unification）：PoU 是统一点，在该点上指令缓存与数据缓存对同一地址的访问见到同一份数据。`dc cvau` 把脏数据行清到 PoU（其他核的 I 侧/外部 DMA 可见），`ic ivau` 再使本核对齐——两段配合覆盖「数据写回 + 指令一致」双方向（蓝图 §5.2「缓存同步（DC CVAU + IC IVAU）确保指令/数据一致」原样口径）。

### 5.2 多核多分区一致性（蓝图 §5.4/§8.1 难点应对）

- **分区维度**：同一物理帧经多分区独立根页表映射到各自 VA——ARM64 缓存按物理地址 tag（VIPT 但 PoU 点物理一致），**虚拟别名不引入不一致**；`dc cvau` 按 VA 操作但清至 PoU 后即物理地址粒度生效，任一映射方执行 sync 即可使全部映射方可见。
- **核维度**：`dsb ish`（Inner Shareable 域屏障）保证维护操作在共享域内全局可见后再推进；Normal WB + Inner Shareable 属性（§4.1）使多核一致性由硬件 MOESI 类协议兜底，sync 序列处理的是**跨 cache 层级（D/I 分离）与外部观察者**的可见性，非核间一致性本身。
- **使用协议**：生产者写数据 → `sync_shm` → `signal` notification（SHI2 生产者-消费者路径锚定）；消费者 wait 命中 → 读数据。notification 的 Release/Acquire 语义（v0.140.0 `fetch_or(Release)`）与 sync 序列叠加构成完整的「写→可见→通知」顺序链。

## 6. seL4 Frame + CSpace 语义对照（蓝图 §8.4）

| 维度 | seL4（ADR-0001 方案 A 参照系） | 本内核 v0.141.0 | 迁移成本 |
|------|------------------------------|----------------|---------|
| 共享内存对象 | Frame capability（物理帧 + 权限位） | `SharedMemory`（连续帧游 + owner） | 帧游逐帧转 Frame cap，一一对应 |
| 授权机制 | CSpace：`seL4_CNode_Mint` 派生弱化 cap | `caps` 定长授权表 + `grant_shm`（D17） | grant → Mint 语义同构，查表替换为 CSpace 查找（§3.3 接管路线） |
| 映射操作 | `seL4_ARM_Page_Map`（VSpace + Frame cap） | `map_partition_frame`（分区根页表 + 帧号，D20） | 能力校验点替换，页表操作等价 |
| 缓存维护 | `seL4_ARM_Page_Clean_Data` / `seL4_ARM_Page_Unify_Instruction` | `sync_shm` → dc cvau + ic ivau 序列（D19） | syscall 号映射，序列同构 |
| 通知配合 | seL4 Notification（v0.140.0 已同构） | notification（SHI2 配合路径） | 零 |

**§8.4 注（x86 缓存指令不同）**：x86 为自洽缓存（self-snooping）架构，数据写回对指令侧/其他核自动可见，**无 dc/ic 维护指令**；sync_shm 在 x86 目标上可退化为编译屏障 + `mfence`。本实现的 `#[cfg(target_arch = "aarch64")]` 分岔 + host no-op 桩已为跨架构退化预留形态（同 vmm 模块 asm 助手先例）。

## 7. 性能实测

### 7.1 实测数据汇总

| 平台 | 口径 | 指标 | 实测值 | 来源 |
|------|------|------|--------|------|
| host（Windows，x86_64） | std Instant 注入时钟 | 100 轮 map/unmap 配对总耗时 | **5.6µs**（均值 **56ns/轮**） | SHI7（`-- --nocapture` eprintln 记录口径，不设硬断言） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 单页映射延迟 avg | **2,309 ns ≈ 2.3µs** | `[shm] ok` 行（qemu_run.md §20） |
| QEMU virt aarch64（TCG） | CNTVCT + cycles_to_ns | 单页映射延迟 max | **197,232 ns ≈ 197.2µs** | `[shm] ok` 行（qemu_run.md §20） |

### 7.2 阈值对照（蓝图 §6.3/§7.2）

| 蓝图目标 | 口径 | 本版本落点 | 判定 |
|---------|------|-----------|------|
| 映射 < 10µs/页（§6.3/§7.2） | **真机 CNTVCT 目标** | QEMU 冒烟判定阈值放宽为 `SHM_LAT_MAX_NS = 1.5ms`（v0.138.0~v0.140.0 先例：TCG 翻译冷启动/CNTVCT 虚拟计数器递送开销为已记录坑点）；实测 avg=2.3µs 已显著优于 10µs/页真机目标，max=197.2µs 距阈值余量 ≈7.6× | ✅ 首轮 PASS，阈值零调整 |
| 数据传输 1GB/s 级（§6.3） | 大块共享内存吞吐目标 | 本冒烟 4 页规模不覆盖吞吐测量；零拷贝成立（SHI1 同帧 + 内容双判定）即吞吐路径无中间拷贝，1GB/s 留待真机大块 shm 实测 | 口径记录，非本版本判定项 |

**CNTVCT 精度坑点**（同 v0.138.0 wcet 段 / v0.139.0 IPC 段 / v0.140.0 notify 段先例）：QEMU TCG 纯软件仿真下 CNTVCT 虚拟计数器的读取本身带仿真递送开销，单样本 max 尖峰系翻译块冷启动（avg/max 相差 85×，形态与 IPC 段 `avg=6916 max=526608`、notify 段 `avg=2646 max=224272` 一致），非共享内存机制退化；真机验收（飞腾/鲲鹏）以 §6.3/§7.2 同口径重测为准，QEMU 数值仅作功能验证记录（蓝图 §8.5 坑点对照口径）。

## 8. 测试矩阵

| 编号 | 类型 | 内容 | 蓝图锚点 |
|------|------|------|---------|
| SHM1~SHM3 | lib 单测 | SharedMemory 七字段构造 + 常量值（128/64MB/16）+ KernelError 九变体互异含 AlreadyMapped=9 | §3/§4.1/§4.4 |
| SHF1~SHF5 | lib 单测 | 页数换算边界（0/1B/4095/4096/64MB/64MB+1）+ alloc/free 连续帧游往返 + 双分区同帧 translate + unmap 后归 None + sync_frames host no-op | §4.4/§6.1 |
| SGM1~SGM9 | lib 单测 | create 槽满/超限/零长校验 + owner 隐式 RW + grant 后他分区映射 + 越权注入双路径 + 重复映射 AlreadyMapped + unmap 幂等 + destroy BadState/帧零泄漏 + sync 校验 + stats 八计数全核算 | §4.4/§6.1/§9.6 |
| SHI1 | 集成测试 | **零拷贝验证**（§6.2）：owner A 创建 4 页 → grant B RO → A map RW@va1、B map RO@va2 → 双侧 translate 逐页同帧 + 帧直访图样写入读出逐字节一致 | §6.2 原样 |
| SHI2 | 集成测试 | **notification 配合生产者-消费者**（§5.5）：A 写帧后 signal(0x1) → B wait 命中即返 → B 读帧校验图样 | §5.5 原样 |
| SHI3 | 集成测试 | 越权注入（§6.5）：未授权 map→PermissionDenied + RO 授权 RW 请求→PermissionDenied + 零副作用 | §6.5/§7.3 |
| SHI4 | 集成测试 | 128 槽创建耗尽 → OutOfMemory | §4.4 |
| SHI5 | 集成测试 | 多映射登记：owner+2 授权分区三映射共存、mappings 计数、unmap 其一余者不动 | §9.5 |
| SHI6 | 集成测试 | destroy 回收：unmap 全量 → destroy → PMM free_pages_count 精确复原（帧零泄漏） | §9.4 |
| SHI7 | 集成测试 | 性能 smoke：100 轮 map/unmap 配对 host 注入时钟 eprintln（§6.3 对照口径，不设硬断言） | §6.3 |
| SHI8 | 集成测试 `#[ignore]` | 故障注入：caps 满 16 连 grant 第 17 条 → OutOfMemory | §6.5 |
| shm_smoke | QEMU 冒烟 | 4 页 shm 创建 → 同帧双 VA 映射零拷贝图样校验 → sync_shm → unmap/destroy 帧回收 → 100 轮 map/unmap CNTVCT avg/max 判定 → `[shm] ok` | §6.2/§6.3 目标机落点 |

回归（蓝图 §6.4「IPC/notification 不受影响」）：`cargo test -p eneros-microkernel` 全量零回归；QEMU 实测既有十三类关键串（`[boot]`~`[notify]`）全中（qemu_run.md §20 串口全文）。

## 9. 安全与可靠性核对（蓝图 §7.3/§9.3/§9.4）

| 要求 | 落点 | 锚定 |
|------|------|------|
| 无映射权限被拒（§7.3/§9.3 能力控制映射） | D17 授权表双拒绝路径（未授权/RO 授权 RW 请求）+ `stats.denied` 计数 | SGM4/SHI3 |
| 销毁时安全回收（§9.4） | mappings 非空 → BadState；全量 unmap 后整段帧游 `free_pages` 归还 | SGM7/SHI6 |
| 映射数/页数统计（§9.6 可观测） | `ShmStats` 八字段 saturating 累加（maps/unmaps/pages_mapped 覆盖映射数与页数） | SGM9 |
| 只读/读写多种权限（§9.7 可扩展） | `ShmCap { readable, writable }` 按位授权 + `ShmMapping.writable` 按位落 PTE（AP[2] 只读位） | SHI1 RO/RW 双映射 |

## 10. D15~D20 偏差表（全局）

| 编号 | 蓝图口径 | 实际决策 | 理由 |
|------|---------|---------|------|
| D15 | `frames: Vec<PhysFrameNum>` 逐页分配 | 连续帧游 `(start, page_count)` 经 `PhysMemManager::alloc_pages` | no_std 无 alloc（同 D2/D9）；buddy 连续分配原生支持；分配失败零帧泄漏；O(1) 帧号存储 |
| D16 | `mappings: Vec<(PartitionId, VirtAddr)>` | 定长 `[Option<ShmMapping>; 16]`（cap = MAX_PARTITIONS，每分区至多一条） | no_std 无堆；有界防 BSS 爆炸 |
| D17 | §4.5「owner 可映射，其他需授权」无授权接口 | 每 shm 定长授权表 caps（cap 16）+ owner 隐式 RW + `grant_shm` 补充方法；map 校验：无授权→PermissionDenied、RO 授权 RW 请求→PermissionDenied、同分区重复→AlreadyMapped | 蓝图接口缺失的最小补全；v0.142.0 CSpace 接管预留语义同构形态 |
| D18 | §4.4 `AlreadyMapped` 文字口径 | KernelError 追加第九变体 `AlreadyMapped = 9`（error.rs 规则「新增必须追加」；ERR1/SHM3 同步） | 错误处理落码 |
| D19 | sync_shm 内联 asm | mem/asm/aarch64.rs 助手 `dc_cvau/ic_ivau/dsb_ish/isb`（逐 VA 步进；host no-op 桩） | 助手化同既有 enable_mmu/tlbi_va 口径；跨架构退化预留（§6 x86 注） |
| D20 | `part_mgr.map_shared_page` / `unmap_page`（不存在） | PartitionManager 追加 `map_partition_frame / unmap_partition_frame / translate_partition_frame`（复用页表遍历、跳过帧分配与配额扣减、不登记 allowed_regions；既有方法零改动） | 适配 v0.132.0 既有设施；共享帧生命周期属 ShmManager 非分区 |

## 11. 源码链接

| 项 | 路径 |
|----|------|
| 共享内存对象与管理器 | [crates/kernel/microkernel/src/ipc/shm.rs](../../crates/kernel/microkernel/src/ipc/shm.rs) |
| 共享页帧管理 | [crates/kernel/microkernel/src/ipc/shm_frame.rs](../../crates/kernel/microkernel/src/ipc/shm_frame.rs) |
| 模块挂载与 D15~D20 头注 | [crates/kernel/microkernel/src/ipc/mod.rs](../../crates/kernel/microkernel/src/ipc/mod.rs) |
| KernelError::AlreadyMapped | [crates/kernel/microkernel/src/error.rs](../../crates/kernel/microkernel/src/error.rs) |
| PartitionManager 三方法 | [crates/kernel/microkernel/src/mem/partition.rs](../../crates/kernel/microkernel/src/mem/partition.rs) |
| ARM64 缓存维护助手 | [crates/kernel/microkernel/src/mem/asm/aarch64.rs](../../crates/kernel/microkernel/src/mem/asm/aarch64.rs) |
| host 集成测试 | [crates/kernel/microkernel/tests/shm_test.rs](../../crates/kernel/microkernel/tests/shm_test.rs) |
| QEMU 冒烟段 | [crates/kernel/microkernel/src/main.rs](../../crates/kernel/microkernel/src/main.rs) `shm_smoke` |
| QEMU 实测记录 | [../boot/qemu_run.md](../boot/qemu_run.md) §20 |
