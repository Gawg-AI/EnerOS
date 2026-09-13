//! 板级能力族（子系统）与按 SKU 种类派生的必需能力集（WP-70.12.1）。
//!
//! 判据来源：`product/skus/board-selection-criteria.md` 的硬门 C1–C16 被
//! 归并到本枚举；`product/skus/*.json` 的 `missing[]` 文本由此具备可机器
//! 核对的结构（此前只有自由文本，无法判定"缺 TRNG"是否真的阻止资格）。

/// 板级能力族 / 子系统。JSON 名字（kebab-case）即冻结记录 `subsystem` 取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// SoC / 平台标识与修订。
    Soc,
    /// CPU 拓扑与核数。
    Cpu,
    /// 缓存与一致性（DMA coherency 的硬件事实）。
    Cache,
    /// 中断控制器（GIC 版本与寄存器布局）。
    Gic,
    /// 系统定时器（频率与 PPI）。
    Timer,
    /// RAM map（含保留区；[`crate::memory`] 判重叠）。
    Ram,
    /// 存储控制器与介质（sector/flush/掉电语义/C4、C5）。
    Storage,
    /// 链路层以太网（MAC/PHY/MDIO，C6）。
    NetworkPhy,
    /// 现场总线（RS485 收发器/隔离/CAN，C8、C9）。
    Bus,
    /// 硬件真随机源（C10：无 TRNG ⇒ 实机密码面全灭）。
    Trng,
    /// 防回滚单调计数器（eFuse/RPMB，C12）。
    RollbackCounter,
    /// 看门狗与复位原因。
    Watchdog,
    /// 实时时钟。
    Rtc,
    /// 电源轨与掉电通知（C5）。
    Power,
    /// 散热与环境（温度传感器/降频事实）。
    Thermal,
    /// 启动链与固件 ABI（TF-A/UEFI 版本与配置，C11、C13）。
    BootChain,
    /// 设备树 / 寄存器手册与 errata（C14、C16）。
    Docs,
    /// 硬件安全元件（TPM/SE/熔丝）。
    SecureElement,
}

impl Subsystem {
    /// 全部能力族（顺序 = 账本报告顺序）。
    pub const ALL: [Subsystem; 18] = [
        Subsystem::Soc,
        Subsystem::Cpu,
        Subsystem::Cache,
        Subsystem::Gic,
        Subsystem::Timer,
        Subsystem::Ram,
        Subsystem::Storage,
        Subsystem::NetworkPhy,
        Subsystem::Bus,
        Subsystem::Trng,
        Subsystem::RollbackCounter,
        Subsystem::Watchdog,
        Subsystem::Rtc,
        Subsystem::Power,
        Subsystem::Thermal,
        Subsystem::BootChain,
        Subsystem::Docs,
        Subsystem::SecureElement,
    ];

    /// JSON 名（kebab-case，冻结记录 `subsystem` 字段取值）。
    pub fn json_name(self) -> &'static str {
        match self {
            Subsystem::Soc => "soc",
            Subsystem::Cpu => "cpu",
            Subsystem::Cache => "cache",
            Subsystem::Gic => "gic",
            Subsystem::Timer => "timer",
            Subsystem::Ram => "ram",
            Subsystem::Storage => "storage",
            Subsystem::NetworkPhy => "network-phy",
            Subsystem::Bus => "bus",
            Subsystem::Trng => "trng",
            Subsystem::RollbackCounter => "rollback-counter",
            Subsystem::Watchdog => "watchdog",
            Subsystem::Rtc => "rtc",
            Subsystem::Power => "power",
            Subsystem::Thermal => "thermal",
            Subsystem::BootChain => "boot-chain",
            Subsystem::Docs => "docs",
            Subsystem::SecureElement => "secure-element",
        }
    }

    /// 解析 JSON 名；未知取值 → `None`（调用方按显式拒绝处理，不猜）。
    pub fn parse(name: &str) -> Option<Subsystem> {
        Subsystem::ALL
            .iter()
            .copied()
            .find(|s| s.json_name() == name)
    }
}

/// SKU 种类（与 `compatibility-catalog.json` 的 `kind` 同域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkuKind {
    /// 开发平台（QEMU/合成）：不承载真实外设事实，不得用真实硬件判据。
    Development,
    /// 产品板卡：启动/资源/外设事实必须齐备（缺失即未资格化）。
    Product,
}

impl SkuKind {
    pub fn parse(name: &str) -> Option<SkuKind> {
        match name {
            "development" => Some(SkuKind::Development),
            "product" => Some(SkuKind::Product),
            _ => None,
        }
    }

    /// 该种类下**必须**有已验证事实的能力族。
    ///
    /// 开发平台只要求能在合成平台上启动的最低事实集（SoC/CPU/RAM/定时器/
    /// 固件与启动链）；产品板卡追加控制闭环所需的外设与安全根（C4–C12）。
    /// 返回切片按 [`Subsystem::ALL`] 顺序。
    pub fn required(self) -> &'static [Subsystem] {
        const DEVELOPMENT: [Subsystem; 6] = [
            Subsystem::Soc,
            Subsystem::Cpu,
            Subsystem::Ram,
            Subsystem::Timer,
            Subsystem::BootChain,
            Subsystem::Gic,
        ];
        const PRODUCT: [Subsystem; 13] = [
            Subsystem::Soc,
            Subsystem::Cpu,
            Subsystem::Ram,
            Subsystem::Timer,
            Subsystem::BootChain,
            Subsystem::Gic,
            Subsystem::Storage,
            Subsystem::NetworkPhy,
            Subsystem::Bus,
            Subsystem::Trng,
            Subsystem::RollbackCounter,
            Subsystem::Power,
            Subsystem::Watchdog,
        ];
        match self {
            SkuKind::Development => &DEVELOPMENT,
            SkuKind::Product => &PRODUCT,
        }
    }
}

/// 一条必需能力：能力族 + 判据来源 + 未满足时的后果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub subsystem: Subsystem,
    /// 选板判据编号（C1–C16）——证据化用途，不是装饰。
    pub criterion: &'static str,
}

impl CapabilityRequirement {
    pub const fn new(subsystem: Subsystem, criterion: &'static str) -> Self {
        Self {
            subsystem,
            criterion,
        }
    }
}

/// 按种类给出必需能力（含判据编号；与 [`SkuKind::required`] 同源）。
pub fn requirements_for(kind: SkuKind) -> alloc::vec::Vec<CapabilityRequirement> {
    let criterion = |s: Subsystem| -> &'static str {
        match s {
            Subsystem::Soc | Subsystem::Docs => "C14",
            Subsystem::Cpu => "C1",
            Subsystem::Gic => "C2",
            Subsystem::Ram => "C3",
            Subsystem::Storage => "C4/C5",
            Subsystem::NetworkPhy => "C6",
            Subsystem::Bus => "C8/C9",
            Subsystem::Trng => "C10",
            Subsystem::RollbackCounter => "C12",
            Subsystem::Watchdog | Subsystem::Rtc => "C5",
            Subsystem::Power => "C5",
            Subsystem::Thermal => "C15",
            Subsystem::BootChain => "C11/C13",
            Subsystem::SecureElement => "C11",
            Subsystem::Cache => "C2",
            Subsystem::Timer => "C2",
        }
    };
    kind.required()
        .iter()
        .copied()
        .map(|s| CapabilityRequirement::new(s, criterion(s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_names_are_unique_and_roundtrip() {
        let mut seen = alloc::collections::BTreeSet::new();
        for s in Subsystem::ALL {
            assert!(
                seen.insert(s.json_name()),
                "duplicate name {}",
                s.json_name()
            );
            assert_eq!(Subsystem::parse(s.json_name()), Some(s));
        }
        assert_eq!(Subsystem::ALL.len(), seen.len());
    }

    #[test]
    fn unknown_name_is_rejected_not_guessed() {
        assert_eq!(Subsystem::parse("ramm"), None);
        assert_eq!(Subsystem::parse(""), None);
        assert_eq!(Subsystem::parse("RAM"), None, "大小写敏感（禁静默纠正）");
    }

    #[test]
    fn product_requirements_are_superset_of_development() {
        let dev: alloc::collections::BTreeSet<_> = SkuKind::Development.required().iter().collect();
        let prod: alloc::collections::BTreeSet<_> = SkuKind::Product.required().iter().collect();
        assert!(dev.is_subset(&prod));
        // 硬门族必须在产品必需集中：无 TRNG / 无防回滚 / 无介质 / 无收发器
        // 都不得通过产品资格（蓝图 C10/C12/C4/C8）。
        assert!(prod.contains(&Subsystem::Trng));
        assert!(prod.contains(&Subsystem::RollbackCounter));
        assert!(prod.contains(&Subsystem::Storage));
        assert!(prod.contains(&Subsystem::Bus));
        // 开发平台不因缺真实外设事实被拒（仍不得宣称硬件实证）。
        assert!(!dev.contains(&Subsystem::Trng));
    }

    #[test]
    fn requirements_carry_criteria_ids() {
        let reqs = requirements_for(SkuKind::Product);
        assert!(reqs
            .iter()
            .any(|r| r.subsystem == Subsystem::Trng && r.criterion == "C10"));
        assert_eq!(reqs.len(), SkuKind::Product.required().len());
    }
}
