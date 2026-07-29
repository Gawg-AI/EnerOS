#![no_std]

//! # eneros-microkernel
//!
//! 自研 Rust 微内核顶层架构设计 + syscall 接口规范（Phase 3 首版 v0.127.0）。
//!
//! ## 用途
//!
//! 本 crate 为 Phase 3 自研内核的架构骨架版本，仅包含 trait 与类型定义，
//! 不包含任何实现主体。它定义了微内核的核心抽象：错误码模型、启动信息
//! 模型、内核状态/版本模型，以及后续版本（Task 2/3 起）将填充的 syscall
//! 接口规范与子系统注册框架。
//!
//! ## ADR-0001 定位
//!
//! 依据 ADR-0001，Phase 3 主路径采用 seL4 深度定制（方案 A）。本 crate 作为
//! 该路径的架构设计输入与 fallback 参考：若 seL4 路径在信创认证或深度定制
//! 验证中受阻，本架构设计可作为自研微内核的启动基线；同时作为信创认证
//! 架构设计文档的代码化输入。
//!
//! ## 交付物清单（v0.127.0）
//!
//! - 错误码模型 [`error::KernelError`]（8 变体，repr(u16)）
//! - 启动信息模型 [`boot::BootInfo`]
//! - 内核状态模型 [`state::KernelState`]（七句柄占位）
//! - 版本模型 [`state::KernelVersion`] / [`state::KERNEL_VERSION`]
//! - syscall 接口规范模块 [`syscall`]（Task 2 填充）
//! - 子系统注册框架 [`registry`]（Task 3 填充）
//! - 物理内存管理 [`mem`]（v0.130.0：buddy 页帧分配器 + Zone 划分）
//! - IPC 子系统 [`ipc`]（v0.139.0：endpoint + 消息寄存器 + 同步 send/recv/call/reply）
//! - 通知子系统 [`ipc::notification`]（v0.140.0：notification + 异步信号）
//! - 共享内存 [`ipc::shm`]（v0.141.0：共享内存对象 + 映射/授权表骨架）
//! - HAL 子系统 [`hal`]（v0.143.0：Hal trait 十一方法 + HalRegistry + Aarch64Hal ARM64 实现）
//! - x86_64 BSP 占位（v0.144.0：[`hal::x86_64`] X86Hal + [`hal::apic`] LocalApic + [`mem::x86_page_table`] X86PageTable——ADR-0001 降级口径接口占位 + host 可测骨架）
//! - 飞腾 D2000 BSP（v0.145.0：[`hal::phytium`] PhytiumHal 组合 Aarch64Hal + PhytiumSmmu + PhytiumCrypto + [`boot::dtb_find_node`] DTB 节点查找——★ 信创核心交付，真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径）
//! - 鲲鹏 920 BSP（v0.146.0：[`hal::kunpeng`] KunpengHal 组合 Aarch64Hal + KunpengSmmu + KunpengNuma + [`boot::dtb_count_nodes`] DTB 节点计数——★ 信创核心交付 双国产平台，真机路径保留 + host 可测骨架 + 真机验证留待硬件三层口径）
//! - RISC-V + 龙芯 BSP（v0.147.0：[`hal::riscv`] RiscvHal standalone + Plic + Clint + UartSifive + [`hal::loongarch`] LoongArchHal standalone + LoongsonPch（复用 [`hal::x86_64::Uart8250`]，D4 跨平台 UART 复用）——P3-E HAL 多平台线终版本，ADR-0001 BSP 收敛降级口径接口占位（同 v0.144.0 X86Hal，**非** 三层口径），不保留 cfg 门控真机寄存器写路径 + 不做真机验证；PPI7 跨架构六平台多态验证 Aarch64Hal + X86Hal + PhytiumHal + KunpengHal + RiscvHal + LoongArchHal 同槽分发）
//!
//! ## 蓝图偏差声明（D1~D7）
//!
//! - D1：crate 落位 `kernel/` 根目录 → `crates/kernel/microkernel/`，包名
//!   eneros-microkernel（仓库规范 §2.3.1 强制子系统归口；`eneros-kernel`
//!   已被 Phase 0 占位 crate 占用）
//! - D2：lib.rs 内定义 `#[panic_handler]` → 不定义（lib crate 全局
//!   panic_handler 与 host `cargo test` 及 workspace 其他二进制链接冲突；
//!   蓝图 §8.5 针对最终内核二进制产物，本版本为接口骨架，归属说明写入
//!   architecture.md）
//! - D3：`SubsystemRegistry` 存储 `&'static dyn` + unsafe const→mut cast
//!   （UB 风险）→ 存储 `&'static mut dyn KernelSubsystem`（启动期单线程
//!   注册语义安全，零 unsafe）
//! - D4：`assert!(idx < 8)` panic / init 失败 panic → 统一返回
//!   `Result<(), KernelError>`（仓库禁 panic! 规则；蓝图 §4.4 panic 路径
//!   语义改为短路错误传播，故障注入测试验证短路而非 panic）
//! - D5：`env!("GIT_HASH")`/`env!("BUILD_TIME")` 需 build.rs 注入 →
//!   `option_env!` + "unknown" 回退（骨架 crate 免构建脚本依赖；构建信息
//!   注入属 v0.128.0 构建系统版本范畴）
//! - D6：`#![feature(inline_asm, naked_functions)]` + naked `_start` 桩 →
//!   不启用 nightly feature gate（asm! 已稳定；`_start`/trampoline 无实现
//!   主体属 v0.129.0 Bootloader 版本范畴，于 architecture.md 预留接口说明）；
//!   `KernelState` 七句柄以单元结构体占位
//! - D7：蓝图 syscall ABI 述及"ARM64 用 `svc #0`，x86 用 `syscall`"（§5.2）
//!   与"用户态通过 `ecall` 陷入"（§4.2）表述矛盾 → 以 §5.2 为准（ecall
//!   为 RISC-V 指令，x86_64 仅留接口占位属 ADR-0001 裁剪项），于
//!   syscall_spec.md 澄清

pub mod boot;
pub mod cap;
pub mod compat;
pub mod error;
pub mod hal;
pub mod interrupt;
pub mod ipc;
pub mod layout;
pub mod mem;
pub mod registry;
pub mod sched;
pub mod smp;
pub mod state;
pub mod sync;
pub mod syscall;

pub use boot::BootInfo;
pub use compat::{
    CPtr, CSpaceMapper, CompatError, CompatStats, KernelSyscallOps, MessageInfo, Rights,
    Sel4Compat, Sel4CompatLayer, Sel4Shim, SyscallTranslator,
};
pub use error::KernelError;
pub use layout::{BuildConfig, KernelImageLayout, OptLevel};
pub use registry::{KernelSubsystem, SubsystemRegistry};
pub use state::{
    BootstrapHandle, CapabilityHandle, HalHandle, IpcHandle, KernelState, KernelVersion,
    MemoryHandle, SchedulerHandle, TimeHandle, KERNEL_VERSION,
};
pub use syscall::{SyscallHandler, SyscallNum};
