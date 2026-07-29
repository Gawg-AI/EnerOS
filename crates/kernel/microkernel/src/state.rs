//! 内核状态与版本模型（蓝图 §4.1）。
//!
//! [`KernelState`] 聚合七大子系统句柄，为内核全局状态的根对象。
//! 本版本中七句柄均为单元结构体占位（偏差 D6），实际实现属后续版本。
//!
//! v0.146.0 交付物（鲲鹏 920 BSP — Phase 3 P3-E 第 4 版 ★ 信创核心 双国产平台）：
//! KunpengHal / KunpengSmmu / KunpengNuma / boot::dtb_count_nodes。
//!
//! v0.147.0 交付物（RISC-V + 龙芯 BSP — Phase 3 P3-E 终版本 接口占位）：
//! RiscvHal / Plic / Clint / UartSifive / LoongArchHal / LoongsonPch。
//!
//! v0.148.0 交付物（形式化规范框架搭建 — Phase 3 P3-F 首版本）：
//! formal/Isabelle/Kernel.thy（抽象规范）+ formal/Isabelle/Refinement.thy（精化映射）
//! + formal/ROOT + formal/Makefile + formal/README.md + docs/formal/framework.md
//! + tests/formal_framework_test.rs（F1~F6 host 可测骨架）。
//!
//! v0.149.0 交付物（调度器规范抽象 — Phase 3 P3-F 第 2 版本）：Scheduler.thy 优先级抢占调度规范抽象（sched_step/inv_priority/liv_fair + sorry 占位延后 v0.150.0）
//!
//! v0.150.0 交付物（调度器形式化证明 ★ — Phase 3 P3-F 第 3 版本 第一个形式化证明里程碑）：Scheduler_Proof.thy（scheduler_safety 完整归纳证明 rtranclp.induct + sched_step.induct 无 sorry + scheduler_liveness sorry 占位需公平性假设 + ready_schedulable 辅助引理 D8 风险标注）+ ROOT 一行新增 + README 追加 + tests/scheduler_proof_test.rs（F1~F5 host 可测骨架）+ docs/formal/scheduler_proof.md（11 章证明设计文档）——ADR-0001 保留项 #1 调度器证明
//! v0.151.0 交付物（内存隔离规范抽象 — Phase 3 P3-F 第 4 版本 ADR-0001 保留项 #2）：Memory.thy 内存隔离规范抽象（vaddr 类型 D2 + partition/mem_state record D4 + accessible 集合并 comprehension D5 + inv_isolation 跨分区不相交不变式 D6 hide_const 遮蔽 + add_region 辅助函数 D3 + map_page 映射操作 + map_preserves_isolation sorry 占位延后 v0.152.0 D7）+ ROOT 一行新增 + README 追加 + tests/memory_spec_test.rs（F1~F5 host 可测骨架）+ docs/formal/memory_spec.md（11 章规范设计文档）——ADR-0001 保留项 #2 内存隔离证明
//! v0.152.0 交付物（内存隔离形式化证明 ★ — Phase 3 P3-F 第 5 版本 第二个形式化证明里程碑 ADR-0001 保留项 #2）：Memory_Proof.thy 内存隔离形式化证明（isolation_safety rtranclp 完整归纳证明 rtranclp.induct 外层 + map_page.induct 内层双层归纳无 sorry ★ 里程碑 + D2a map_page\<^sup>*\<^sup>* 记法修正 + D2b case (map v s target) 去除多余 s' + D2c IH 直接引用 + D2d/D2e simp 规则补充 add_region_def）+ ROOT 一行新增 + README 追加 + tests/memory_proof_test.rs（F1~F5 host 可测骨架 ★ 无 sorry 验收）+ docs/formal/memory_proof.md（11 章证明设计文档）——ADR-0001 保留项 #2 内存隔离证明 ★ 第二个形式化证明里程碑，D3 Memory.thy sorry 保留不动 Surgical Changes

/// 调度器句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SchedulerHandle;

/// 内存管理句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryHandle;

/// 能力系统句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct CapabilityHandle;

/// IPC 子系统句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct IpcHandle;

/// HAL 句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct HalHandle;

/// 时间子系统句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeHandle;

/// 引导子系统句柄占位（D6：本版本仅占位，实际实现属后续版本）。
#[derive(Debug, Clone, Copy, Default)]
pub struct BootstrapHandle;

/// 内核全局状态（蓝图 §4.1）。
///
/// 聚合七大子系统句柄；构造为 `const fn`，允许以静态变量形式驻留。
#[derive(Debug)]
pub struct KernelState {
    /// 调度器句柄。
    pub scheduler: SchedulerHandle,
    /// 内存管理句柄。
    pub memory: MemoryHandle,
    /// 能力系统句柄。
    pub caps: CapabilityHandle,
    /// IPC 子系统句柄。
    pub ipc: IpcHandle,
    /// HAL 句柄。
    pub hal: HalHandle,
    /// 时间子系统句柄。
    pub time: TimeHandle,
    /// 引导子系统句柄。
    pub bootstrap: BootstrapHandle,
}

impl KernelState {
    /// 构造全占位句柄的内核状态。
    pub const fn new() -> Self {
        Self {
            scheduler: SchedulerHandle,
            memory: MemoryHandle,
            caps: CapabilityHandle,
            ipc: IpcHandle,
            hal: HalHandle,
            time: TimeHandle,
            bootstrap: BootstrapHandle,
        }
    }
}

impl Default for KernelState {
    fn default() -> Self {
        Self::new()
    }
}

/// 内核版本信息。
///
/// `commit` / `build_time` 在编译期注入；骨架版本使用 `option_env!`
/// 并在未注入时回退为 `"unknown"`（偏差 D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelVersion {
    /// 主版本号。
    pub major: u16,
    /// 次版本号。
    pub minor: u16,
    /// 修订号。
    pub patch: u16,
    /// 构建时 Git 提交哈希（未注入时为 `"unknown"`）。
    pub commit: &'static str,
    /// 构建时间戳（未注入时为 `"unknown"`）。
    pub build_time: &'static str,
}

/// 当前内核版本常量（v0.155.0）。
pub const KERNEL_VERSION: KernelVersion = KernelVersion {
    major: 0,
    minor: 155,
    patch: 0,
    commit: match option_env!("GIT_HASH") {
        Some(s) => s,
        None => "unknown",
    },
    build_time: match option_env!("BUILD_TIME") {
        Some(s) => s,
        None => "unknown",
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    /// VER1：KERNEL_VERSION 主/次/修订号为 0.155.0。
    #[test]
    fn ver1_version_numbers() {
        assert_eq!(KERNEL_VERSION.major, 0);
        assert_eq!(KERNEL_VERSION.minor, 155);
        assert_eq!(KERNEL_VERSION.patch, 0);
    }

    /// VER2：commit 与 build_time 非空（未注入时回退 "unknown"）。
    #[test]
    fn ver2_commit_and_build_time_non_empty() {
        assert!(!KERNEL_VERSION.commit.is_empty());
        assert!(!KERNEL_VERSION.build_time.is_empty());
    }

    /// STATE1：KernelState::new() 可构造（const fn）。
    #[test]
    fn state1_kernel_state_new_constructible() {
        const STATE: KernelState = KernelState::new();
        let _ = &STATE.scheduler;
        let _ = &STATE.bootstrap;
    }
}
