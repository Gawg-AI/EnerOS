//! 38（WP-38.4）：root 平台装配——PlatformAdapter/HalRuntime 唯一入口。
//!
//! # 装配链
//! 1. `runtime_dtb_slice`（BOOT-007 窗口，只读）→ [`QemuVirtAdapter`]
//!    `discover`（FdtDescriptionProvider + 板级事实）→ `validate` →
//!    [`ValidatedInventory`]；
//! 2. `build_hal` → [`HalRuntime`]（CPU 拓扑来自 inventory、timer 频率
//!    校验非零、console/entropy 来自 chosen、DMA 运行时来自平台池）；
//! 3. main 用 `hal.timer` 装配 time 子系统（不再直取
//!    `arm64::timer::clock()`）；CPU 数、console/entropy 打印证据行。
//!
//! # 不变量（蓝图 38 WP-38.4）
//! - root 不再直接调用 `arm64::timer::clock` 或 QEMU 常量：生产必须经
//!   adapter 构造 HAL；未初始化访问返回错误/阻断，不回退默认；
//! - console/entropy 缺失影响 capability/readiness（缺 required → 阻断）；
//! - seL4 BootInfo `numNodes` 交叉验证通道（elfloader ABI）未建——当前
//!   以 0（未知）跳过，机制（[`cross_check_cpu_count`]）host 已测；
//!   真机交叉验证登记 42/43。

// 38（WP-38.4 fix-44.3）：装配层依赖 hal/board 的 PlatformAdapter seam
// 类型与 trait 必须显式导入（真实 aarch64 目标构建才编译 `assemble`，
// host check 经 `#[cfg(target_os = "none")]` 跳过——44.3 目标构建暴露）。
// HalRuntime/ValidatedInventory 同时被 host 编译的 RootPlatform 使用，
// 保持无 cfg；仅目标侧装配符号随 assemble 门控。
use eneros_hal::platform_adapter::HalRuntime;
#[cfg(target_os = "none")]
use eneros_hal::platform_adapter::{
    cpu_topology_from, cross_check_cpu_count, PlatformAdapter, PlatformError, PlatformInput,
};
use eneros_hal::ValidatedInventory;

/// 装配失败类型（打印 + fail-closed 挂起由调用方执行）。
pub enum AssemblyError {
    NoDescription,
    ParseFailed,
    InvalidInventory,
    Unsupported(&'static str),
}

impl core::fmt::Debug for AssemblyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AssemblyError::NoDescription => write!(f, "no platform description"),
            AssemblyError::ParseFailed => write!(f, "platform description parse failed"),
            AssemblyError::InvalidInventory => write!(f, "inventory validation failed"),
            AssemblyError::Unsupported(w) => write!(f, "platform unsupported: {w}"),
        }
    }
}

/// 装配产物：已验证 inventory + HAL 运行时。
pub struct RootPlatform {
    pub inventory: ValidatedInventory,
    pub hal: HalRuntime,
}

/// 从运行时 DTB 装配平台（只读；不触发设备写/格式化/业务 I/O）。
#[cfg(target_os = "none")]
pub fn assemble(dtb: &[u8], bootinfo_cpu_count: u32) -> Result<RootPlatform, AssemblyError> {
    // 39 CVR-39-06：DMA VA→PA 偏移运行时推导——root 镜像物理基址
    // （userImageFrames 首个 cap）减去镜像链接基址 0x200000。
    // rootserver 加载地址随内核/镜像尺寸漂移，禁止硬编码。
    let image_phys = eneros_sel4_sys::safe::user_image_phys_base();
    if image_phys != 0 {
        let offset = image_phys.saturating_sub(0x200_000);
        eneros_hal::dma::set_identity_offset(offset);
    }
    let adapter = eneros_board::adapter::QemuVirtAdapter::new();
    let input = PlatformInput {
        runtime_dtb: Some(dtb),
        bootinfo_cpu_count,
    };
    let validated = adapter.discover(&input).map_err(|e| match e {
        PlatformError::NoDescription => AssemblyError::NoDescription,
        PlatformError::ParseFailed => AssemblyError::ParseFailed,
        PlatformError::InvalidInventory => AssemblyError::InvalidInventory,
        PlatformError::Unsupported => AssemblyError::Unsupported("unknown platform"),
        PlatformError::HalBuildFailed { .. } => AssemblyError::Unsupported("hal build failed"),
    })?;
    // CPU 交叉验证（bootinfo=0 未知跳过；不一致 → Unsupported fail-closed）。
    let topo = eneros_hal::platform_adapter::cpu_topology_from(&validated.inventory)
        .map_err(|_| AssemblyError::Unsupported("no enabled cpu"))?;
    cross_check_cpu_count(&topo, bootinfo_cpu_count)
        .map_err(|_| AssemblyError::Unsupported("cpu count mismatch"))?;
    let hal = adapter
        .build_hal(&validated)
        .map_err(|_| AssemblyError::Unsupported("hal build failed"))?;
    Ok(RootPlatform {
        inventory: validated,
        hal,
    })
}
