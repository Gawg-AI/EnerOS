//! ARM64 memory-mapped Ethernet MAC HAL (register-level access).
//!
//! Provides register-level MMIO access to a generic Ethernet MAC for PHY
//! identification and MAC address retrieval. This is a low-level helper used
//! by the future network stack; it does not implement a HAL trait yet.
//!
//! Targets the QEMU `virt` platform network device region (base `0x0901_0000`).

// ---------------------------------------------------------------------------
// MAC register offsets
// ---------------------------------------------------------------------------
#[allow(dead_code)] // MAC register map; MAC_CFG not used yet in v0.7.0
const MAC_CFG: u64 = 0x00; // MAC Configuration
const MAC_ADDR_LOW: u64 = 0x04; // MAC Address Low
const MAC_ADDR_HIGH: u64 = 0x08; // MAC Address High
const MAC_MII_ADDR: u64 = 0x10; // MII Address (PHY register address)
const MAC_MII_DATA: u64 = 0x14; // MII Data (PHY register data)

// ---------------------------------------------------------------------------
// PHY register addresses
// ---------------------------------------------------------------------------
const PHY_ID_HIGH: u8 = 0x02; // PHY ID High register
const PHY_ID_LOW: u8 = 0x03; // PHY ID Low register

// ---------------------------------------------------------------------------
// MII management bits
// ---------------------------------------------------------------------------
const MII_BUSY: u32 = 1 << 0; // MII Busy bit
#[allow(dead_code)] // MII management map; write path not used yet in v0.7.0
const MII_WRITE: u32 = 1 << 1; // MII Write bit
#[allow(dead_code)] // MII management map; read path uses implicit 0
const MII_READ: u32 = 0; // MII Read (Write bit clear)

/// ARM64 memory-mapped Ethernet MAC HAL helper.
pub struct NetMmio {
    mac_base: u64,
}

impl NetMmio {
    /// Create a new NetMmio instance at the given MAC MMIO base address.
    pub const fn new(mac_base: u64) -> Self {
        Self { mac_base }
    }

    /// Write a 32-bit MMIO register.
    #[inline]
    unsafe fn w32(base: u64, off: u64, v: u32) {
        core::ptr::write_volatile((base + off) as *mut u32, v);
    }

    /// Read a 32-bit MMIO register.
    #[inline]
    unsafe fn r32(base: u64, off: u64) -> u32 {
        core::ptr::read_volatile((base + off) as *const u32)
    }

    /// Read a PHY register via MII management.
    ///
    /// Waits for the MII bus to be idle, issues a read of `reg` from the PHY
    /// at `phy_addr`, then waits for completion and returns the 16-bit data.
    pub fn read_phy_reg(&self, phy_addr: u8, reg: u8) -> u16 {
        // Wait for MII not busy.
        while unsafe { Self::r32(self.mac_base, MAC_MII_ADDR) } & MII_BUSY != 0 {
            core::hint::spin_loop();
        }
        // Write MII address: phy_addr in bits [12:8], reg in bits [4:0],
        // write bit cleared (read operation).
        let addr = ((phy_addr as u32) << 8) | (reg as u32);
        unsafe { Self::w32(self.mac_base, MAC_MII_ADDR, addr) };
        // Wait for read to complete.
        while unsafe { Self::r32(self.mac_base, MAC_MII_ADDR) } & MII_BUSY == 0 {
            core::hint::spin_loop();
        }
        // Read data.
        let data = unsafe { Self::r32(self.mac_base, MAC_MII_DATA) };
        data as u16
    }

    /// Read the PHY identifier (high and low halves).
    pub fn read_phy_id(&self) -> (u16, u16) {
        let high = self.read_phy_reg(0, PHY_ID_HIGH);
        let low = self.read_phy_reg(0, PHY_ID_LOW);
        (high, low)
    }

    /// Read the station MAC address (6 octets).
    pub fn read_mac_addr(&self) -> [u8; 6] {
        let low = unsafe { Self::r32(self.mac_base, MAC_ADDR_LOW) };
        let high = unsafe { Self::r32(self.mac_base, MAC_ADDR_HIGH) };
        [
            (low & 0xff) as u8,
            ((low >> 8) & 0xff) as u8,
            ((low >> 16) & 0xff) as u8,
            ((low >> 24) & 0xff) as u8,
            (high & 0xff) as u8,
            ((high >> 8) & 0xff) as u8,
        ]
    }
}

static ARM64_NET: NetMmio = NetMmio::new(0x09010000);

/// Returns the ARM64 network MMIO helper singleton.
pub fn net() -> &'static NetMmio {
    &ARM64_NET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_register_offsets() {
        assert_eq!(MAC_CFG, 0x00);
        assert_eq!(MAC_ADDR_LOW, 0x04);
        assert_eq!(MAC_ADDR_HIGH, 0x08);
        assert_eq!(MAC_MII_ADDR, 0x10);
        assert_eq!(MAC_MII_DATA, 0x14);
    }

    #[test]
    fn phy_register_addresses() {
        assert_eq!(PHY_ID_HIGH, 0x02);
        assert_eq!(PHY_ID_LOW, 0x03);
    }

    #[test]
    fn mii_bits() {
        assert_eq!(MII_BUSY, 1);
    }
}
