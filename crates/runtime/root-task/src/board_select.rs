//! 70.12（WP-70.12.2/3）：目标侧板级装配选择与描述绑定。
//!
//! # 旧行为（本卷修正的缺陷）
//! `platform.rs::assemble` 硬编码 `QemuVirtAdapter`：任何镜像——包括为真实
//! 板卡构建的 `board-prod`——都用 QEMU 的平台事实装配（一致内存、无
//! reset/clock）。真板上这不是"缺功能"，而是**静默的错误事实**。
//!
//! # 新行为
//! 装配期从编译期注入的绑定（`ENEROS_BOARD_*`）构造 [`BoardSelection`]：
//! - 未声明 → 开发入口（QEMU virt adapter），行为与既有镜像一致；
//! - 声明但该 SKU **未注册 adapter** → 拒绝（`board-no-adapter-for-sku`），
//!   打印机器可读拒绝行并挂起——**不回退 QEMU**；
//! - 声明了描述摘要 → 运行时用收到的 DTB 重算并与声明比对（介质被换/
//!   装错板 = 拒绝）。
//!
//! 判据实现全部复用 `eneros-board-facts`（与 xtask 侧同一份代码）。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use eneros_board_facts::digest::description_digest;
use eneros_board_facts::registry::BoardRefusal;
use eneros_board_facts::{
    assembly_selection, vouch_selection, AdapterRegistry, BoardSelection, PlatformVouchError,
    RegisteredAdapter,
};
use eneros_hal::platform_adapter::PlatformAdapter;

/// 编译期绑定（`option_env!`：未注入 = 开发镜像）。
pub const ENV_SKU: Option<&str> = option_env!("ENEROS_BOARD_SKU");
pub const ENV_REVISION: Option<&str> = option_env!("ENEROS_BOARD_REVISION");
pub const ENV_RECORD_DIGEST: Option<&str> = option_env!("ENEROS_BOARD_RECORD_DIGEST");
pub const ENV_DESCRIPTION_DIGEST: Option<&str> = option_env!("ENEROS_BOARD_DESCRIPTION_DIGEST");

/// 板级装配错误（打印 + fail-closed 挂起由调用方执行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardSelectError {
    /// 装配绑定形态非法/半套绑定/摘要非法/未注册 adapter。
    Selection(BoardRefusal),
    /// 描述绑定失败（SKU/修订/描述摘要/必需设备/MMIO/一致性依据）。
    Vouch(PlatformVouchError),
}

impl BoardSelectError {
    /// 机器可读 reason（证据行/测试断言）。
    pub fn reason(&self) -> String {
        match self {
            BoardSelectError::Selection(r) => r.reason(),
            BoardSelectError::Vouch(e) => e.reason(),
        }
    }
}

/// 已注册的实板 adapter（**当前为空**：真实板卡未冻结前不虚构 adapter）。
///
/// 板卡型号冻结后，`product/skus/<sku>.json` 的 revision/摘要与实现的
/// adapter 在此登记；在此之前 `board-prod` 镜像会在此处**明确拒绝**
/// （`board-no-adapter-for-sku`），而不是回退 QEMU。
pub const REGISTERED_ADAPTERS: &[RegisteredAdapter] = &[];

/// 目标侧注册表（含开发平台 adapter + 已注册实板 adapter）。
///
/// 开发 adapter 走单独入口而不是注册项：它没有 SKU/修订语义，注册它会让
/// "某块真板已就绪"的判据失真。
pub struct RootBoardRegistry {
    dev_adapter: &'static dyn PlatformAdapter,
    registry: AdapterRegistry,
}

impl RootBoardRegistry {
    /// 构造（`dev_adapter` = 开发平台 adapter，通常 QEMU virt）。
    pub fn new(dev_adapter: &'static dyn PlatformAdapter) -> Result<Self, BoardRefusal> {
        let mut registry = AdapterRegistry::new();
        for entry in REGISTERED_ADAPTERS {
            registry.register(RegisteredAdapter {
                sku_id: entry.sku_id,
                revision: entry.revision,
                record_digest: entry.record_digest,
                adapter: entry.adapter,
            })?;
        }
        Ok(Self {
            dev_adapter,
            registry,
        })
    }

    /// 编译期绑定 → 选择（`None` = 开发入口）。
    pub fn compile_time_selection() -> Result<Option<BoardSelection>, BoardRefusal> {
        assembly_selection(
            ENV_SKU,
            ENV_REVISION,
            ENV_RECORD_DIGEST,
            ENV_DESCRIPTION_DIGEST,
        )
    }

    /// 解析 adapter（未注册 → 拒绝，不回退开发平台）。
    pub fn adapter(
        &self,
        selection: Option<&BoardSelection>,
    ) -> Result<&dyn PlatformAdapter, BoardRefusal> {
        self.registry.resolve(selection, self.dev_adapter)
    }

    /// 运行时描述绑定（SKU/修订/描述摘要 + 必需设备/MMIO/一致性依据）。
    pub fn vouch(
        &self,
        selection: Option<&BoardSelection>,
        runtime_sku: &str,
        runtime_revision: &str,
        description: &[u8],
        validated: &eneros_hal::inventory::ValidatedInventory,
        ledger: &eneros_board_facts::ledger::BoardLedger,
    ) -> Result<VouchLine, BoardSelectError> {
        let Some(sel) = selection else {
            // 开发入口：不绑定 SKU/描述摘要（但描述必须非空——无描述即无平台）。
            if description.is_empty() {
                return Err(BoardSelectError::Vouch(PlatformVouchError::NoDescription));
            }
            return Ok(VouchLine {
                sku: String::from(runtime_sku),
                revision: String::from(runtime_revision),
                description_digest: description_digest(description),
                bound: false,
                devices: 0,
            });
        };
        let v = vouch_selection(
            sel,
            runtime_sku,
            runtime_revision,
            description,
            validated,
            ledger,
        )
        .map_err(BoardSelectError::Vouch)?;
        Ok(VouchLine {
            sku: runtime_sku.to_string(),
            revision: runtime_revision.to_string(),
            description_digest: v.description_digest,
            bound: v.bound,
            devices: v.required_devices.len(),
        })
    }
}

/// 描述绑定证据行素材（root 打印；xtask 测试断言同一公式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VouchLine {
    pub sku: String,
    pub revision: String,
    pub description_digest: String,
    pub bound: bool,
    /// 已识别的平台角色设备数（网络控制器等）。
    pub devices: usize,
}

impl VouchLine {
    /// 证据行（稳定词形：`70.12 BOARD` 前缀，便于串口判据）。
    pub fn log_line(&self) -> String {
        alloc::format!(
            "[eneros-root-task] 70.12 BOARD sku={} revision={} desc_digest={} bound={} devices={}",
            self.sku,
            self.revision,
            &self.description_digest[..self.description_digest.len().min(16)],
            self.bound,
            self.devices
        )
    }
}

/// 空账本（目标侧当前不携带冻结记录：记录判据由 xtask 侧把守；
/// 空账本时 `vouch_selection` 跳过记录判据，不重复判）。
pub fn empty_ledger(sku: &str) -> eneros_board_facts::ledger::BoardLedger {
    eneros_board_facts::ledger::BoardLedger::new(sku, "unknown", 1, Vec::new(), Vec::new())
        .expect("空账本构造不可失败（sku 非空时）")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试断言 panic!（elf-loader/ota-ab 先例）
    use eneros_hal::inventory::ValidatedInventory;
    use eneros_hal::platform_adapter::{HalRuntime, PlatformError, PlatformInput};

    use super::*;

    /// 占位 adapter（只用于装配选择路径；`discover` 恒拒绝）。
    struct DevAdapter;
    impl PlatformAdapter for DevAdapter {
        fn discover(&self, _i: &PlatformInput) -> Result<ValidatedInventory, PlatformError> {
            Err(PlatformError::Unsupported)
        }
        fn build_hal(&self, _i: &ValidatedInventory) -> Result<HalRuntime, PlatformError> {
            Err(PlatformError::Unsupported)
        }
    }
    static DEV: DevAdapter = DevAdapter;

    fn sel(sku: &str, revision: &str, desc_digest: &str) -> BoardSelection {
        BoardSelection {
            sku_id: sku.into(),
            revision: revision.into(),
            record_digest: String::new(),
            description_digest: desc_digest.into(),
        }
    }

    /// 最小可用 inventory（控制台/存储/熵/网卡 + chosen 三角色）。
    fn dev_inventory() -> ValidatedInventory {
        use eneros_hal::addr::PhysAddr;
        use eneros_hal::dma::DmaCoherency;
        use eneros_hal::inventory::{
            ChosenResources, CpuNode, DeviceId, DeviceResources, InventoryBuilder, MemoryRegion,
            MmioRegion, NonEmptyVec, PlatformProvenance,
        };
        let dev = |id: u32, compat: &str, phys: u64, path: &str| DeviceResources {
            id: DeviceId(id),
            path: path.into(),
            compatible: NonEmptyVec::new(alloc::vec![compat.into()]).unwrap(),
            status: eneros_hal::inventory::DeviceStatus::Ok,
            regs: alloc::vec![MmioRegion {
                phys: PhysAddr::new(phys),
                size: 0x1000,
            }],
            interrupts: alloc::vec![],
            clocks: alloc::vec![],
            resets: alloc::vec![],
            gpios: alloc::vec![],
            dmas: alloc::vec![],
            iommu: None,
            dma_coherency: DmaCoherency::NonCoherent,
            properties: Default::default(),
        };
        InventoryBuilder {
            cpus: alloc::vec![CpuNode {
                id: 0,
                enabled: true
            }],
            memory: alloc::vec![MemoryRegion {
                phys: PhysAddr::new(0x4000_0000),
                size: 0x4000_0000
            }],
            devices: alloc::vec![
                dev(1, "arm,pl011", 0x0900_0000, "/pl011@9000000"),
                dev(2, "virtio,mmio", 0x0a00_0000, "/virtio_mmio@a000000"),
                dev(3, "acme,nic", 0x0b00_0000, "/ethernet@b000000"),
                dev(4, "acme,rng", 0x0c00_0000, "/rng@c000000"),
            ],
            chosen: ChosenResources {
                console: Some(0),
                boot_storage: Some(1),
                entropy: Some(3),
                ..Default::default()
            },
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: eneros_hal::inventory::DescriptionSource::Synthetic,
            digest: None,
            description: "test".into(),
        })
        .validate()
        .expect("fixture must validate")
    }

    #[test]
    fn undeclared_selection_uses_development_adapter() {
        let r = RootBoardRegistry::new(&DEV).expect("registry");
        let a = r.adapter(None).expect("dev entry");
        assert!(a.discover(&PlatformInput::default()).is_err());
        // 开发入口无描述 → 拒绝（无描述即无平台）。
        let err = r
            .vouch(
                None,
                "qemu-virt-aarch64",
                "1.0.0",
                b"",
                &dev_inventory(),
                &empty_ledger("qemu-virt-aarch64"),
            )
            .unwrap_err();
        assert_eq!(err.reason(), "platform-no-description");
    }

    #[test]
    fn declared_unregistered_sku_is_refused_without_qemu_fallback() {
        let r = RootBoardRegistry::new(&DEV).expect("registry");
        let s = sel("phytium-d2000", "unknown", "");
        let err = r.adapter(Some(&s)).unwrap_err();
        assert_eq!(
            err.reason(),
            "board-no-adapter-for-sku:phytium-d2000:registered=[]"
        );
        let e = BoardSelectError::Selection(err);
        assert_eq!(
            e.reason(),
            "board-no-adapter-for-sku:phytium-d2000:registered=[]"
        );
    }

    #[test]
    fn declared_selection_binds_description_digest() {
        let r = RootBoardRegistry::new(&DEV).expect("registry");
        let desc = b"runtime-dtb";
        let digest = description_digest(desc);
        let s = sel("dev-sku", "rev-A", &digest);
        // 未注册 adapter → 先在这里拒绝（真实板卡路径本卷不虚构 adapter）。
        assert!(r.adapter(Some(&s)).is_err());
        // 描述绑定判据本身（直接走 vouch）：摘要不符即拒绝。
        let err = r
            .vouch(
                Some(&s),
                "dev-sku",
                "rev-A",
                b"other-dtb",
                &dev_inventory(),
                &empty_ledger("dev-sku"),
            )
            .unwrap_err();
        assert!(err.reason().starts_with("platform-description-mismatch:"));
    }

    #[test]
    fn dev_entry_log_line_has_stable_wording() {
        let r = RootBoardRegistry::new(&DEV).expect("registry");
        let line = r
            .vouch(
                None,
                "qemu-virt-aarch64",
                "1.0.0",
                b"dtb-bytes",
                &dev_inventory(),
                &empty_ledger("qemu-virt-aarch64"),
            )
            .unwrap();
        assert!(!line.bound, "开发入口不绑定描述摘要");
        let text = line.log_line();
        assert!(text.starts_with("[eneros-root-task] 70.12 BOARD sku=qemu-virt-aarch64"));
        assert!(text.contains("bound=false"));
    }
}
