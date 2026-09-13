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
#[cfg(target_os = "none")]
use alloc::string::String;

use eneros_hal::platform_adapter::HalRuntime;
#[cfg(target_os = "none")]
use eneros_hal::platform_adapter::{
    cpu_topology_from, cross_check_cpu_count, PlatformError, PlatformInput,
};
use eneros_hal::ValidatedInventory;

/// 70.12（WP-70.12.2/3）：板级装配选择（注册表 + 描述绑定）——替换原
/// 硬编码 `QemuVirtAdapter`。声明了 SKU 但未注册 adapter → 拒绝，
/// **不回退**开发平台（"运时猜 compatible 再兜底 QEMU"的反面）。
#[cfg(target_os = "none")]
use crate::board_select::{BoardSelectError, RootBoardRegistry, VouchLine};

/// 装配失败类型（打印 + fail-closed 挂起由调用方执行）。
pub enum AssemblyError {
    NoDescription,
    ParseFailed,
    InvalidInventory,
    Unsupported(&'static str),
    /// 70.12：板级选择/描述绑定拒绝（机器可读理由见 [`Self::reason`]）。
    #[cfg(target_os = "none")]
    Board(String),
}

impl AssemblyError {
    /// 机器可读 reason（证据行/测试断言；`&'static str` 变体给固定串）。
    pub fn reason(&self) -> alloc::string::String {
        use alloc::string::ToString;
        match self {
            AssemblyError::NoDescription => "platform-no-description".to_string(),
            AssemblyError::ParseFailed => "platform-description-parse-failed".to_string(),
            AssemblyError::InvalidInventory => "platform-inventory-invalid".to_string(),
            AssemblyError::Unsupported(w) => alloc::format!("platform-unsupported:{w}"),
            #[cfg(target_os = "none")]
            AssemblyError::Board(r) => r.clone(),
        }
    }
}

impl core::fmt::Debug for AssemblyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AssemblyError::NoDescription => write!(f, "no platform description"),
            AssemblyError::ParseFailed => write!(f, "platform description parse failed"),
            AssemblyError::InvalidInventory => write!(f, "inventory validation failed"),
            AssemblyError::Unsupported(w) => write!(f, "platform unsupported: {w}"),
            #[cfg(target_os = "none")]
            AssemblyError::Board(r) => write!(f, "board refused: {r}"),
        }
    }
}

/// 装配产物：已验证 inventory + HAL 运行时（+ 板级绑定证据行）。
pub struct RootPlatform {
    pub inventory: ValidatedInventory,
    pub hal: HalRuntime,
    /// 板级绑定证据行（70.12；host 装配路径为 None）。
    #[cfg(target_os = "none")]
    pub vouch: Option<VouchLine>,
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
    // 70.12：开发 adapter（QEMU virt）作为**唯一**开发入口；实板 adapter
    // 走 `REGISTERED_ADAPTERS` 注册（当前为空——板卡冻结前不虚构）。
    let dev_adapter: &'static eneros_board::adapter::QemuVirtAdapter = alloc::boxed::Box::leak(
        alloc::boxed::Box::new(eneros_board::adapter::QemuVirtAdapter::new()),
    );
    let registry = RootBoardRegistry::new(dev_adapter)
        .map_err(|e| AssemblyError::Board(BoardSelectError::Selection(e).reason()))?;
    let selection = RootBoardRegistry::compile_time_selection()
        .map_err(|e| AssemblyError::Board(BoardSelectError::Selection(e).reason()))?;
    let adapter = registry
        .adapter(selection.as_ref())
        .map_err(|e| AssemblyError::Board(BoardSelectError::Selection(e).reason()))?;
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
    // 70.12（WP-70.12.2）：运行时描述绑定核对。声明了描述摘要则必须匹配；
    // 声明了 SKU 则必需设备/MMIO/一致性依据都要成立（未声明 = 开发入口，
    // 仍核对描述非空与必需设备）。记录判据由 xtask 侧把守（目标侧不携带
    // 冻结记录：账本为空时 `vouch_selection` 跳过记录判据）。
    let runtime_sku = match selection.as_ref() {
        Some(s) => s.sku_id.clone(),
        None => alloc::string::String::from("qemu-virt-aarch64"),
    };
    let runtime_revision = match selection.as_ref() {
        Some(s) => s.revision.clone(),
        None => alloc::string::String::from("1.0.0"),
    };
    let ledger = crate::board_select::empty_ledger(&runtime_sku);
    let vouch = registry
        .vouch(
            selection.as_ref(),
            &runtime_sku,
            &runtime_revision,
            dtb,
            &validated,
            &ledger,
        )
        .map_err(|e| AssemblyError::Board(e.reason()))?;
    Ok(RootPlatform {
        inventory: validated,
        hal,
        vouch: Some(vouch),
    })
}
