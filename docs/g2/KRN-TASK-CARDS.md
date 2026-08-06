# EnerOS G2 工作包 Task Cards（KRN-001~KRN-024, SVC-001~SVC-007）

> 依据：MASTER-真实OS与AgentOS商业交付总蓝图.md（G2 / G3 前置部分）
> 平台：qemu-prod（QEMU virt aarch64，参考平台）；L3 真机证据统一延后至 G10。
> 证据规则：README/注释/版本号/crate 存在/Mock/日志关键词不作为交付证据。

## 总体设计决策（ADR-G2-001：地址空间与内核布局）

- 内核采用“每进程根页表 + TTBR0 切换 + ASID”的硬件隔离模型。每个进程拥有
  独立根页表（VSpace），内核固定映射（text RX / rodata RO-NX / data RW-NX /
  设备 Device-nGnRnE / 其余 RAM RW-NX，全范围无 W+X）随根页表保持 EL1-only；
  用户映射仅存在于本进程根页表。上下文切换真实写 TTBR0_EL1（BADDR|ASID）
  并执行 TLBI ASID；跨核映射撤销走 SGI IPI shootdown 协议。
- 本设计相对 02 分卷“TTBR0 用户 / TTBR1 内核”的差异：镜像工厂与 G1 启动链
  契约要求内核 ELF VMA==PMA（ET_EXEC 恒等装载），TTBR1 高半内核需要重写
  全部 PMM/页表/MMIO 的物理寻址并引入启动期双重映射。MASTER 的 G2 硬性
  判据（真实页表/TTBR/ASID 隔离、无 identity RWX RAM、W^X、guard/null 页）
  在本设计中全部满足；差异记录于此并在最终报告标注。
- 异常模型：完整 16 槽向量；EL0→EL1 异常把全部 GPR + SP_EL0 + ELR/SPSR +
  TPIDR_EL0 + ESR/FAR + FP/SIMD（q0~q31/FPCR/FPSR）保存到线程内核栈顶
  exception frame；阻塞/抢占即“保存帧 + 切换 SP + eret”，真实调度器驱动。
- Syscall ABI：唯一来源 crate `eneros-abi-syscall`（ID/错误码/布局常量），
  内核与 libeneros 共同依赖；`svc #0`，x8=ID、x0..x9=参数、x0=状态、
  其余寄存器按调用语义回传。版本协商 syscall 0。
- 用户 ELF：v1 固定基址静态 ET_EXEC（构建期固定方案），loader 拒绝
  PT_DYNAMIC/PT_INTERP/任何 relocation；entry 必须在 RX 段内。
- 阻塞 IPC：endpoint 有界队列 + 线程 wait 队列 + 定时器超时队列；阻塞线程
  不再返回用户态，帧保存于内核栈，由调度器唤醒后恢复并返回结果。

## KRN 工作包

```yaml
id: KRN-001
objective: 缩减生产 kernel entry：入口只做 BootInfo 验证、页表、CPU/异常、
  PMM/VMM、调度器、root task 装载；smoke 移出生产二进制。
depends_on: [G1 BOOT-006/BOOT-002]
owned_paths: [crates/kernel/microkernel/src/main.rs, crates/kernel/microkernel/src/smoke_main.rs]
system_evidence: [bootinfo-validated, kernel-init-complete]
forbidden_shortcuts: [smoke-in-entry, self-built-bootinfo, final-wfe]
rollback: 保留 kernel-smoke 独立 bin；生产入口不引用 smoke 模块
---
id: KRN-002
objective: 最终内核页表：按段 RX/RO/RW+NX，设备 Device-nGnRnE，无全 RAM RWX identity。
depends_on: [KRN-001]
owned_paths: [microkernel/src/prod/mmu.rs, mem/page_table.rs]
system_evidence: [mmu-map-pass, wx-scan-pass]
forbidden_shortcuts: [KERNEL_RW_X-over-ram, software-range-check]
---
id: KRN-003
objective: 完整 EL0 异常上下文：x0~x30、SP_EL0、ELR/SPSR、TPIDR_EL0、q0~q31/FPCR/FPSR。
depends_on: [KRN-001]
owned_paths: [microkernel/src/prod/vectors.rs, microkernel/src/prod/frame.rs]
system_evidence: [el0-enter, fault-frame, fpu-roundtrip]
---
id: KRN-004
objective: SVC entry/exit：ESR 识别 SVC、参数从帧取、返回写帧、可抢占返回。
depends_on: [KRN-003]
owned_paths: [microkernel/src/prod/syscall.rs]
system_evidence: [svc-call, svc-unknown-id-rejected]
---
id: KRN-005
objective: user copy primitives：copy_from_user/copy_to_user，逐页验证 + fault 恢复。
depends_on: [KRN-002, KRN-003]
owned_paths: [microkernel/src/prod/user_mem.rs]
system_evidence: [bad-pointer-rejected]
---
id: KRN-006
objective: 进程对象与生命周期：PID 代际、create/exit/wait/reap、资源回收。
depends_on: [KRN-002, KRN-003]
owned_paths: [microkernel/src/prod/process.rs]
system_evidence: [process-create, process-exit-reclaim]
---
id: KRN-007
objective: 每进程 VSpace/ASID：独立根页表、同 VA 不同物理页、硬件隔离。
depends_on: [KRN-006]
owned_paths: [microkernel/src/prod/mmu.rs, microkernel/src/prod/process.rs]
system_evidence: [isolation-cross-process]
---
id: KRN-008
objective: TTBR 切换与跨核 TLB shootdown：切换写 TTBR0+ASID+TLBI；SGI 协议。
depends_on: [KRN-007, KRN-019-ipi-primitive]
owned_paths: [microkernel/src/prod/tlb.rs, microkernel/src/prod/smp.rs]
system_evidence: [ttbr-switch, shootdown-pass]
---
id: KRN-009
objective: W^X / guard page / null page：执行数据、写代码、null 解引用全部硬件 fault。
depends_on: [KRN-007]
owned_paths: [microkernel/src/prod/mmu.rs, microkernel/src/prod/loader.rs]
system_evidence: [wx-execute-data-fault, wx-write-code-fault, null-page-fault]
---
id: KRN-010
objective: ELF64 解析器：header/PHDR/溢出/重叠/对齐/入口校验 + 恶意 corpus。
depends_on: [KRN-006]
owned_paths: [microkernel/src/prod/elf.rs]
negative_tests: [truncated, overlapping, huge, wrong-machine, wx-segment]
---
id: KRN-011
objective: ELF 段装载：逐 PT_LOAD 复制、BSS 清零、权限映射、失败回滚。
depends_on: [KRN-010, KRN-007]
owned_paths: [microkernel/src/prod/loader.rs]
system_evidence: [root-task-loaded, service-loaded]
---
id: KRN-012
objective: PIE/relocation 策略：v1 固定基址静态 ET_EXEC；拒绝 PT_DYNAMIC/reloc。
depends_on: [KRN-011]
owned_paths: [microkernel/src/prod/elf.rs]
---
id: KRN-013
objective: 从签名 init bundle 启动 root task 进入 EL0，内核不链接业务实现。
depends_on: [KRN-011, KRN-003]
owned_paths: [microkernel/src/prod/boot.rs, crates/runtime/root-task]
system_evidence: [root-task-el0]
---
id: KRN-014
objective: versioned syscall ABI crate：固定编号/错误码/版本协商/元数据。
depends_on: [KRN-004]
owned_paths: [crates/abi/syscall]
system_evidence: [abi-version-query]
---
id: KRN-015
objective: 真实阻塞 IPC call/reply/cancel：调度器驱动、超时、进程退出清理。
depends_on: [KRN-004, KRN-017-preemption]
owned_paths: [microkernel/src/prod/ipc.rs]
system_evidence: [ipc-call-reply, ipc-timeout, ipc-cancel-on-exit]
---
id: KRN-016
objective: capability mint/transfer/revoke：派生树、立即失效、无放大。
depends_on: [KRN-006]
owned_paths: [microkernel/src/prod/caps.rs]
system_evidence: [cap-mint, cap-revoke-immediate]
---
id: KRN-017
objective: 抢占式调度：timer IRQ 抢占非 WFI 线程、优先级队列、时间片。
depends_on: [KRN-003]
owned_paths: [microkernel/src/prod/sched.rs]
system_evidence: [preemption-switch-count]
---
id: KRN-018
objective: priority inheritance / budget：IPC 链提升、CPU 预算、超额拒绝。
depends_on: [KRN-017, KRN-015]
owned_paths: [microkernel/src/prod/sched.rs]
system_evidence: [prio-inherit, budget-reject]
---
id: KRN-019
objective: SMP per-CPU / IPI / GIC：次核启动、per-CPU 栈/状态、SGI IPI、shootdown。
depends_on: [KRN-017]
owned_paths: [microkernel/src/prod/smp.rs]
system_evidence: [secondary-online, ipi-rx]
---
id: KRN-020
objective: IRQ ownership / storm control：中断归属 capability、速率限制、隔离。
depends_on: [KRN-019]
owned_paths: [microkernel/src/prod/irq.rs]
system_evidence: [irq-storm-rate-limited]
---
id: KRN-021
objective: DMA/IOMMU 原语：pin/方向/域/撤销契约（QEMU virt 无 SMMU 时以域记账 + 契约测试）。
depends_on: [KRN-006]
owned_paths: [microkernel/src/prod/dma.rs]
---
id: KRN-022
objective: OOM / 资源记账：帧/线程/cap/endpoint 配额，关键池保留，拒绝而非 panic。
depends_on: [KRN-006, KRN-016]
owned_paths: [microkernel/src/prod/process.rs, microkernel/src/prod/resource.rs]
system_evidence: [oom-rejected]
---
id: KRN-023
objective: 结构化 panic / crash dump：build id、CPU、ESR/ELR/FAR、任务、最近事件。
depends_on: [KRN-003]
owned_paths: [microkernel/src/prod/panic.rs]
system_evidence: [crash-record-structured]
---
id: KRN-024
objective: watchdog / panic rendezvous：panic 停其他核、记录、PSCI 复位。
depends_on: [KRN-023, KRN-019]
owned_paths: [microkernel/src/prod/panic.rs]
system_evidence: [panic-reset]
```

## SVC 工作包（G2 前置部分）

```yaml
id: SVC-001
objective: crt0/libeneros 最小 runtime：_start、svc veneer、字符串/格式化最小集。
depends_on: [KRN-014]
owned_paths: [crates/runtime/libeneros]
system_evidence: [svc-call-from-libeneros]
---
id: SVC-002
objective: init manifest schema：签名 init bundle 的模块表（kind/name/hash/offset/len）。
depends_on: [KRN-013]
owned_paths: [crates/abi/syscall（bundle schema）, xtask/src/image_factory.rs]
system_evidence: [root-task-parses-manifest]
---
id: SVC-003
objective: root task process manager：创建/停止/回收服务进程。
depends_on: [SVC-002, KRN-013]
owned_paths: [crates/runtime/root-task]
system_evidence: [svc-create, svc-reap]
---
id: SVC-004
objective: service dependency DAG：循环拒绝、ready 顺序启动。
depends_on: [SVC-003]
owned_paths: [crates/runtime/root-task]
system_evidence: [dag-acyclic, dag-ready-order]
---
id: SVC-005
objective: health/readiness 协议：heartbeat/readiness IPC、timeout、backoff。
depends_on: [SVC-003, KRN-015]
owned_paths: [crates/runtime/root-task]
system_evidence: [health-ready, health-timeout-restart]
---
id: SVC-006
objective: crash-loop quarantine：连续失败计数、指数退避、quarantine 状态。
depends_on: [SVC-005]
owned_paths: [crates/runtime/root-task]
system_evidence: [quarantine-after-crashes]
---
id: SVC-007
objective: nameservice / endpoint discovery：注册/查找、capability 安全传递。
depends_on: [KRN-016, SVC-003]
owned_paths: [microkernel/src/prod/ns.rs]
system_evidence: [nameservice-register-lookup]
```

## G2 出口验收映射（MASTER G2 Exit criteria）

| MASTER 判据 | 证据 |
|---|---|
| 两个 EL0 地址空间真实隔离；恶意读写/执行被硬件 fault | isolation-cross-process / wx-* / null-page-fault |
| root task 从签名 init bundle 启动并通过真实 syscalls 创建第二进程 | root-task-el0 / svc-create |
| 恶意进程不能 panic kernel；资源与 capability 完整回收 | fault-isolation / oom-rejected / cap-revoke-immediate |
| 抢占、IPC、FP/SIMD、SMP 压力长期无状态损坏 | preemption-switch-count / ipc-call-reply / fpu-roundtrip / shootdown-pass |
| kernel binary 无业务/协议/Agent/AI 逻辑 | 生产闭包：microkernel 依赖仅 boot-protocol/abi/early-platform；root task/服务为独立 EL0 ELF |
