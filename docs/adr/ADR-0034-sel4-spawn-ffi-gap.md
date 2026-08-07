# ADR-0034：G-KERNEL K1.2 真实 spawn 的 seL4 FFI 缺口补齐

- 状态：已接受（2026-08-08，21-A K1.2 设计阶段）
- 关联：蓝图/21-A §2.K1.2；ADR-0023（seL4 基线）；ADR-0025（IPC wire）
- 背景蓝图：`蓝图/21-A-内核地基-G-KERNEL闸门-裸机OS真闭环开发蓝图.md`

## 问题

21-A §2.0 评估称"seL4 FFI 声明（ffi.rs，27 个 extern）✅ 完整……直接调用，**不新增 FFI**"。
K1.2 设计阶段以 `third_party/sel4-build/top/kernel/kernel_all.c`（seL4 14.0.0 预处理内核源码）逐项核对
spawn 序列后确认：**该评估不成立**。按 PROJECT.md 第一性原则 5（单一事实源：代码为准），
补齐缺口并记本 ADR，不改代码迁就蓝图。

## 事实依据（内核源码）

| 缺口 | 内核证据 | 缺它会发生什么 |
|---|---|---|
| `seL4_TCB_SetSpace`（CSpace+VSpace+IPC buffer 绑定） | TCB 创建后无 CSpace/VSpace 能力槽，恢复运行即 CapFault | spawn 的 TCB 无法运行 |
| `seL4_TCB_SetIPCBuffer` | 消息长度 > 寄存器 MR（aarch64 4 words）必须落 IPC buffer；无 buffer 的线程 IPC 即 fault | K2 的 ipc-wire 帧（≥5 words）无法收发 |
| `seL4_ARM_ASIDPoolAssign` | `arm_page_map` 走 `findVSpaceForASID(asid)`；VSpace cap 未绑 ASID 时 `capVSMappedASID=0` → FailedLookup | 服务 VSpace 无法映射任何页 |
| `seL4_ARM_PageUnmap` | `decodeARMFrameInvocation/ARMPageMap`：`capFMappedASID != 0` 且新 vaddr ≠ cap 记录地址 → `InvalidArgument`（frame 只能映射一个地址） | K1.3 方案 A（临时窗口拷贝）无法解除窗口映射，同一 frame 无法再映射进服务 VSpace |
| root-task 固定 cap 常量 getter（InitThreadTCB/CNode/VSpace/ASIDPool） | bootinfo.h 编译期宏；Rust 侧硬编码有版本漂移风险 | 拿不到 root CSpace/VSpace/ASID pool 的 cap |

## 决策

1. **补齐 5 类 FFI 封装**（shim C 真封装 → ffi.rs 声明 → safe.rs 封装，全部真调 seL4 API）：
   - `eneros_sel4_tcb_set_space` / `eneros_sel4_tcb_set_ipcbuffer`
   - `eneros_sel4_arm_asid_pool_assign`
   - `eneros_sel4_arm_page_unmap`
   - `eneros_sel4_cap_init_thread_*` 常量 getter（tcb/cnode/vspace/asid_pool）
2. **host 构建一律返回 `Sel4Error::HostUnsupported`**（21-A 红线 2，与 K1.1 的
   `arm_page_map`/`arm_page_table_map` 一致；区别于既有 retype/tcb_* 的 Ok stub）。
3. **aarch64 4 级页表模型**（`lookupPTSlot`：ptBitsLeft 39→30→21→12）：4KiB 页映射需
   L1+L2+L3 三级页表对象；`arm_page_table_map` 按 lookup 层级自动定级。每服务 3 个
   PageTable 对象（服务地址区域 0x4000_0000），root 临时窗口 2 个（0x8000_0000，
   L0/L1 已由 elfloader 占用）。
4. **K1.3 方案 A 流程修正**（frame 单地址映射证据）：retype → 窗口映射 → 拷贝 → `PageUnmap` →
   服务 VSpace 映射。窗口页表只建一次，窗口固定 0x8000_0000（1 GiB 区域）。

## 影响

- `crates/runtime/sel4-sys`：shim/sel4_shim.{c,h}、src/ffi.rs、src/safe.rs 各 +若干声明/封装。
- `crates/runtime/root-task`：依赖 eneros-sel4-sys；spawn.rs 实现 untyped 分配器 +
  真实 spawn 序列；main.rs 接线 BootInfo。
- 与 21-A §2.0"不新增 FFI"表述冲突：**以代码为准，本 ADR 为单一事实源**。
  建议在 21-A 下一修订中更新 §2.0 资产表。
- K2.1（IPC 封装）不受影响；K2.2/K2.3 依赖本 ADR 的 SetIPCBuffer/SetSpace。

## 备选（否决）

- TCB_Configure 替代 SetSpace+SetIPCBuffer：功能等价但参数耦合（含 priority/affinity），
  与 21-A 序列的"分步调用"不符，且 safe 封装粒度更细利于错误定位。
- 放弃 ASID、复用 root 的 ASID pool：服务 VSpace 与 root 同 ASID 会共享 TLB 语义，破坏地址空间隔离判据。
- frame 双映射（窗口 + 服务）免 unmap：内核源码明确拒绝，不可行。
