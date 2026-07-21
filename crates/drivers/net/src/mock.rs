//! Mock implementations for testing.
//!
//! Provides [`MockMacRegs`], an in-memory implementation of [`MacRegs`]
//! that simulates MAC registers and the MII management protocol. This
//! allows PHY and MAC controller logic to be tested without real hardware.

#![cfg(test)]

use alloc::collections::BTreeMap;

use crate::mac::{MacRegs, MAC_MII_ADDR, MAC_MII_DATA, MII_BUSY, MII_WRITE};

/// Mock MAC register set with simulated MII/PHY management.
///
/// Stores MAC registers in a `BTreeMap<u64, u32>` and PHY registers in a
/// `BTreeMap<u8, u16>`. When `MAC_MII_ADDR` is written, the mock simulates
/// the MII management protocol: it reads/writes the PHY register at the
/// specified address and updates `MAC_MII_DATA`, then clears `MII_BUSY`
/// to indicate completion.
pub struct MockMacRegs {
    /// MAC register values (offset → value).
    mac_regs: BTreeMap<u64, u32>,
    /// PHY register values (register address → value).
    phy_regs: BTreeMap<u8, u16>,
    /// PHY address that this mock responds to.
    phy_addr: u8,
}

impl MockMacRegs {
    /// Create a new mock register set responding to the given PHY address.
    pub fn new(phy_addr: u8) -> Self {
        Self {
            mac_regs: BTreeMap::new(),
            phy_regs: BTreeMap::new(),
            phy_addr,
        }
    }

    /// Pre-set a PHY register value (used to simulate PHY state).
    pub fn set_phy_reg(&mut self, reg: u8, value: u16) {
        self.phy_regs.insert(reg, value);
    }

    /// Read a PHY register value (used to verify PHY writes).
    pub fn get_phy_reg(&self, reg: u8) -> Option<u16> {
        self.phy_regs.get(&reg).copied()
    }

    /// Pre-set a MAC register value (used to simulate hardware state).
    pub fn set_mac_reg(&mut self, offset: u64, value: u32) {
        self.mac_regs.insert(offset, value);
    }

    /// Read a MAC register value (used to verify register writes).
    pub fn get_mac_reg(&self, offset: u64) -> Option<u32> {
        self.mac_regs.get(&offset).copied()
    }

    /// Parse the MII address register value.
    ///
    /// Format:
    /// - Bits [15:11]: PHY address (5 bits)
    /// - Bits [10:6]: MII register address (5 bits)
    /// - Bit 1: MII write (1=write, 0=read)
    /// - Bit 0: MII busy
    fn parse_mii_addr(addr: u32) -> (u8, u8, bool) {
        let phy_addr = ((addr >> 11) & 0x1f) as u8;
        let reg = ((addr >> 6) & 0x1f) as u8;
        let is_write = (addr & MII_WRITE) != 0;
        (phy_addr, reg, is_write)
    }
}

impl MacRegs for MockMacRegs {
    fn read(&self, offset: u64) -> u32 {
        self.mac_regs.get(&offset).copied().unwrap_or(0)
    }

    fn write(&mut self, offset: u64, value: u32) {
        if offset == MAC_MII_ADDR {
            // Simulate the MII management protocol.
            let (phy_addr, reg, is_write) = Self::parse_mii_addr(value);

            if phy_addr == self.phy_addr {
                if is_write {
                    // Write: read data from MAC_MII_DATA and store to PHY reg.
                    let data = self.mac_regs.get(&MAC_MII_DATA).copied().unwrap_or(0) as u16;
                    self.phy_regs.insert(reg, data);
                } else {
                    // Read: copy PHY reg value to MAC_MII_DATA.
                    let data = self.phy_regs.get(&reg).copied().unwrap_or(0) as u32;
                    self.mac_regs.insert(MAC_MII_DATA, data);
                }
            }
            // Clear MII_BUSY to indicate the operation is complete.
            self.mac_regs.insert(MAC_MII_ADDR, value & !MII_BUSY);
        } else {
            self.mac_regs.insert(offset, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::{MAC_CR, MAC_FF};
    use crate::phy::{BMCR_RESET, BMSR_LINK, MII_BMCR, MII_BMSR};

    #[test]
    fn test_mock_new() {
        let regs = MockMacRegs::new(0);
        assert_eq!(regs.phy_addr, 0);
        assert_eq!(regs.get_mac_reg(MAC_CR), None);
    }

    #[test]
    fn test_mock_mac_reg_write_read() {
        let mut regs = MockMacRegs::new(0);
        regs.write(MAC_CR, 0x1234);
        assert_eq!(regs.read(MAC_CR), 0x1234);
        assert_eq!(regs.get_mac_reg(MAC_CR), Some(0x1234));
    }

    #[test]
    fn test_mock_mac_reg_default_zero() {
        let regs = MockMacRegs::new(0);
        assert_eq!(regs.read(MAC_CR), 0); // unwritten registers read as 0
    }

    #[test]
    fn test_mock_set_get_phy_reg() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK);
        assert_eq!(regs.get_phy_reg(MII_BMSR), Some(BMSR_LINK));
    }

    #[test]
    fn test_mock_set_get_mac_reg() {
        let mut regs = MockMacRegs::new(0);
        regs.set_mac_reg(MAC_FF, 0x01);
        assert_eq!(regs.get_mac_reg(MAC_FF), Some(0x01));
    }

    #[test]
    fn test_mock_mii_read() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK);

        // Simulate a MII read: write MAC_MII_ADDR with read command
        let addr = ((MII_BMSR as u32) << 6) | MII_BUSY;
        regs.write(MAC_MII_ADDR, addr);

        // MII_BUSY should be cleared
        let mii_addr = regs.read(MAC_MII_ADDR);
        assert_eq!(mii_addr & MII_BUSY, 0);

        // MAC_MII_DATA should contain the PHY register value
        let data = regs.read(MAC_MII_DATA);
        assert_eq!(data as u16, BMSR_LINK);
    }

    #[test]
    fn test_mock_mii_write() {
        let mut regs = MockMacRegs::new(0);

        // Set up MAC_MII_DATA with the value to write
        regs.write(MAC_MII_DATA, BMCR_RESET as u32);

        // Simulate a MII write: write MAC_MII_ADDR with write command
        let addr = ((MII_BMCR as u32) << 6) | MII_BUSY | MII_WRITE;
        regs.write(MAC_MII_ADDR, addr);

        // PHY register should be updated
        assert_eq!(regs.get_phy_reg(MII_BMCR), Some(BMCR_RESET));

        // MII_BUSY should be cleared
        let mii_addr = regs.read(MAC_MII_ADDR);
        assert_eq!(mii_addr & MII_BUSY, 0);
    }

    #[test]
    fn test_mock_mii_wrong_phy_addr() {
        let mut regs = MockMacRegs::new(1); // PHY at address 1
        regs.set_phy_reg(MII_BMSR, BMSR_LINK);

        // Try to read from PHY address 0 (wrong address)
        let addr = ((MII_BMSR as u32) << 6) | MII_BUSY;
        regs.write(MAC_MII_ADDR, addr);

        // MAC_MII_DATA should be 0 (PHY at addr 0 doesn't exist in this mock)
        let data = regs.read(MAC_MII_DATA);
        assert_eq!(data, 0);
    }

    #[test]
    fn test_mock_parse_mii_addr() {
        // PHY addr=3, reg=5, write=1, busy=1
        let addr = (3u32 << 11) | (5u32 << 6) | MII_WRITE | MII_BUSY;
        let (phy, reg, is_write) = MockMacRegs::parse_mii_addr(addr);
        assert_eq!(phy, 3);
        assert_eq!(reg, 5);
        assert!(is_write);

        // PHY addr=0, reg=1, write=0, busy=1
        let addr = (1u32 << 6) | MII_BUSY;
        let (phy, reg, is_write) = MockMacRegs::parse_mii_addr(addr);
        assert_eq!(phy, 0);
        assert_eq!(reg, 1);
        assert!(!is_write);
    }
}
