//! 38（WP-38.1）：PlatformInventory / DeviceResources——ARM 平台资源唯一事实。
//!
//! # 不变量（蓝图 38 WP-38.1）
//! - 平台事实**唯一来源**：核心业务只消费本模型，禁止板卡名/厂商常量分支；
//! - 资源强类型且有 owner：地址一律 [`crate::addr`] 强类型，禁止裸 `u64` 层间传址；
//! - `DeviceResources` 只含**已验证**平台事实（FDT/ACPI/manifest 经
//!   [`crate::PlatformDescriptionProvider`] 产出后归一化）；
//! - 未识别/disabled/资源不全 → 明确状态（`Unsupported/Unavailable/Degraded`），
//!   不猜地址/容量（35-01 §1.3）；
//! - no_std + alloc；**本模块不含密码学依赖**——规范化字节由装配层哈希绑定证据。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::addr::PhysAddr;
use crate::dma::DmaCoherency;
use crate::types::IrqTrigger;

/// 设备状态（FDT `status` / manifest 语义归一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    /// 可用（status=okay/absent 缺省）。
    Ok,
    /// 显式禁用（status=disabled）——不 bind、不发布 capability。
    Disabled,
    /// 故障（status=fail）。
    Fail,
    /// 保留（status=reserved）。
    Reserved,
}

/// 非空容器（构造期校验 ≥1；零元素类型不可能表示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptyVec<T>(Vec<T>);

impl<T> NonEmptyVec<T> {
    /// 构造；空输入 → None（fail-closed，不静默降级为空）。
    pub fn new(items: Vec<T>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self(items))
        }
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
    pub fn first(&self) -> &T {
        // 构造不变量：非空。
        &self.0[0]
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// compatible 标识（非空：FDT `compatible` 列表按优先级排序）。
pub type CompatibleId = String;

/// 设备唯一标识（inventory 内稳定索引语义；跨重启由平台描述 digest 绑定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u32);

/// 一个 MMIO 资源区（`reg` 经 bus ranges 翻译后的 CPU 物理地址）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioRegion {
    pub phys: PhysAddr,
    pub size: u64,
}

/// 已解析中断（全局中断号 + 触发类型；SPI/PPI 换算在解析层完成）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptSpec {
    pub irq: u32,
    pub trigger: IrqTrigger,
}

/// 时钟引用（`clocks` 属性：phandle + specifier cells；`name` 可空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockRef {
    pub phandle: u32,
    /// 时钟频率（Hz）；`clock-frequency` 属性或 phandle 指向的 fixed-clock
    /// 解析后填充；无法确定 → `None`（驱动必须拒绝猜时钟）。
    pub frequency_hz: Option<u64>,
}

/// 复位引用（`resets` 属性）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetRef {
    pub phandle: u32,
    pub specifier: Vec<u32>,
}

/// GPIO 引用（`gpios` 属性）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpioRef {
    pub phandle: u32,
    pub specifier: Vec<u32>,
}

/// DMA 通道引用（`dmas` 属性）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmaChannelRef {
    pub phandle: u32,
    pub specifier: Vec<u32>,
}

/// IOMMU 引用（`iommus` 属性；一个设备至多一个域）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IommuRef {
    pub phandle: u32,
    pub domain_id: u32,
}

/// 单设备完整资源描述（工厂/驱动唯一消费面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceResources {
    pub id: DeviceId,
    /// 描述树节点路径（FDT node path；manifest 源可留空）。
    ///
    /// 70.12（WP-70.12.3）：平台角色判据（"这是不是网卡"）用**描述事实**
    /// 而不是厂商 compatible 子串猜——路径在真板 FDT 上直接可读，换板不需要
    /// 改判定逻辑；compatible 厂商串不参与角色判定（厂商分支禁忌）。
    pub path: String,
    pub compatible: NonEmptyVec<CompatibleId>,
    pub status: DeviceStatus,
    pub regs: Vec<MmioRegion>,
    pub interrupts: Vec<InterruptSpec>,
    pub clocks: Vec<ClockRef>,
    pub resets: Vec<ResetRef>,
    pub gpios: Vec<GpioRef>,
    pub dmas: Vec<DmaChannelRef>,
    pub iommu: Option<IommuRef>,
    pub dma_coherency: DmaCoherency,
    /// 有界扩展属性（未知属性保留，不影响核心；上限 [`MAX_PROPERTIES`]）。
    pub properties: BoundedPropertyMap,
}

impl DeviceResources {
    /// 首个 MMIO 区（无 → None；调用方据此 fail-closed）。
    pub fn first_reg(&self) -> Option<MmioRegion> {
        self.regs.first().copied()
    }
    /// 首个时钟频率（无 → None——驱动不得猜时钟）。
    pub fn first_clock_hz(&self) -> Option<u64> {
        self.clocks.iter().find_map(|c| c.frequency_hz)
    }
}

/// 有界属性映射（未知属性白名单保留；超限拒绝——防恶意 DTB 膨胀）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundedPropertyMap {
    entries: Vec<(String, Vec<u8>)>,
}

/// 属性上限（防 OOM；与 FDT walker 上限一致）。
pub const MAX_PROPERTIES: usize = 32;

impl BoundedPropertyMap {
    pub fn try_push(&mut self, name: String, value: Vec<u8>) -> Result<(), InventoryError> {
        if self.entries.len() >= MAX_PROPERTIES {
            return Err(InventoryError::TooManyProperties);
        }
        self.entries.push((name, value));
        Ok(())
    }
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// CPU 拓扑节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuNode {
    pub id: u32,
    pub enabled: bool,
}

/// 内存区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub phys: PhysAddr,
    pub size: u64,
}

/// 保留内存（/reserved-memory；排除 untyped 分配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedRegion {
    pub phys: PhysAddr,
    pub size: u64,
    pub purpose: String,
}

/// 中断控制器域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptDomain {
    pub id: u32,
    pub kind: InterruptDomainKind,
}

/// 中断控制器类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptDomainKind {
    GicV3,
    Other(u16),
}

/// IOMMU 域描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IommuDomainSpec {
    pub id: u32,
    pub base: PhysAddr,
    pub size: u64,
}

/// 总线节点（父子索引；`parent=None` 即根下）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusNode {
    pub name: String,
    pub parent: Option<usize>,
}

/// chosen 资源（console/entropy/boot storage 唯一性校验依据）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChosenResources {
    /// stdout-path（FDT chosen 属性原样）。
    pub stdout: Option<String>,
    /// 控制台设备（compatible 匹配后的设备索引）。
    pub console: Option<usize>,
    /// 熵源设备索引。
    pub entropy: Option<usize>,
    /// 启动存储设备索引（boot storage role）。
    pub boot_storage: Option<usize>,
}

/// 平台描述来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptionSource {
    Fdt,
    Acpi,
    ProductManifest,
    Synthetic,
}

/// 平台出处（审计/evidence 绑定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformProvenance {
    pub source: DescriptionSource,
    /// 规范化字节（`canonical_bytes` 输出）；装配层哈希后填 digest。
    pub digest: Option<[u8; 32]>,
    pub description: String,
}

/// 平台清单（唯一事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInventory {
    pub cpus: Vec<CpuNode>,
    pub memory: Vec<MemoryRegion>,
    pub reserved_memory: Vec<ReservedRegion>,
    pub interrupt_domains: Vec<InterruptDomain>,
    pub iommu_domains: Vec<IommuDomainSpec>,
    pub buses: Vec<BusNode>,
    pub devices: Vec<DeviceResources>,
    pub chosen: ChosenResources,
    pub provenance: PlatformProvenance,
}

/// 清单校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// 无 CPU（平台不可用）。
    NoCpu,
    /// 无内存。
    NoMemory,
    /// 内存区重叠。
    MemoryOverlap { a: PhysAddr, b: PhysAddr },
    /// 保留区越出内存。
    ReservedOutsideMemory { at: PhysAddr },
    /// 设备 MMIO 范围冲突（两设备重叠）。
    MmioConflict { a: DeviceId, b: DeviceId },
    /// 中断域引用缺失。
    UnknownInterruptDomain { id: u32 },
    /// IOMMU owner 重复（同一域多设备属 39 策略；此处仅校验引用合法）。
    UnknownIommuDomain { id: u32 },
    /// console/entropy/boot storage 唯一性冲突（同一设备被多个角色占用）。
    ChosenConflict { role: &'static str },
    /// 设备无 compatible（无法匹配驱动）。
    DeviceWithoutCompatible { id: DeviceId },
    /// 属性超限。
    TooManyProperties,
}

/// 已验证清单（构造即校验通过）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedInventory {
    pub inventory: PlatformInventory,
}

impl PlatformInventory {
    /// 全量校验（WP-38.3）：
    /// 1. CPU/内存非空；
    /// 2. memory 区互不重叠；reserved 必须落在某 memory 区内；
    /// 3. 设备 MMIO 区互不重叠（两设备共享 reg 视为冲突，fail-closed）；
    /// 4. chosen 角色唯一（console/entropy/boot_storage 互不相同）；
    /// 5. 每设备 compatible 非空。
    pub fn validate(&self) -> Result<ValidatedInventory, InventoryError> {
        if self.cpus.is_empty() {
            return Err(InventoryError::NoCpu);
        }
        if self.memory.is_empty() {
            return Err(InventoryError::NoMemory);
        }
        for (i, m) in self.memory.iter().enumerate() {
            for n in self.memory.iter().skip(i + 1) {
                if m.phys.range_overlaps(m.size, n.phys, n.size) {
                    return Err(InventoryError::MemoryOverlap {
                        a: m.phys,
                        b: n.phys,
                    });
                }
            }
        }
        for r in &self.reserved_memory {
            let inside = self.memory.iter().any(|m| {
                m.phys.range_overlaps(m.size, r.phys, r.size) && r.phys.as_u64() >= m.phys.as_u64()
            });
            if !inside {
                return Err(InventoryError::ReservedOutsideMemory { at: r.phys });
            }
        }
        // 设备 MMIO 冲突：同地址区被两**不同设备**声明（同设备多 reg 区
        // 可合法相邻/部分重叠——如 GICD/GICR 同控制器两个窗口）。
        let mut regs: Vec<(DeviceId, PhysAddr, u64)> = Vec::new();
        for d in &self.devices {
            if d.compatible.is_empty() {
                return Err(InventoryError::DeviceWithoutCompatible { id: d.id });
            }
            for r in &d.regs {
                for (oid, ophys, osize) in &regs {
                    if *oid != d.id && r.phys.range_overlaps(r.size, *ophys, *osize) {
                        return Err(InventoryError::MmioConflict { a: *oid, b: d.id });
                    }
                }
                regs.push((d.id, r.phys, r.size));
            }
        }
        // chosen 唯一性。
        let mut roles: Vec<(&'static str, usize)> = Vec::new();
        for (role, idx) in [
            ("console", self.chosen.console),
            ("entropy", self.chosen.entropy),
            ("boot_storage", self.chosen.boot_storage),
        ] {
            if let Some(i) = idx {
                if roles.iter().any(|(_, j)| *j == i) {
                    return Err(InventoryError::ChosenConflict { role });
                }
                roles.push((role, i));
            }
        }
        Ok(ValidatedInventory {
            inventory: self.clone(),
        })
    }

    /// 规范化字节序列（确定性；装配层哈希后绑定 evidence）。
    ///
    /// 顺序固定：cpus → memory → reserved → interrupt_domains → iommu_domains
    /// → buses → devices → chosen。所有值小端 u64/u32/u16 定宽 + 长度前缀
    /// 字符串——同输入同输出（哈希可重算）。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        push_u32(&mut out, 1); // canonical schema v1
        push_u64(&mut out, self.cpus.len() as u64);
        for c in &self.cpus {
            push_u32(&mut out, c.id);
            push_u32(&mut out, c.enabled as u32);
        }
        push_u64(&mut out, self.memory.len() as u64);
        for m in &self.memory {
            push_u64(&mut out, m.phys.as_u64());
            push_u64(&mut out, m.size);
        }
        push_u64(&mut out, self.reserved_memory.len() as u64);
        for r in &self.reserved_memory {
            push_u64(&mut out, r.phys.as_u64());
            push_u64(&mut out, r.size);
            push_str(&mut out, &r.purpose);
        }
        push_u64(&mut out, self.interrupt_domains.len() as u64);
        for d in &self.interrupt_domains {
            push_u32(&mut out, d.id);
            push_u32(
                &mut out,
                match d.kind {
                    InterruptDomainKind::GicV3 => 1,
                    InterruptDomainKind::Other(v) => 0x8000_0000u32 | v as u32,
                },
            );
        }
        push_u64(&mut out, self.iommu_domains.len() as u64);
        for d in &self.iommu_domains {
            push_u32(&mut out, d.id);
            push_u64(&mut out, d.base.as_u64());
            push_u64(&mut out, d.size);
        }
        push_u64(&mut out, self.buses.len() as u64);
        for b in &self.buses {
            push_str(&mut out, &b.name);
            push_u32(&mut out, b.parent.map(|p| p as u32 + 1).unwrap_or(0));
        }
        push_u64(&mut out, self.devices.len() as u64);
        for d in &self.devices {
            push_u32(&mut out, d.id.0);
            push_u32(&mut out, d.compatible.len() as u32);
            for c in d.compatible.as_slice() {
                push_str(&mut out, c);
            }
            push_u32(
                &mut out,
                match d.status {
                    DeviceStatus::Ok => 0,
                    DeviceStatus::Disabled => 1,
                    DeviceStatus::Fail => 2,
                    DeviceStatus::Reserved => 3,
                },
            );
            push_u64(&mut out, d.regs.len() as u64);
            for r in &d.regs {
                push_u64(&mut out, r.phys.as_u64());
                push_u64(&mut out, r.size);
            }
            push_u64(&mut out, d.interrupts.len() as u64);
            for i in &d.interrupts {
                push_u32(&mut out, i.irq);
                push_u32(&mut out, i.trigger as u32);
            }
            push_u64(&mut out, d.clocks.len() as u64);
            for c in &d.clocks {
                push_u32(&mut out, c.phandle);
                push_u64(&mut out, c.frequency_hz.unwrap_or(u64::MAX));
            }
            push_u64(&mut out, d.resets.len() as u64);
            for r in &d.resets {
                push_u32(&mut out, r.phandle);
                push_u32(&mut out, r.specifier.len() as u32);
                for s in &r.specifier {
                    push_u32(&mut out, *s);
                }
            }
            push_u64(&mut out, d.gpios.len() as u64);
            for g in &d.gpios {
                push_u32(&mut out, g.phandle);
                push_u32(&mut out, g.specifier.len() as u32);
                for s in &g.specifier {
                    push_u32(&mut out, *s);
                }
            }
            push_u64(&mut out, d.dmas.len() as u64);
            for dm in &d.dmas {
                push_u32(&mut out, dm.phandle);
                push_u32(&mut out, dm.specifier.len() as u32);
                for s in &dm.specifier {
                    push_u32(&mut out, *s);
                }
            }
            push_u32(&mut out, d.iommu.as_ref().map(|i| i.phandle).unwrap_or(0));
            push_u32(&mut out, d.iommu.as_ref().map(|i| i.domain_id).unwrap_or(0));
            push_u32(
                &mut out,
                match d.dma_coherency {
                    DmaCoherency::Coherent => 0,
                    DmaCoherency::NonCoherent => 1,
                },
            );
            // 有界扩展属性：name + value（确定性顺序 = 插入顺序）。
            push_u32(&mut out, d.properties.len() as u32);
            for (name, value) in d.properties.entries.iter() {
                push_str(&mut out, name);
                push_u64(&mut out, value.len() as u64);
                for b in value {
                    push_u64(&mut out, *b as u64);
                }
            }
        }
        out
    }
}

fn push_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    push_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

/// 便捷构造（测试/合成平台）。
#[derive(Debug, Default)]
pub struct InventoryBuilder {
    pub cpus: Vec<CpuNode>,
    pub memory: Vec<MemoryRegion>,
    pub reserved_memory: Vec<ReservedRegion>,
    pub interrupt_domains: Vec<InterruptDomain>,
    pub iommu_domains: Vec<IommuDomainSpec>,
    pub buses: Vec<BusNode>,
    pub devices: Vec<DeviceResources>,
    pub chosen: ChosenResources,
}

impl InventoryBuilder {
    pub fn build(self, provenance: PlatformProvenance) -> PlatformInventory {
        PlatformInventory {
            cpus: self.cpus,
            memory: self.memory,
            reserved_memory: self.reserved_memory,
            interrupt_domains: self.interrupt_domains,
            iommu_domains: self.iommu_domains,
            buses: self.buses,
            devices: self.devices,
            chosen: self.chosen,
            provenance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::PhysAddr;
    use crate::dma::DmaCoherency;

    fn dev(id: u32, compat: &str, base: u64, size: u64) -> DeviceResources {
        DeviceResources {
            id: DeviceId(id),
            path: String::from("/soc/device"),
            compatible: NonEmptyVec::new(vec![compat.into()]).unwrap(),
            status: DeviceStatus::Ok,
            regs: vec![MmioRegion {
                phys: PhysAddr::new(base),
                size,
            }],
            interrupts: vec![],
            clocks: vec![],
            resets: vec![],
            gpios: vec![],
            dmas: vec![],
            iommu: None,
            dma_coherency: DmaCoherency::Coherent,
            properties: BoundedPropertyMap::default(),
        }
    }

    fn valid_inventory() -> PlatformInventory {
        InventoryBuilder {
            cpus: vec![CpuNode {
                id: 0,
                enabled: true,
            }],
            memory: vec![MemoryRegion {
                phys: PhysAddr::new(0x4000_0000),
                size: 0x1000_0000,
            }],
            devices: vec![dev(1, "arm,pl011", 0x0900_0000, 0x1000)],
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: DescriptionSource::Synthetic,
            digest: None,
            description: "test".into(),
        })
    }

    #[test]
    fn validate_ok_inventory() {
        assert!(valid_inventory().validate().is_ok());
    }

    #[test]
    fn validate_rejects_no_cpu_no_memory() {
        let inv = InventoryBuilder {
            cpus: vec![],
            memory: vec![MemoryRegion {
                phys: PhysAddr::new(0x4000_0000),
                size: 0x1000,
            }],
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: DescriptionSource::Synthetic,
            digest: None,
            description: "t".into(),
        });
        assert_eq!(inv.validate(), Err(InventoryError::NoCpu));
        let inv2 = InventoryBuilder {
            cpus: vec![CpuNode {
                id: 0,
                enabled: true,
            }],
            memory: vec![],
            ..Default::default()
        }
        .build(PlatformProvenance {
            source: DescriptionSource::Synthetic,
            digest: None,
            description: "t".into(),
        });
        assert_eq!(inv2.validate(), Err(InventoryError::NoMemory));
    }

    #[test]
    fn validate_rejects_memory_overlap() {
        let mut inv = valid_inventory();
        inv.memory.push(MemoryRegion {
            phys: PhysAddr::new(0x4080_0000),
            size: 0x1000,
        });
        assert!(matches!(
            inv.validate(),
            Err(InventoryError::MemoryOverlap { .. })
        ));
    }

    #[test]
    fn validate_rejects_reserved_outside_memory() {
        let mut inv = valid_inventory();
        inv.reserved_memory.push(ReservedRegion {
            phys: PhysAddr::new(0x5000_0000),
            size: 0x1000,
            purpose: "boot-res".into(),
        });
        assert_eq!(
            inv.validate(),
            Err(InventoryError::ReservedOutsideMemory {
                at: PhysAddr::new(0x5000_0000)
            })
        );
    }

    #[test]
    fn validate_rejects_mmio_conflict() {
        let mut inv = valid_inventory();
        // 第二设备与 pl011 同一 MMIO 区 → 冲突。
        inv.devices.push(dev(2, "nxp,sja1000", 0x0900_0000, 0x100));
        assert!(matches!(
            inv.validate(),
            Err(InventoryError::MmioConflict { .. })
        ));
    }

    #[test]
    fn validate_rejects_chosen_conflict() {
        let mut inv = valid_inventory();
        inv.chosen.console = Some(0);
        inv.chosen.entropy = Some(0);
        assert_eq!(
            inv.validate(),
            Err(InventoryError::ChosenConflict { role: "entropy" })
        );
    }

    #[test]
    fn canonical_bytes_deterministic() {
        let a = valid_inventory();
        let b = valid_inventory();
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
        // 顺序不同 → 字节不同（determinism 是顺序固定）。
        let mut c = valid_inventory();
        c.cpus.push(CpuNode {
            id: 1,
            enabled: true,
        });
        assert_ne!(a.canonical_bytes(), c.canonical_bytes());
    }

    #[test]
    fn non_empty_vec_rejects_empty() {
        assert!(NonEmptyVec::<u32>::new(vec![]).is_none());
        assert_eq!(NonEmptyVec::new(vec![1u32]).unwrap().first(), &1);
    }

    #[test]
    fn property_map_bounded() {
        let mut m = BoundedPropertyMap::default();
        for i in 0..MAX_PROPERTIES {
            assert!(m.try_push(format!("p{i}"), vec![i as u8]).is_ok());
        }
        assert_eq!(
            m.try_push("overflow".into(), vec![]),
            Err(InventoryError::TooManyProperties)
        );
        assert_eq!(m.get("p1"), Some(&[1u8][..]));
    }
}
