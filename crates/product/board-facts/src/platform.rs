//! 板级装配与描述校验（WP-70.12.2/70.12.3）。
//!
//! # 判据（全部 fail-closed）
//! - **构建期所选 = 运行时所见**：`BoardSelection` 由构建配置冻结（SKU id +
//!   记录摘要），运行时用收到的描述重算摘要，不一致即拒绝——这样"运时猜
//!   compatible 再兜底 QEMU"不可能悄悄成立；
//! - **必需设备在场**：按能力族声明的最低设备集合（控制台/存储/网络/熵）
//!   逐项核对，缺任一项不进入必需服务就绪；
//! - **地址与中断合法性**：MMIO 越界/重叠、重复 IRQ、非页对齐、
//!   零容量沿用既有 inventory 校验并补足；
//! - **一致性声明**：宣称 `Coherent` 的平台必须给出硬件依据（`Cache` 族
//!   已验证事实），否则退回 `NonCoherent`——不许把 QEMU 的一致假设搬到
//!   真板（蓝图决策表明确 B 方案不可接受）。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use eneros_hal::inventory::{DeviceStatus, ValidatedInventory};
use eneros_hal::platform_adapter::PlatformError;

use crate::capability::Subsystem;
use crate::digest::description_digest;
use crate::ledger::BoardLedger;

/// 构建期冻结的板级选择（由 profile/产品装配提供；运行时只消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSelection {
    /// 目标 SKU id（必须与冻结记录一致）。
    pub sku_id: String,
    /// 板卡修订（换修订 = 新组合）。
    pub revision: String,
    /// 冻结记录摘要（[`crate::digest::ledger_digest`]）。
    pub record_digest: String,
    /// 运行时描述摘要（[`description_digest`]）；空 = 该组合不作描述绑定
    /// （仅开发平台允许，且必须在文档登记）。
    pub description_digest: String,
}

impl BoardSelection {
    /// 该选择是否要求描述绑定。
    pub fn binds_description(&self) -> bool {
        !self.description_digest.is_empty()
    }
}

/// 选择本身的形态错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    EmptySku,
    EmptyRevision,
    BadRecordDigest,
    BadDescriptionDigest,
}

impl SelectionError {
    pub fn reason(&self) -> &'static str {
        match self {
            SelectionError::EmptySku => "selection-empty-sku",
            SelectionError::EmptyRevision => "selection-empty-revision",
            SelectionError::BadRecordDigest => "selection-bad-record-digest",
            SelectionError::BadDescriptionDigest => "selection-bad-description-digest",
        }
    }
}

impl BoardSelection {
    /// 形态校验：SKU/修订非空；摘要一旦给出必须是合法 sha256。
    pub fn validate(&self) -> Result<(), SelectionError> {
        if self.sku_id.trim().is_empty() {
            return Err(SelectionError::EmptySku);
        }
        if self.revision.trim().is_empty() {
            return Err(SelectionError::EmptyRevision);
        }
        if !self.record_digest.is_empty() && !crate::fact::is_sha256_hex(&self.record_digest) {
            return Err(SelectionError::BadRecordDigest);
        }
        if !self.description_digest.is_empty()
            && !crate::fact::is_sha256_hex(&self.description_digest)
        {
            return Err(SelectionError::BadDescriptionDigest);
        }
        Ok(())
    }
}

/// 描述绑定结果（可打印的证据行素材）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformVouch {
    /// 运行时描述摘要（重算值）。
    pub description_digest: String,
    /// 绑定是否成立（`Ok` 时恒真）。
    pub bound: bool,
    /// 校验过的必需设备（按 compatible 主标识）。
    pub required_devices: Vec<String>,
}

/// 描述绑定失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformVouchError {
    /// 选择形态非法。
    Selection(SelectionError),
    /// SKU 不匹配（构建期选 A、运行时描述是 B）。
    SkuMismatch { selected: String, runtime: String },
    /// 修订不匹配。
    RevisionMismatch { selected: String, runtime: String },
    /// 描述摘要不匹配（介质被替换/装载了另一块板的 DTB）。
    DescriptionMismatch { expected: String, actual: String },
    /// 平台描述缺失。
    NoDescription,
    /// inventory 校验失败（沿用既有拒绝语义）。
    InvalidInventory(String),
    /// 必需设备缺失或状态非 Ok。
    MissingRequiredDevice {
        subsystem: &'static str,
        what: &'static str,
    },
    /// MMIO 区不可用（越出内存/非页对齐/容量为零）。
    BadMmio {
        device: String,
        reason: &'static str,
    },
    /// 宣称一致却没有硬件依据。
    CoherencyWithoutFact { device: String },
    /// 账本未通过判据（构建期选择所依据的事实本身有问题）。
    LedgerNotValid(String),
    /// 账本身份与选择不一致（选择的 SKU 与账本记录的 SKU 不同）。
    LedgerIdentityMismatch { expected: String, actual: String },
    /// 记录摘要与当前账本不符（记录已变 / 选错了记录）。
    RecordDigestMismatch { expected: String, actual: String },
}

impl PlatformVouchError {
    /// 机器可读 reason（证据行/测试断言用）。
    pub fn reason(&self) -> String {
        match self {
            PlatformVouchError::Selection(e) => e.reason().to_string(),
            PlatformVouchError::SkuMismatch { selected, runtime } => {
                format!("platform-sku-mismatch:{selected}!={runtime}")
            }
            PlatformVouchError::RevisionMismatch { selected, runtime } => {
                format!("platform-revision-mismatch:{selected}!={runtime}")
            }
            PlatformVouchError::DescriptionMismatch { expected, actual } => format!(
                "platform-description-mismatch:{}!={}",
                short(expected),
                short(actual)
            ),
            PlatformVouchError::NoDescription => "platform-no-description".to_string(),
            PlatformVouchError::InvalidInventory(r) => format!("platform-inventory-invalid:{r}"),
            PlatformVouchError::MissingRequiredDevice { subsystem, what } => {
                format!("platform-missing-device:{subsystem}:{what}")
            }
            PlatformVouchError::BadMmio { device, reason } => {
                format!("platform-bad-mmio:{device}:{reason}")
            }
            PlatformVouchError::CoherencyWithoutFact { device } => {
                format!("platform-coherency-without-fact:{device}")
            }
            PlatformVouchError::LedgerNotValid(r) => format!("platform-ledger-invalid:{r}"),
            PlatformVouchError::LedgerIdentityMismatch { expected, actual } => {
                format!("platform-ledger-identity-mismatch:{expected}!={actual}")
            }
            PlatformVouchError::RecordDigestMismatch { expected, actual } => format!(
                "platform-record-digest-mismatch:{}!={}",
                short(expected),
                short(actual)
            ),
        }
    }

    /// 是否必须停止必需服务就绪（全部 fail-closed，保留语义供调用方分流）。
    pub fn blocks_readiness(&self) -> bool {
        true
    }
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

/// 必需设备族的最低要求（按能力族）。
///
/// `required_devices` 是**蓝图 WP-70.12.2「实测记录 boot 耗时/映射窗/服务
/// 内存」的机器前提**：缺控制台就没有串口证据，缺存储就没有持久进度，
/// 缺熵就不能进密码面。
pub fn required_device_subsystems() -> &'static [(Subsystem, &'static str)] {
    &[
        (Subsystem::Docs, "console(chosen.stdout-path)"),
        (Subsystem::Storage, "boot-storage(chosen)"),
        (Subsystem::Trng, "entropy(chosen)"),
        (Subsystem::NetworkPhy, "network-controller"),
    ]
}

/// 设备平台角色（**由描述事实判定**：FDT 节点路径或 compatible 的通用
/// 类别词，绝不匹配具体厂商串——换板不改判定逻辑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformRole {
    Console,
    Block,
    Net,
    Entropy,
    Other,
}

/// 通用类别词（跨厂商稳定；`compatible` 的通用部分而非型号）。
/// 审计（S7 F14）：`phy` 单独成词会让 `/soc/usb-phy@…` 冒充网络控制器——
/// 网络判定改为"需要明确的网络类词"（`ethernet`/`net`/`mdio`/`mac`/`gmac`/
/// `macb`）；PHY 只在同时出现网络词时才有意义，不单独作为判据。
const ROLE_CLASSES: [(PlatformRole, &[&str]); 4] = [
    (
        PlatformRole::Net,
        &["ethernet", "net", "mdio", "mac", "gmac", "macb"],
    ),
    (PlatformRole::Console, &["serial", "uart", "console"]),
    (
        PlatformRole::Block,
        &["blk", "block", "mmc", "nvme", "sata", "scsi"],
    ),
    (PlatformRole::Entropy, &["rng", "trng", "entropy", "random"]),
];

/// 判定设备平台角色：先看节点路径（FDT 事实），再看 compatible 的类别词。
pub fn platform_role(device: &eneros_hal::inventory::DeviceResources) -> PlatformRole {
    let path = device.path.to_ascii_lowercase();
    for (role, words) in ROLE_CLASSES {
        if words.iter().any(|w| path.contains(w)) {
            return role;
        }
    }
    for c in device.compatible.as_slice() {
        let c = c.to_ascii_lowercase();
        // 只取 `compatible` 的类别词（跳过厂商前缀段）。
        let class_part = c.split(',').next_back().unwrap_or(&c);
        for (role, words) in ROLE_CLASSES {
            if words
                .iter()
                .any(|w| class_part == *w || class_part.contains(w))
            {
                return role;
            }
        }
    }
    PlatformRole::Other
}

/// MMIO 基础合法性（页对齐 + 非零容量）。
pub fn mmio_is_legal(phys: u64, size: u64) -> Result<(), &'static str> {
    const PAGE: u64 = 4096;
    if size == 0 {
        return Err("zero-size");
    }
    if phys == 0 {
        return Err("null-address");
    }
    if phys % PAGE != 0 {
        return Err("unaligned-phys");
    }
    if size % PAGE != 0 {
        return Err("unaligned-size");
    }
    Ok(())
}

/// 运行时描述绑定校验（唯一入口）。
///
/// 参数：
/// - `selection`：构建期冻结选择；
/// - `runtime_sku` / `runtime_revision`：运行时从描述/清单读出的事实；
/// - `description`：运行时描述字节（DTB）；
/// - `validated`：已通过 inventory 校验的清单；
/// - `ledger`：该 SKU 的冻结账本（提供一致性/熵等硬件依据）。
pub fn vouch_selection(
    selection: &BoardSelection,
    runtime_sku: &str,
    runtime_revision: &str,
    description: &[u8],
    validated: &ValidatedInventory,
    ledger: &BoardLedger,
) -> Result<PlatformVouch, PlatformVouchError> {
    selection
        .validate()
        .map_err(PlatformVouchError::Selection)?;
    if description.is_empty() {
        return Err(PlatformVouchError::NoDescription);
    }
    if runtime_sku != selection.sku_id {
        return Err(PlatformVouchError::SkuMismatch {
            selected: selection.sku_id.clone(),
            runtime: runtime_sku.to_string(),
        });
    }
    if runtime_revision != selection.revision {
        return Err(PlatformVouchError::RevisionMismatch {
            selected: selection.revision.clone(),
            runtime: runtime_revision.to_string(),
        });
    }
    let actual = description_digest(description);
    if selection.binds_description() && selection.description_digest != actual {
        return Err(PlatformVouchError::DescriptionMismatch {
            expected: selection.description_digest.clone(),
            actual,
        });
    }
    let inv = &validated.inventory;
    inv.validate()
        .map_err(|e| PlatformVouchError::InvalidInventory(format!("{e:?}")))?;

    // 审计（S7 F4）：选择里带了**记录摘要**时，必须核对账本身份与摘要——
    // 旧实现只比描述摘要，`selection.record_digest` 从不参与判定（"构建期
    // 所选记录"与"运行时依据的事实"可以完全无关）。目标侧不携带冻结记录时
    // 账本为空（记录判据由 xtask 侧把守），此时不重复判。
    if !ledger.facts.is_empty() {
        if ledger.sku_id != selection.sku_id {
            return Err(PlatformVouchError::LedgerIdentityMismatch {
                expected: selection.sku_id.clone(),
                actual: ledger.sku_id.clone(),
            });
        }
        if !selection.record_digest.is_empty() {
            let actual = crate::digest::freeze_digest(
                &ledger.sku_id,
                &ledger.revision,
                ledger.ledger_version,
                &ledger.facts,
                &ledger.missing,
            );
            if actual != selection.record_digest {
                return Err(PlatformVouchError::RecordDigestMismatch {
                    expected: selection.record_digest.clone(),
                    actual,
                });
            }
        }
    }

    // 必需设备在场（chosen 角色 + 网络控制器）。
    // 审计（S7 F14）：每个 chosen 角色报**自己的**能力族（旧实现三者都报
    // `docs`，拒绝理由指错族——判据串是给运维读的，必须指对人）。
    let chosen_ok = |idx: Option<usize>,
                     what: &'static str,
                     subsystem: &'static str|
     -> Result<(), PlatformVouchError> {
        match idx {
            Some(i) => match inv.devices.get(i) {
                Some(d) if d.status == DeviceStatus::Ok => Ok(()),
                _ => Err(PlatformVouchError::MissingRequiredDevice { subsystem, what }),
            },
            None => Err(PlatformVouchError::MissingRequiredDevice { subsystem, what }),
        }
    };
    chosen_ok(inv.chosen.console, "console", "docs")?;
    chosen_ok(inv.chosen.boot_storage, "boot-storage", "storage")?;
    chosen_ok(inv.chosen.entropy, "entropy", "trng")?;

    let mut required_devices: Vec<String> = Vec::new();
    for d in &inv.devices {
        let name = d.compatible.first().clone();
        if d.status != DeviceStatus::Ok {
            continue;
        }
        // 网络存在性：描述事实里的网卡（角色判定，非厂商串）。
        if platform_role(d) == PlatformRole::Net {
            required_devices.push(name.clone());
        }
        for reg in &d.regs {
            mmio_is_legal(reg.phys.as_u64(), reg.size).map_err(|r| {
                PlatformVouchError::BadMmio {
                    device: name.clone(),
                    reason: r,
                }
            })?;
        }
        // 一致性声明必须有硬件依据（`Cache` 族已验证事实）。
        if d.dma_coherency == eneros_hal::dma::DmaCoherency::Coherent
            && ledger.verified_of(Subsystem::Cache).is_empty()
            && ledger.verified_of(Subsystem::Soc).is_empty()
        {
            return Err(PlatformVouchError::CoherencyWithoutFact {
                device: name.clone(),
            });
        }
    }
    if required_devices.is_empty() {
        return Err(PlatformVouchError::MissingRequiredDevice {
            subsystem: "network-phy",
            what: "network-controller",
        });
    }
    // 账本自身判据（构建期选择所依据的事实必须成立）。空账本 = 目标侧未携带
    // 冻结记录（开发镜像），此时记录判据由 xtask 侧把守，此处不重复判。
    if !ledger.facts.is_empty() {
        let report = ledger.validate(crate::capability::SkuKind::Development);
        if !report.is_ok() {
            return Err(PlatformVouchError::LedgerNotValid(
                report.violations.join(","),
            ));
        }
    }
    Ok(PlatformVouch {
        description_digest: actual,
        bound: selection.binds_description(),
        required_devices,
    })
}

/// 从 assembly 层错误映射（root 装配统一错误面）。
pub fn platform_error_of(v: &PlatformVouchError) -> PlatformError {
    match v {
        PlatformVouchError::NoDescription => PlatformError::NoDescription,
        PlatformVouchError::InvalidInventory(_) => PlatformError::InvalidInventory,
        _ => PlatformError::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use eneros_hal::addr::PhysAddr;
    use eneros_hal::dma::DmaCoherency;
    use eneros_hal::inventory::{
        ChosenResources, CpuNode, DeviceId, DeviceResources, InventoryBuilder, MemoryRegion,
        MmioRegion, NonEmptyVec, PlatformProvenance,
    };

    use super::*;
    use crate::fact::{Fact, UNIT_NONE};
    use crate::memory::ram_window_facts;

    /// 测试平台设备表（每设备独立 MMIO 页区、chosen 三角色互不冲突）：
    /// 0=控制台，1=virtio（启动存储），2=硬件熵源，3=网络控制器。
    fn device_at(
        id: u32,
        compatible: &str,
        phys: u64,
        size: u64,
        coherent: bool,
        path: &str,
    ) -> DeviceResources {
        DeviceResources {
            id: DeviceId(id),
            path: path.into(),
            compatible: NonEmptyVec::new(vec![compatible.into()]).unwrap(),
            status: DeviceStatus::Ok,
            regs: vec![MmioRegion {
                phys: PhysAddr::new(phys),
                size,
            }],
            interrupts: vec![],
            clocks: vec![],
            resets: vec![],
            gpios: vec![],
            dmas: vec![],
            iommu: None,
            dma_coherency: if coherent {
                DmaCoherency::Coherent
            } else {
                DmaCoherency::NonCoherent
            },
            properties: Default::default(),
        }
    }

    fn board_inventory() -> ValidatedInventory {
        InventoryBuilder {
            cpus: vec![
                CpuNode {
                    id: 0,
                    enabled: true,
                },
                CpuNode {
                    id: 1,
                    enabled: true,
                },
            ],
            memory: vec![MemoryRegion {
                phys: PhysAddr::new(0x4000_0000),
                size: 0x4000_0000,
            }],
            devices: vec![
                device_at(1, "arm,pl011", 0x0900_0000, 0x1000, false, "/pl011@9000000"),
                device_at(
                    2,
                    "virtio,mmio",
                    0x0a00_0000,
                    0x1000,
                    true,
                    "/virtio_mmio@a000000",
                ),
                device_at(
                    3,
                    "smsc,lan91c111",
                    0x0b00_0000,
                    0x1000,
                    false,
                    "/ethernet@b000000",
                ),
                device_at(
                    4,
                    "qemu,virtio-rng",
                    0x0c00_0000,
                    0x1000,
                    true,
                    "/rng@c000000",
                ),
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
        .expect("test fixture must be a valid inventory")
    }

    fn ledger_for() -> BoardLedger {
        let mut facts = vec![
            Fact::repo(Subsystem::Soc, "compatible", "qemu,virt", UNIT_NONE),
            Fact::repo(Subsystem::Cpu, "cores", "2", "count"),
            Fact::repo(Subsystem::Timer, "frequency", "62500000", "Hz"),
            Fact::repo(Subsystem::BootChain, "uefi", "AAVMF", UNIT_NONE),
            Fact::repo(Subsystem::Gic, "version", "gicv3", UNIT_NONE),
            Fact::repo(Subsystem::Cache, "dma", "coherent-on-synthetic", UNIT_NONE),
        ];
        facts.extend(ram_window_facts(0, 0x4000_0000, 1024));
        for f in facts.iter_mut() {
            f.source = "test".into();
        }
        BoardLedger::new("test-sku", "rev1", 2, facts, Vec::new()).unwrap()
    }

    fn selection_for(description: &[u8]) -> BoardSelection {
        BoardSelection {
            sku_id: "test-sku".into(),
            revision: "rev1".into(),
            record_digest: String::new(),
            description_digest: description_digest(description),
        }
    }

    fn ensure_network_inventory() -> ValidatedInventory {
        board_inventory()
    }

    #[test]
    fn vouch_accepts_bound_selection() {
        let desc = b"runtime-dtb-bytes";
        let inv = ensure_network_inventory();
        let v = vouch_selection(
            &selection_for(desc),
            "test-sku",
            "rev1",
            desc,
            &inv,
            &ledger_for(),
        )
        .expect("bound selection must pass");
        assert!(v.bound);
        assert_eq!(v.description_digest, description_digest(desc));
        assert_eq!(v.required_devices.len(), 1, "网络控制器必须被识别");
    }

    #[test]
    fn vouch_rejects_swapped_board_description() {
        // 构建期选 A 板（摘要 X），运行时喂 B 板 DTB（摘要 Y）→ 拒绝。
        let sel = selection_for(b"board-a-dtb");
        let inv = ensure_network_inventory();
        let err = vouch_selection(
            &sel,
            "test-sku",
            "rev1",
            b"board-b-dtb",
            &inv,
            &ledger_for(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PlatformVouchError::DescriptionMismatch { .. }
        ));
        assert!(err.reason().starts_with("platform-description-mismatch:"));
        assert!(err.blocks_readiness());
    }

    #[test]
    fn vouch_rejects_sku_and_revision_mismatch() {
        let inv = ensure_network_inventory();
        let desc = b"dtb";
        let sel = selection_for(desc);
        let err =
            vouch_selection(&sel, "other-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert!(matches!(err, PlatformVouchError::SkuMismatch { .. }));
        let err = vouch_selection(&sel, "test-sku", "rev2", desc, &inv, &ledger_for()).unwrap_err();
        assert!(matches!(err, PlatformVouchError::RevisionMismatch { .. }));
    }

    #[test]
    fn vouch_rejects_missing_console_or_network() {
        let desc = b"dtb";
        let sel = selection_for(desc);
        let mut inv = ensure_network_inventory();
        inv.inventory.chosen.console = None;
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert_eq!(
            err.reason(),
            "platform-missing-device:docs:console",
            "缺控制台 → 无串口证据，必须停"
        );
        // 描述里没有网卡（换成 GPIO 节点）→ 拒绝。
        let mut inv = ensure_network_inventory();
        for d in inv.inventory.devices.iter_mut() {
            if d.id == DeviceId(3) {
                d.path = "/gpio@b000000".into();
                d.compatible = NonEmptyVec::new(vec!["arm,pl061".into()]).unwrap();
            }
        }
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert!(
            err.reason().contains("network-controller"),
            "unexpected: {}",
            err.reason()
        );
    }

    #[test]
    fn vouch_rejects_bad_mmio() {
        let desc = b"dtb";
        let sel = selection_for(desc);
        let mut inv = ensure_network_inventory();
        inv.inventory.devices[0].regs[0] = MmioRegion {
            phys: PhysAddr::new(0x0900_0001), // 非页对齐
            size: 0x1000,
        };
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert!(matches!(err, PlatformVouchError::BadMmio { .. }));
        assert!(err.reason().contains("unaligned-phys"));
        let mut inv = ensure_network_inventory();
        inv.inventory.devices[0].regs[0] = MmioRegion {
            phys: PhysAddr::new(0x0900_0000),
            size: 0,
        };
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert!(err.reason().contains("zero-size"));
    }

    #[test]
    fn coherency_claim_needs_hardware_fact() {
        // 账本无 Cache/Soc 依据 + 描述宣称一致 → 拒绝（不得沿用 QEMU 假设）。
        let mut facts = vec![
            Fact::repo(Subsystem::Cpu, "cores", "2", "count"),
            Fact::repo(Subsystem::Timer, "frequency", "62500000", "Hz"),
            Fact::repo(Subsystem::BootChain, "uefi", "AAVMF", UNIT_NONE),
            Fact::repo(Subsystem::Gic, "version", "gicv3", UNIT_NONE),
        ];
        facts.extend(ram_window_facts(0, 0x4000_0000, 1024));
        for f in facts.iter_mut() {
            f.source = "test".into();
        }
        let ledger = BoardLedger::new("test-sku", "rev1", 2, facts, Vec::new()).unwrap();
        let inv = ensure_network_inventory();
        let desc = b"dtb";
        let err = vouch_selection(
            &selection_for(desc),
            "test-sku",
            "rev1",
            desc,
            &inv,
            &ledger,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PlatformVouchError::CoherencyWithoutFact { .. }
        ));
    }

    /// 审计（S7 F4）红测：选择里的记录摘要/账本身份必须参与判定。
    #[test]
    fn record_digest_and_ledger_identity_are_bound() {
        let desc = b"dtb";
        let inv = ensure_network_inventory();
        let ledger = ledger_for();
        let digest = crate::digest::freeze_digest(
            &ledger.sku_id,
            &ledger.revision,
            ledger.ledger_version,
            &ledger.facts,
            &ledger.missing,
        );
        let mut sel = selection_for(desc);
        sel.record_digest = digest.clone();
        assert!(vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger).is_ok());
        let mut bad = selection_for(desc);
        bad.record_digest = "b".repeat(64);
        let err = vouch_selection(&bad, "test-sku", "rev1", desc, &inv, &ledger).unwrap_err();
        assert!(
            err.reason().starts_with("platform-record-digest-mismatch:"),
            "unexpected: {}",
            err.reason()
        );
        let evil = BoardLedger::new(
            "evil-sku",
            "rev1",
            2,
            ledger.facts.clone(),
            ledger.missing.clone(),
        )
        .unwrap();
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &evil).unwrap_err();
        assert!(
            err.reason()
                .starts_with("platform-ledger-identity-mismatch:"),
            "unexpected: {}",
            err.reason()
        );
    }

    #[test]
    fn usb_phy_is_not_a_network_controller() {
        // 审计（S7 F14）：`phy` 单独成词不得判为网络控制器。
        let nd = device_at(
            9,
            "vendor,usb2-phy",
            0x1000_0000,
            0x1000,
            false,
            "/soc/usb-phy@10000000",
        );
        assert_eq!(platform_role(&nd), PlatformRole::Other);
        let nd = device_at(9, "vendor,mdio", 0x1000_0000, 0x1000, false, "/soc/x");
        assert_eq!(platform_role(&nd), PlatformRole::Net);
    }

    #[test]
    fn chosen_role_refusals_name_the_right_subsystem() {
        let desc = b"dtb";
        let sel = selection_for(desc);
        let mut inv = ensure_network_inventory();
        inv.inventory.chosen.entropy = None;
        let err = vouch_selection(&sel, "test-sku", "rev1", desc, &inv, &ledger_for()).unwrap_err();
        assert_eq!(
            err.reason(),
            "platform-missing-device:trng:entropy",
            "缺熵源必须报 trng（旧实现报 docs）"
        );
    }

    #[test]
    fn platform_role_uses_description_facts_not_vendor_strings() {
        // 路径事实优先：厂商 compatible 完全陌生的网卡也能判出角色。
        let nd = device_at(
            9,
            "acme,nic-x1",
            0x1000_0000,
            0x1000,
            false,
            "/soc/ethernet@10000000",
        );
        assert_eq!(platform_role(&nd), PlatformRole::Net);
        // 只有 compatible 时按类别词（不是型号）：通用段命中。
        let nd = device_at(9, "vendor,trng", 0x1000_0000, 0x1000, false, "/soc/x");
        assert_eq!(platform_role(&nd), PlatformRole::Entropy);
        let nd = device_at(9, "vendor,serial", 0x1000_0000, 0x1000, false, "/soc/x");
        assert_eq!(platform_role(&nd), PlatformRole::Console);
        // 型号串不参与角色判定（"lan91c111"/"d2000" 不是类别词）。
        let nd = device_at(9, "smsc,lan91c111", 0x1000_0000, 0x1000, false, "/soc/x");
        assert_eq!(platform_role(&nd), PlatformRole::Other);
        let nd = device_at(
            9,
            "phytium,d2000-gpio",
            0x1000_0000,
            0x1000,
            false,
            "/soc/x",
        );
        assert_eq!(platform_role(&nd), PlatformRole::Other);
    }

    #[test]
    fn selection_shape_is_validated() {
        let bad = BoardSelection {
            sku_id: String::new(),
            revision: "r".into(),
            record_digest: String::new(),
            description_digest: String::new(),
        };
        assert_eq!(bad.validate(), Err(SelectionError::EmptySku));
        let bad = BoardSelection {
            sku_id: "s".into(),
            revision: "r".into(),
            record_digest: "not-a-digest".into(),
            description_digest: String::new(),
        };
        assert_eq!(bad.validate(), Err(SelectionError::BadRecordDigest));
        let good = selection_for(b"dtb");
        assert!(good.validate().is_ok());
    }

    #[test]
    fn empty_description_is_fail_closed() {
        let inv = ensure_network_inventory();
        let sel = BoardSelection {
            sku_id: "test-sku".into(),
            revision: "rev1".into(),
            record_digest: String::new(),
            description_digest: String::new(),
        };
        let err = vouch_selection(&sel, "test-sku", "rev1", b"", &inv, &ledger_for()).unwrap_err();
        assert_eq!(err.reason(), "platform-no-description");
        assert_eq!(platform_error_of(&err), PlatformError::NoDescription);
    }
}
