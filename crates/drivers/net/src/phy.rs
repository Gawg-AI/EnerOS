//! PHY driver — autonegotiation and link state management.
//!
//! Provides [`GenericPhy`], a generic PHY driver that communicates with any
//! IEEE 802.3 compliant PHY via the standard MII management interface. The
//! driver does not own the MAC register set; instead, methods accept a
//! `&mut R: MacRegs` reference, allowing the [`MacController`](crate::mac::MacController)
//! to share its registers.

use core::fmt;

use crate::error::NetError;
use crate::mac::{MacRegs, MAC_MII_ADDR, MAC_MII_DATA, MII_BUSY, MII_WRITE};

// ---------------------------------------------------------------------------
// MII register addresses (IEEE 802.3 §22.2.4)
// ---------------------------------------------------------------------------
/// MII Basic Mode Control Register.
pub const MII_BMCR: u8 = 0x00;
/// MII Basic Mode Status Register.
pub const MII_BMSR: u8 = 0x01;
/// MII PHY Identifier High.
pub const MII_PHYID1: u8 = 0x02;
/// MII PHY Identifier Low.
pub const MII_PHYID2: u8 = 0x03;
/// MII Auto-Negotiation Advertisement.
pub const MII_ANAR: u8 = 0x04;
/// MII Auto-Negotiation Link Partner Ability.
pub const MII_ANLPAR: u8 = 0x05;

// ---------------------------------------------------------------------------
// BMCR bit definitions
// ---------------------------------------------------------------------------
/// BMCR Reset bit (bit 15).
pub const BMCR_RESET: u16 = 0x8000;
/// BMCR Autonegotiation Enable bit (bit 12).
pub const BMCR_AUTONEG: u16 = 0x1000;
/// BMCR Restart Autonegotiation bit (bit 9).
pub const BMCR_RESTART: u16 = 0x0200;
/// BMCR Speed Selection bit (bit 13) — 1 = 1000 Mbps.
pub const BMCR_SPEED1000: u16 = 0x2000;
/// BMCR Speed Selection bit (bit 6) — 1 = 100 Mbps.
pub const BMCR_SPEED100: u16 = 0x0040;
/// BMCR Duplex Mode bit (bit 8) — 1 = full duplex.
pub const BMCR_FULL_DUPLEX: u16 = 0x0100;

// ---------------------------------------------------------------------------
// BMSR bit definitions
// ---------------------------------------------------------------------------
/// BMSR Link Status bit (bit 2) — 1 = link up.
pub const BMSR_LINK: u16 = 0x0004;
/// BMSR Autonegotiation Complete bit (bit 5).
pub const BMSR_ANEG_COMPLETE: u16 = 0x0020;
/// BMSR Autonegotiation Ability bit (bit 3).
pub const BMSR_ANEG_ABILITY: u16 = 0x0008;

// ---------------------------------------------------------------------------
// ANLPAR speed/duplex bits
// ---------------------------------------------------------------------------
/// ANLPAR 1000M Full (bit 10 in 1000BASE-T extended status; simplified).
pub const ANLPAR_1000_FULL: u16 = 0x0400;
/// ANLPAR 100M Full (bit 8).
pub const ANLPAR_100_FULL: u16 = 0x0100;
/// ANLPAR 100M Half (bit 7).
pub const ANLPAR_100_HALF: u16 = 0x0080;
/// ANLPAR 10M Full (bit 6).
pub const ANLPAR_10_FULL: u16 = 0x0040;
/// ANLPAR 10M Half (bit 5).
pub const ANLPAR_10_HALF: u16 = 0x0020;

/// Maximum autonegotiation poll cycles before timeout.
const AUTONEG_MAX_POLLS: u32 = 100_000;

/// PHY link speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhySpeed {
    /// 10 Mbps.
    #[default]
    Speed10M,
    /// 100 Mbps.
    Speed100M,
    /// 1000 Mbps (1 Gbps).
    Speed1000M,
}

impl fmt::Display for PhySpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhySpeed::Speed10M => write!(f, "10 Mbps"),
            PhySpeed::Speed100M => write!(f, "100 Mbps"),
            PhySpeed::Speed1000M => write!(f, "1000 Mbps"),
        }
    }
}

/// PHY duplex mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhyDuplex {
    /// Half duplex.
    #[default]
    Half,
    /// Full duplex.
    Full,
}

impl fmt::Display for PhyDuplex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhyDuplex::Half => write!(f, "Half"),
            PhyDuplex::Full => write!(f, "Full"),
        }
    }
}

/// PHY state snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhyState {
    /// Link is up.
    pub link_up: bool,
    /// Negotiated link speed.
    pub speed: PhySpeed,
    /// Negotiated duplex mode.
    pub duplex: PhyDuplex,
    /// Autonegotiation has completed.
    pub autoneg_complete: bool,
}

impl Default for PhyState {
    fn default() -> Self {
        Self {
            link_up: false,
            speed: PhySpeed::Speed10M,
            duplex: PhyDuplex::Half,
            autoneg_complete: false,
        }
    }
}

/// Trait for PHY drivers.
///
/// This trait is defined for future PHY-specific implementations (e.g.,
/// RTL8211, YT8521). The generic [`GenericPhy`] does not implement this
/// trait because it requires a `MacRegs` reference for register access.
pub trait PhyDriver {
    /// Reset the PHY (write BMCR reset bit).
    fn reset(&mut self) -> Result<(), NetError>;
    /// Start autonegotiation and wait for completion.
    fn autoneg(&mut self) -> Result<PhyState, NetError>;
    /// Read a PHY register.
    fn read_reg(&self, reg: u8) -> Result<u16, NetError>;
    /// Write a PHY register.
    fn write_reg(&mut self, reg: u8, val: u16) -> Result<(), NetError>;
    /// Return the current cached link state.
    fn link_state(&self) -> PhyState;
}

/// Generic IEEE 802.3 PHY driver.
///
/// Operates any standard PHY via the MII management interface. Does not own
/// the MAC registers — methods accept a `&mut R: MacRegs` reference so the
/// [`MacController`](crate::mac::MacController) can share its register set.
pub struct GenericPhy {
    /// PHY address on the MII bus (0–31).
    pub phy_addr: u8,
    /// Cached link state (updated by `autoneg` / `update_link_state`).
    pub state: PhyState,
}

impl GenericPhy {
    /// Create a new generic PHY driver at the given MII address.
    pub fn new(phy_addr: u8) -> Self {
        Self {
            phy_addr,
            state: PhyState::default(),
        }
    }

    /// Build the MII address register value.
    ///
    /// Format:
    /// - Bits [15:11]: PHY address (5 bits)
    /// - Bits [10:6]: MII register address (5 bits)
    /// - Bit 1: MII write (1=write, 0=read)
    /// - Bit 0: MII busy
    fn build_mii_addr(&self, reg: u8, write: bool) -> u32 {
        let mut addr =
            ((self.phy_addr as u32 & 0x1f) << 11) | ((reg as u32 & 0x1f) << 6) | MII_BUSY;
        if write {
            addr |= MII_WRITE;
        }
        addr
    }

    /// Read a PHY register via MII management.
    ///
    /// Writes the MII address register with the PHY address and register
    /// number, waits for the busy bit to clear, then reads MII_DATA.
    pub fn read_reg<R: MacRegs>(&self, regs: &mut R, reg: u8) -> Result<u16, NetError> {
        let addr = self.build_mii_addr(reg, false);
        regs.write(MAC_MII_ADDR, addr);
        // Poll for completion (busy bit clears).
        for _ in 0..AUTONEG_MAX_POLLS {
            let status = regs.read(MAC_MII_ADDR);
            if status & MII_BUSY == 0 {
                let data = regs.read(MAC_MII_DATA);
                return Ok(data as u16);
            }
            core::hint::spin_loop();
        }
        Err(NetError::Timeout)
    }

    /// Write a PHY register via MII management.
    ///
    /// Writes the data to MII_DATA, then writes the MII address register
    /// with the write bit set, and waits for the busy bit to clear.
    pub fn write_reg<R: MacRegs>(&self, regs: &mut R, reg: u8, val: u16) -> Result<(), NetError> {
        regs.write(MAC_MII_DATA, val as u32);
        let addr = self.build_mii_addr(reg, true);
        regs.write(MAC_MII_ADDR, addr);
        // Poll for completion.
        for _ in 0..AUTONEG_MAX_POLLS {
            let status = regs.read(MAC_MII_ADDR);
            if status & MII_BUSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(NetError::Timeout)
    }

    /// Reset the PHY by setting the BMCR reset bit.
    pub fn reset<R: MacRegs>(&mut self, regs: &mut R) -> Result<(), NetError> {
        self.write_reg(regs, MII_BMCR, BMCR_RESET)?;
        Ok(())
    }

    /// Start autonegotiation and wait for completion.
    ///
    /// Writes BMCR with AUTONEG | RESTART, then polls BMSR for the
    /// ANEG_COMPLETE bit. Returns the negotiated [`PhyState`] on success
    /// or [`NetError::Timeout`] if autoneg does not complete within the
    /// poll limit.
    pub fn autoneg<R: MacRegs>(&mut self, regs: &mut R) -> Result<PhyState, NetError> {
        // Enable autonegotiation and restart it.
        self.write_reg(regs, MII_BMCR, BMCR_AUTONEG | BMCR_RESTART)?;

        // Poll BMSR for autoneg completion.
        for _ in 0..AUTONEG_MAX_POLLS {
            let bmsr = self.read_reg(regs, MII_BMSR)?;
            if bmsr & BMSR_ANEG_COMPLETE != 0 {
                let state = self.update_link_state(regs);
                return Ok(state);
            }
            core::hint::spin_loop();
        }
        Err(NetError::Timeout)
    }

    /// Update the cached link state from PHY registers.
    ///
    /// Reads BMSR for link status and ANLPAR for negotiated speed/duplex.
    pub fn update_link_state<R: MacRegs>(&mut self, regs: &mut R) -> PhyState {
        let bmsr = self.read_reg(regs, MII_BMSR).unwrap_or(0);
        let anlpar = self.read_reg(regs, MII_ANLPAR).unwrap_or(0);

        let link_up = bmsr & BMSR_LINK != 0;
        let autoneg_complete = bmsr & BMSR_ANEG_COMPLETE != 0;

        // Determine speed and duplex from link partner ability.
        let (speed, duplex) = if anlpar & ANLPAR_1000_FULL != 0 {
            (PhySpeed::Speed1000M, PhyDuplex::Full)
        } else if anlpar & ANLPAR_100_FULL != 0 {
            (PhySpeed::Speed100M, PhyDuplex::Full)
        } else if anlpar & ANLPAR_100_HALF != 0 {
            (PhySpeed::Speed100M, PhyDuplex::Half)
        } else if anlpar & ANLPAR_10_FULL != 0 {
            (PhySpeed::Speed10M, PhyDuplex::Full)
        } else {
            (PhySpeed::Speed10M, PhyDuplex::Half)
        };

        self.state = PhyState {
            link_up,
            speed,
            duplex,
            autoneg_complete,
        };
        self.state
    }

    /// Return the cached link state (does not read PHY registers).
    pub fn link_state(&self) -> PhyState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockMacRegs;

    fn setup_phy_with_link() -> (GenericPhy, MockMacRegs) {
        let mut regs = MockMacRegs::new(0);
        // BMSR: link up + autoneg complete
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        // ANLPAR: 1000M full duplex
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_1000_FULL);
        let phy = GenericPhy::new(0);
        (phy, regs)
    }

    #[test]
    fn test_phy_speed_display() {
        assert_eq!(format!("{}", PhySpeed::Speed10M), "10 Mbps");
        assert_eq!(format!("{}", PhySpeed::Speed100M), "100 Mbps");
        assert_eq!(format!("{}", PhySpeed::Speed1000M), "1000 Mbps");
    }

    #[test]
    fn test_phy_duplex_display() {
        assert_eq!(format!("{}", PhyDuplex::Half), "Half");
        assert_eq!(format!("{}", PhyDuplex::Full), "Full");
    }

    #[test]
    fn test_phy_state_default() {
        let state = PhyState::default();
        assert!(!state.link_up);
        assert_eq!(state.speed, PhySpeed::Speed10M);
        assert_eq!(state.duplex, PhyDuplex::Half);
        assert!(!state.autoneg_complete);
    }

    #[test]
    fn test_mii_register_constants() {
        assert_eq!(MII_BMCR, 0x00);
        assert_eq!(MII_BMSR, 0x01);
        assert_eq!(MII_PHYID1, 0x02);
        assert_eq!(MII_PHYID2, 0x03);
        assert_eq!(MII_ANAR, 0x04);
        assert_eq!(MII_ANLPAR, 0x05);
    }

    #[test]
    fn test_bmcr_constants() {
        assert_eq!(BMCR_RESET, 0x8000);
        assert_eq!(BMCR_AUTONEG, 0x1000);
        assert_eq!(BMCR_RESTART, 0x0200);
    }

    #[test]
    fn test_bmsr_constants() {
        assert_eq!(BMSR_LINK, 0x0004);
        assert_eq!(BMSR_ANEG_COMPLETE, 0x0020);
    }

    #[test]
    fn test_generic_phy_new() {
        let phy = GenericPhy::new(3);
        assert_eq!(phy.phy_addr, 3);
        assert!(!phy.state.link_up);
    }

    #[test]
    fn test_read_reg_basic() {
        let (phy, mut regs) = setup_phy_with_link();
        let bmsr = phy.read_reg(&mut regs, MII_BMSR);
        assert_eq!(bmsr, Ok(BMSR_LINK | BMSR_ANEG_COMPLETE));
    }

    #[test]
    fn test_read_reg_phyid() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_PHYID1, 0x1234);
        let phy = GenericPhy::new(0);
        let val = phy.read_reg(&mut regs, MII_PHYID1);
        assert_eq!(val, Ok(0x1234));
    }

    #[test]
    fn test_write_reg_basic() {
        let mut regs = MockMacRegs::new(0);
        let phy = GenericPhy::new(0);
        let result = phy.write_reg(&mut regs, MII_BMCR, BMCR_RESET);
        assert!(result.is_ok());
        assert_eq!(regs.get_phy_reg(MII_BMCR), Some(BMCR_RESET));
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let mut regs = MockMacRegs::new(0);
        let phy = GenericPhy::new(0);
        // Write a value
        phy.write_reg(&mut regs, MII_ANAR, 0x0DE0).unwrap();
        // Read it back
        let val = phy.read_reg(&mut regs, MII_ANAR);
        assert_eq!(val, Ok(0x0DE0));
    }

    #[test]
    fn test_reset() {
        let mut regs = MockMacRegs::new(0);
        let mut phy = GenericPhy::new(0);
        let result = phy.reset(&mut regs);
        assert!(result.is_ok());
        assert_eq!(regs.get_phy_reg(MII_BMCR), Some(BMCR_RESET));
    }

    #[test]
    fn test_autoneg_success() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_1000_FULL);
        let mut phy = GenericPhy::new(0);
        let state = phy.autoneg(&mut regs).expect("autoneg should succeed");
        assert!(state.link_up);
        assert!(state.autoneg_complete);
        assert_eq!(state.speed, PhySpeed::Speed1000M);
        assert_eq!(state.duplex, PhyDuplex::Full);
    }

    #[test]
    fn test_autoneg_100m_full() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_100_FULL);
        let mut phy = GenericPhy::new(0);
        let state = phy.autoneg(&mut regs).unwrap();
        assert_eq!(state.speed, PhySpeed::Speed100M);
        assert_eq!(state.duplex, PhyDuplex::Full);
    }

    #[test]
    fn test_autoneg_100m_half() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_100_HALF);
        let mut phy = GenericPhy::new(0);
        let state = phy.autoneg(&mut regs).unwrap();
        assert_eq!(state.speed, PhySpeed::Speed100M);
        assert_eq!(state.duplex, PhyDuplex::Half);
    }

    #[test]
    fn test_autoneg_10m_full() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_10_FULL);
        let mut phy = GenericPhy::new(0);
        let state = phy.autoneg(&mut regs).unwrap();
        assert_eq!(state.speed, PhySpeed::Speed10M);
        assert_eq!(state.duplex, PhyDuplex::Full);
    }

    #[test]
    fn test_autoneg_10m_half_default() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, 0); // no capability bits set
        let mut phy = GenericPhy::new(0);
        let state = phy.autoneg(&mut regs).unwrap();
        assert_eq!(state.speed, PhySpeed::Speed10M);
        assert_eq!(state.duplex, PhyDuplex::Half);
    }

    #[test]
    fn test_autoneg_timeout() {
        let mut regs = MockMacRegs::new(0);
        // BMSR without ANEG_COMPLETE — autoneg never finishes
        regs.set_phy_reg(MII_BMSR, 0);
        let mut phy = GenericPhy::new(0);
        let result = phy.autoneg(&mut regs);
        assert_eq!(result, Err(NetError::Timeout));
    }

    #[test]
    fn test_update_link_state() {
        let mut regs = MockMacRegs::new(0);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_100_FULL);
        let mut phy = GenericPhy::new(0);
        let state = phy.update_link_state(&mut regs);
        assert!(state.link_up);
        assert!(state.autoneg_complete);
        assert_eq!(state.speed, PhySpeed::Speed100M);
        assert_eq!(state.duplex, PhyDuplex::Full);
    }

    #[test]
    fn test_link_state_cached() {
        let mut phy = GenericPhy::new(0);
        // Before any update, state is default
        let state = phy.link_state();
        assert!(!state.link_up);
        // Manually update state
        phy.state = PhyState {
            link_up: true,
            speed: PhySpeed::Speed1000M,
            duplex: PhyDuplex::Full,
            autoneg_complete: true,
        };
        let cached = phy.link_state();
        assert!(cached.link_up);
        assert_eq!(cached.speed, PhySpeed::Speed1000M);
    }

    #[test]
    fn test_phy_addr_isolation() {
        // PHY at address 1 should not be affected by writes to address 0.
        let mut regs = MockMacRegs::new(1);
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        let phy = GenericPhy::new(1);
        let bmsr = phy.read_reg(&mut regs, MII_BMSR);
        assert_eq!(bmsr, Ok(BMSR_LINK | BMSR_ANEG_COMPLETE));
    }
}
