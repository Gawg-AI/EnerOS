//! 38（蓝图 38 §7）：CapabilityRegistry——由成功 Started 的 driver 发布的能力注册表。
//!
//! # 不变量（蓝图 38 §7）
//! - capability 含版本、owner、epoch、质量；业务只按 capability 请求，
//!   不按 device ID/compatible；
//! - 缺 required capability 阻断对应 service readiness（装配层检查）；
//! - optional 能力缺失产生显式 product degradation；
//! - 注册表有界（防恶意膨胀）；发布前必须驱动已 Started+Healthy
//!   （生命周期由 [`super::DriverSupervisor`] 保证）；
//! - "全 ARM 兼容"证据是相同核心面对不同 inventory 能正确组合/拒绝。

extern crate alloc;

use alloc::vec::Vec;

/// 能力种类（蓝图 38 §7 列表；版本 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityKind {
    /// 控制台（串口输出/输入）。
    Console,
    /// 熵源。
    Entropy,
    /// 单调时钟。
    MonotonicClock,
    /// 墙上时钟（RTC）。
    WallClock,
    /// 块设备（role/capacity/sector/flush 属 39 扩展）。
    BlockDevice,
    /// 网络接口（39 扩展）。
    NetworkInterface,
    /// 串行总线（RS232/RS485 等）。
    SerialBus,
    /// CAN 总线。
    CanBus,
    /// 看门狗。
    Watchdog,
    /// TPM。
    Tpm,
    /// GPIO。
    Gpio,
    /// PTP 时钟。
    PtpClock,
    /// 电源控制。
    PowerControl,
}

impl CapabilityKind {
    /// 能力名（证据行/日志使用；保持稳定）。
    pub const fn name(self) -> &'static str {
        match self {
            CapabilityKind::Console => "console",
            CapabilityKind::Entropy => "entropy",
            CapabilityKind::MonotonicClock => "monotonic-clock",
            CapabilityKind::WallClock => "wall-clock",
            CapabilityKind::BlockDevice => "block-device",
            CapabilityKind::NetworkInterface => "network-interface",
            CapabilityKind::SerialBus => "serial-bus",
            CapabilityKind::CanBus => "can-bus",
            CapabilityKind::Watchdog => "watchdog",
            CapabilityKind::Tpm => "tpm",
            CapabilityKind::Gpio => "gpio",
            CapabilityKind::PtpClock => "ptp-clock",
            CapabilityKind::PowerControl => "power-control",
        }
    }
}

/// 能力质量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityQuality {
    /// 完整可用。
    Full,
    /// 降级（缺失部分功能；不阻断非关键路径）。
    Degraded,
}

/// 一条已发布能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub kind: CapabilityKind,
    /// 能力版本（驱动/适配器声明；业务按版本协商）。
    pub version: u32,
    /// owner 服务/驱动名（`DeviceDriver::name`；驱动名非 'static，用 String）。
    pub owner: alloc::string::String,
    /// 发布时 epoch（服务重启后变化——旧句柄必须失效）。
    pub epoch: u64,
    pub quality: CapabilityQuality,
}

/// 注册表容量（有界；恶意平台描述最多产生 64 条）。
pub const MAX_CAPABILITIES: usize = 64;

/// 能力注册表（root 装配层持有；业务只按 kind 查询）。
#[derive(Debug, Clone, Default)]
pub struct CapabilityRegistry {
    caps: Vec<Capability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self { caps: Vec::new() }
    }

    /// 发布（调用方保证驱动已 Started+Healthy；同一 kind+owner 覆盖更新）。
    pub fn publish(&mut self, cap: Capability) -> Result<(), &'static str> {
        if self.caps.len() >= MAX_CAPABILITIES {
            return Err("capability registry full");
        }
        if let Some(existing) = self
            .caps
            .iter_mut()
            .find(|c| c.kind == cap.kind && c.owner == cap.owner)
        {
            *existing = cap;
        } else {
            self.caps.push(cap);
        }
        Ok(())
    }

    /// 按 kind 查询（首个；无 → None——调用方据此阻断/降级）。
    pub fn get(&self, kind: CapabilityKind) -> Option<&Capability> {
        self.caps.iter().find(|c| c.kind == kind)
    }

    /// 是否全部 required 能力已发布。
    pub fn has_all(&self, required: &[CapabilityKind]) -> bool {
        required.iter().all(|k| self.get(*k).is_some())
    }

    /// 缺失的 required 能力（readiness 阻断判据）。
    pub fn missing(&self, required: &[CapabilityKind]) -> Vec<CapabilityKind> {
        required
            .iter()
            .copied()
            .filter(|k| self.get(*k).is_none())
            .collect()
    }

    /// 全部已发布能力（证据行）。
    pub fn all(&self) -> &[Capability] {
        &self.caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(kind: CapabilityKind, owner: &str, epoch: u64) -> Capability {
        Capability {
            kind,
            version: 1,
            owner: owner.to_owned(),
            epoch,
            quality: CapabilityQuality::Full,
        }
    }

    #[test]
    fn publish_and_query() {
        let mut reg = CapabilityRegistry::new();
        reg.publish(cap(CapabilityKind::Console, "pl011-uart0", 7))
            .unwrap();
        assert!(reg.get(CapabilityKind::Console).is_some());
        assert!(reg.get(CapabilityKind::Entropy).is_none());
        assert_eq!(
            reg.get(CapabilityKind::Console).unwrap().owner,
            "pl011-uart0"
        );
    }

    #[test]
    fn same_kind_owner_replaces() {
        let mut reg = CapabilityRegistry::new();
        reg.publish(cap(CapabilityKind::Console, "pl011-uart0", 7))
            .unwrap();
        reg.publish(cap(CapabilityKind::Console, "pl011-uart0", 8))
            .unwrap();
        assert_eq!(reg.get(CapabilityKind::Console).unwrap().epoch, 8);
        assert_eq!(reg.all().len(), 1);
    }

    #[test]
    fn required_capability_gating() {
        let mut reg = CapabilityRegistry::new();
        reg.publish(cap(CapabilityKind::Console, "pl011-uart0", 1))
            .unwrap();
        // 缺 MonotonicClock → has_all false，missing 列出。
        let required = [CapabilityKind::Console, CapabilityKind::MonotonicClock];
        assert!(!reg.has_all(&required));
        assert_eq!(reg.missing(&required), vec![CapabilityKind::MonotonicClock]);
        reg.publish(cap(CapabilityKind::MonotonicClock, "arm64-timer", 1))
            .unwrap();
        assert!(reg.has_all(&required));
        assert!(reg.missing(&required).is_empty());
    }

    #[allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）
    #[test]
    fn registry_bounded() {
        let mut reg = CapabilityRegistry::new();
        for i in 0..MAX_CAPABILITIES + 1 {
            let c = cap(CapabilityKind::Gpio, &format!("d{i}"), i as u64);
            if reg.publish(c).is_err() {
                assert_eq!(reg.all().len(), MAX_CAPABILITIES);
                return;
            }
        }
        panic!("注册表必须拒绝超限");
    }

    #[test]
    fn capability_names_stable() {
        assert_eq!(CapabilityKind::Console.name(), "console");
        assert_eq!(CapabilityKind::MonotonicClock.name(), "monotonic-clock");
        assert_eq!(CapabilityKind::BlockDevice.name(), "block-device");
    }
}
