//! DMA protection domain guard.
//!
//! Provides [`DmaGuard`] trait and [`SmmuGuard`] implementation for
//! DMA access control. Each device must be authorized to access a
//! physical address range before performing DMA.

use crate::partition::PaddrRange;
use crate::vspace::MmError;

/// Maximum number of DMA protection domains.
const MAX_DMA_DOMAINS: usize = 16;

/// Device identifier (newtype).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId(pub u32);

/// A DMA protection domain mapping a device to an allowed physical range.
#[derive(Clone, Copy, Debug)]
pub struct DmaDomain {
    /// The partition that owns this device.
    pub owner_partition: u32,
    /// The physical address range the device is allowed to access.
    pub allowed_phys: PaddrRange,
}

/// Abstraction over DMA access control.
pub trait DmaGuard {
    /// Authorize a device to access a physical address range.
    fn authorize(&mut self, dev: DeviceId, range: PaddrRange) -> Result<(), MmError>;
    /// Check whether a device is authorized to access `pa`.
    fn check(&self, dev: DeviceId, pa: u64) -> Result<(), MmError>;
}

/// Software-based DMA guard using an array of protection domains.
///
/// In a real system, `authorize` would configure the SMMU/IOMMU page
/// tables. This implementation stores domains in a fixed-size array
/// and performs software checks.
pub struct SmmuGuard {
    /// DMA protection domains (None = empty slot).
    pub domains: [Option<(DeviceId, DmaDomain)>; MAX_DMA_DOMAINS],
}

impl SmmuGuard {
    /// Creates a new SmmuGuard with no domains.
    pub fn new() -> Self {
        Self {
            domains: [None; MAX_DMA_DOMAINS],
        }
    }
}

impl Default for SmmuGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaGuard for SmmuGuard {
    fn authorize(&mut self, dev: DeviceId, range: PaddrRange) -> Result<(), MmError> {
        // Find an empty slot or replace existing entry for this device
        for slot in self.domains.iter_mut() {
            if let Some((id, _)) = slot {
                if *id == dev {
                    // Replace existing authorization
                    *slot = Some((
                        dev,
                        DmaDomain {
                            owner_partition: dev.0,
                            allowed_phys: range,
                        },
                    ));
                    return Ok(());
                }
            }
        }

        // Find first empty slot
        for slot in self.domains.iter_mut() {
            if slot.is_none() {
                *slot = Some((
                    dev,
                    DmaDomain {
                        owner_partition: dev.0,
                        allowed_phys: range,
                    },
                ));
                return Ok(());
            }
        }

        Err(MmError::OutOfMemory)
    }

    fn check(&self, dev: DeviceId, pa: u64) -> Result<(), MmError> {
        for (id, domain) in self.domains.iter().flatten() {
            if *id == dev && domain.allowed_phys.contains(pa) {
                return Ok(());
            }
        }
        Err(MmError::PermissionDenied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id() {
        let a = DeviceId(1);
        let b = DeviceId(1);
        let c = DeviceId(2);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_smmu_guard_new() {
        let g = SmmuGuard::new();
        for slot in &g.domains {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn test_authorize_and_check_allowed() {
        let mut g = SmmuGuard::new();
        let dev = DeviceId(1);
        let range = PaddrRange::new(0x1000, 0x2000);

        assert!(g.authorize(dev, range).is_ok());
        assert!(g.check(dev, 0x1000).is_ok());
        assert!(g.check(dev, 0x1500).is_ok());
        assert!(g.check(dev, 0x1FFF).is_ok());
    }

    #[test]
    fn test_check_denied_unauthorized_device() {
        let g = SmmuGuard::new();
        let dev = DeviceId(1);
        assert_eq!(g.check(dev, 0x1000), Err(MmError::PermissionDenied));
    }

    #[test]
    fn test_check_denied_out_of_range() {
        let mut g = SmmuGuard::new();
        let dev = DeviceId(1);
        g.authorize(dev, PaddrRange::new(0x1000, 0x2000)).unwrap();

        assert_eq!(g.check(dev, 0x2000), Err(MmError::PermissionDenied));
        assert_eq!(g.check(dev, 0x0FFF), Err(MmError::PermissionDenied));
    }

    #[test]
    fn test_authorize_multiple_devices() {
        let mut g = SmmuGuard::new();
        let dev1 = DeviceId(1);
        let dev2 = DeviceId(2);

        g.authorize(dev1, PaddrRange::new(0x1000, 0x2000)).unwrap();
        g.authorize(dev2, PaddrRange::new(0x3000, 0x4000)).unwrap();

        assert!(g.check(dev1, 0x1500).is_ok());
        assert!(g.check(dev2, 0x3500).is_ok());
        assert_eq!(g.check(dev1, 0x3500), Err(MmError::PermissionDenied));
        assert_eq!(g.check(dev2, 0x1500), Err(MmError::PermissionDenied));
    }

    #[test]
    fn test_authorize_replace_existing() {
        let mut g = SmmuGuard::new();
        let dev = DeviceId(1);

        g.authorize(dev, PaddrRange::new(0x1000, 0x2000)).unwrap();
        assert!(g.check(dev, 0x1500).is_ok());

        // Replace with new range
        g.authorize(dev, PaddrRange::new(0x3000, 0x4000)).unwrap();
        assert_eq!(g.check(dev, 0x1500), Err(MmError::PermissionDenied));
        assert!(g.check(dev, 0x3500).is_ok());
    }
}
