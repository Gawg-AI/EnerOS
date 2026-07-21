//! MAC controller driver and `NetDevice` trait.
//!
//! Provides [`MacController`], a generic Ethernet MAC driver that manages
//! DMA rings, PHY communication, and frame TX/RX. Register access is
//! abstracted via the [`MacRegs`] trait, enabling both real hardware
//! ([`MmioMacRegs`]) and mock testing ([`MockMacRegs`](crate::mock::MockMacRegs)).

use alloc::vec;
use alloc::vec::Vec;

use crate::dma_ring::{DmaRing, DESC_FS, DESC_IOC, DESC_LS, DESC_OWN};
use crate::error::{NetError, NetStats};
use crate::phy::{GenericPhy, PhyState};

// ---------------------------------------------------------------------------
// MAC register offsets (relative to MAC base address)
// ---------------------------------------------------------------------------
/// MAC Configuration Register.
pub const MAC_CR: u64 = 0x00;
/// MAC Frame Filter Register (bit 0 = promiscuous mode).
pub const MAC_FF: u64 = 0x04;
/// MAC MII Address Register (PHY register access control).
pub const MAC_MII_ADDR: u64 = 0x10;
/// MAC MII Data Register (PHY register data).
pub const MAC_MII_DATA: u64 = 0x14;
/// DMA TX Poll Demand Register.
pub const DMA_TX_POLL: u64 = 0x48;
/// DMA RX Poll Demand Register.
pub const DMA_RX_POLL: u64 = 0x4C;
/// DMA Status Register (interrupt cause).
pub const DMA_STATUS: u64 = 0x60;

// ---------------------------------------------------------------------------
// MII management bits (within MAC_MII_ADDR)
// ---------------------------------------------------------------------------
/// MII Busy bit (bit 0) — set to initiate an MII transaction.
pub const MII_BUSY: u32 = 1 << 0;
/// MII Write bit (bit 1) — set for write, clear for read.
pub const MII_WRITE: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// MAC_CR bits
// ---------------------------------------------------------------------------
/// MAC_CR: Enable TX.
pub const MAC_CR_TX_ENABLE: u32 = 1 << 0;
/// MAC_CR: Enable RX.
pub const MAC_CR_RX_ENABLE: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// DMA_STATUS bits
// ---------------------------------------------------------------------------
/// DMA_STATUS: TX interrupt.
pub const DMA_STATUS_TX_INT: u32 = 1 << 0;
/// DMA_STATUS: RX interrupt.
pub const DMA_STATUS_RX_INT: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// MAC_FF bits
// ---------------------------------------------------------------------------
/// MAC_FF: Promiscuous mode (bit 0).
pub const MAC_FF_PROMISC: u32 = 1 << 0;

/// Trait abstracting MAC register read/write operations.
///
/// Implementations include [`MmioMacRegs`] for real hardware and
/// [`MockMacRegs`](crate::mock::MockMacRegs) for testing.
pub trait MacRegs {
    /// Read a 32-bit register at the given offset.
    fn read(&self, offset: u64) -> u32;
    /// Write a 32-bit value to the register at the given offset.
    fn write(&mut self, offset: u64, value: u32);
}

/// Unified network device abstraction for all network interfaces.
///
/// Implemented by [`MacController`] and any future virtual/software
/// network devices. Used by the v0.28.0 TCP/IP protocol stack.
pub trait NetDevice {
    /// Send a raw Ethernet frame.
    fn send(&mut self, frame: &[u8]) -> Result<(), NetError>;
    /// Receive a frame into `buf`; returns the number of bytes read.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError>;
    /// Return the device MAC address.
    fn mac_address(&self) -> [u8; 6];
    /// Return the maximum transmission unit (bytes).
    fn mtu(&self) -> usize;
    /// Return whether the link is up.
    fn link_up(&self) -> bool;
    /// Enable or disable promiscuous mode.
    fn set_promiscuous(&mut self, on: bool);
    /// Return network statistics.
    fn stats(&self) -> NetStats;
}

/// Ethernet MAC controller driver.
///
/// Manages DMA TX/RX rings, PHY communication, and frame buffering.
/// Generic over `R: MacRegs` to support both MMIO hardware and mock testing.
pub struct MacController<R: MacRegs> {
    /// MAC register accessor.
    pub regs: R,
    /// Station MAC address.
    pub mac_addr: [u8; 6],
    /// Maximum transmission unit (bytes, includes Ethernet header).
    pub mtu: usize,
    /// DMA descriptor ring (TX + RX).
    pub dma: DmaRing,
    /// TX frame buffers (one per TX descriptor).
    pub tx_buffers: Vec<Vec<u8>>,
    /// RX frame buffers (one per RX descriptor).
    pub rx_buffers: Vec<Vec<u8>>,
    /// Network statistics.
    pub stats: NetStats,
    /// Promiscuous mode flag.
    pub promiscuous: bool,
    /// Generic PHY driver.
    pub phy: GenericPhy,
    /// Device has been initialized.
    pub initialized: bool,
}

impl<R: MacRegs> MacController<R> {
    /// Create a new MAC controller.
    ///
    /// Allocates TX/RX DMA rings with `tx_count` / `rx_count` descriptors
    /// and corresponding frame buffers of size `mtu`.
    pub fn new(
        regs: R,
        mac_addr: [u8; 6],
        mtu: usize,
        tx_count: usize,
        rx_count: usize,
        phy_addr: u8,
    ) -> Self {
        let dma = DmaRing::new(tx_count, rx_count);
        let tx_buffers = vec![vec![0u8; mtu]; tx_count];
        let rx_buffers = vec![vec![0u8; mtu]; rx_count];
        Self {
            regs,
            mac_addr,
            mtu,
            dma,
            tx_buffers,
            rx_buffers,
            stats: NetStats::new(),
            promiscuous: false,
            phy: GenericPhy::new(phy_addr),
            initialized: false,
        }
    }

    /// Initialize the MAC controller.
    ///
    /// Configures DMA rings (sets RX descriptors to DMA-owned with IOC),
    /// enables TX/RX in MAC_CR, and starts PHY autonegotiation.
    pub fn init(&mut self) -> Result<(), NetError> {
        // Initialize RX descriptors: give to DMA with interrupt-on-completion.
        for desc in &mut self.dma.rx_desc {
            desc.set_owned_by_dma(true);
            desc.set_ioc(true);
        }
        // Set rx_head to rx_count (all descriptors given to DMA).
        // This way rx_head wraps to 0, and rx_tail stays at 0.
        // Actually, we use rx_head == rx_tail to mean "empty", so after
        // giving all descriptors to DMA, rx_head should equal rx_tail
        // (no frames received yet). The descriptors are DMA-owned, so
        // rx_dequeue will return None until DMA releases them.
        // We leave rx_head = 0 and rx_tail = 0 (initial state).

        // Enable TX and RX in MAC_CR.
        self.regs.write(MAC_CR, MAC_CR_TX_ENABLE | MAC_CR_RX_ENABLE);

        // Configure frame filter (no promiscuous by default).
        self.regs.write(MAC_FF, 0);

        // Start PHY autonegotiation.
        self.phy.autoneg(&mut self.regs)?;

        self.initialized = true;
        Ok(())
    }

    /// Handle a DMA interrupt.
    ///
    /// Processes TX completions (advances tx_tail for descriptors where
    /// DMA has cleared OWN) and optionally triggers RX polling.
    pub fn handle_irq(&mut self) {
        // Process TX completions: advance tail for completed descriptors.
        while !self.dma.tx_is_empty() {
            let tail = self.dma.tx_tail as usize;
            if self.dma.tx_desc[tail].is_owned_by_dma() {
                // DMA still owns this descriptor — not done yet.
                break;
            }
            self.dma.tx_advance_tail();
        }

        // Clear DMA_STATUS by writing 1s (write-to-clear semantics).
        self.regs.write(DMA_STATUS, 0xFFFF_FFFF);
    }

    /// Read a PHY register via the MAC MII management interface.
    pub fn read_phy_reg(&mut self, reg: u8) -> u16 {
        self.phy.read_reg(&mut self.regs, reg).unwrap_or(0)
    }

    /// Write a PHY register via the MAC MII management interface.
    pub fn write_phy_reg(&mut self, reg: u8, val: u16) {
        let _ = self.phy.write_reg(&mut self.regs, reg, val);
    }

    /// Return the current PHY link state.
    pub fn phy_state(&self) -> PhyState {
        self.phy.link_state()
    }

    /// Simulate a TX completion (test helper).
    ///
    /// Clears the OWN bit on the TX descriptor at `tx_tail`, simulating
    /// the DMA engine completing a transmission. Used in tests to verify
    /// `handle_irq` logic.
    #[cfg(test)]
    pub fn simulate_tx_completion(&mut self) {
        if !self.dma.tx_is_empty() {
            let tail = self.dma.tx_tail as usize;
            self.dma.tx_desc[tail].set_owned_by_dma(false);
        }
    }

    /// Simulate an RX frame arrival (test helper).
    ///
    /// Finds the next DMA-owned RX descriptor, writes the frame data to
    /// the corresponding buffer, and clears OWN to simulate the DMA
    /// engine releasing a received frame.
    #[cfg(test)]
    pub fn simulate_rx_frame(&mut self, frame: &[u8]) {
        // Find a DMA-owned descriptor to simulate frame reception.
        for i in 0..self.dma.rx_count() {
            if self.dma.rx_desc[i].is_owned_by_dma() {
                let copy_len = frame.len().min(self.rx_buffers[i].len());
                self.rx_buffers[i][..copy_len].copy_from_slice(&frame[..copy_len]);
                self.dma.rx_desc[i].buffer_length = copy_len as u32;
                self.dma.rx_desc[i].set_owned_by_dma(false);
                // Advance rx_head to reflect this descriptor being "in flight".
                // We set rx_head to (i + 1) % count to indicate a frame is available.
                let count = self.dma.rx_count() as u32;
                self.dma.rx_head = (i as u32 + 1) % count;
                return;
            }
        }
    }
}

impl<R: MacRegs> NetDevice for MacController<R> {
    fn send(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.initialized {
            self.stats.record_tx_error();
            return Err(NetError::NotInitialized);
        }
        if !self.phy.state.link_up {
            self.stats.record_tx_error();
            return Err(NetError::LinkDown);
        }
        if frame.len() > self.mtu {
            self.stats.record_tx_error();
            return Err(NetError::FrameTooLarge {
                size: frame.len(),
                max: self.mtu,
            });
        }

        let idx = match self.dma.tx_enqueue() {
            Some(i) => i,
            None => {
                self.stats.record_tx_error();
                return Err(NetError::NoBuffer);
            }
        };

        // Copy frame to TX buffer.
        let buf = &mut self.tx_buffers[idx];
        buf[..frame.len()].copy_from_slice(frame);

        // Update descriptor: set OWN (give to DMA), FS+LS (single segment), IOC.
        let desc = &mut self.dma.tx_desc[idx];
        desc.buffer_length = frame.len() as u32;
        desc.flags = DESC_OWN | DESC_FS | DESC_LS | DESC_IOC;

        // Trigger TX polling.
        self.regs.write(DMA_TX_POLL, 1);

        self.stats.record_tx(frame.len());
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        if !self.initialized {
            self.stats.record_rx_error();
            return Err(NetError::NotInitialized);
        }

        let idx = match self.dma.rx_dequeue() {
            Some(i) => i,
            None => {
                self.stats.record_rx_error();
                return Err(NetError::NoBuffer);
            }
        };

        let frame_len = self.dma.rx_desc[idx].buffer_length as usize;
        let copy_len = frame_len.min(buf.len());

        // Copy frame from RX buffer to user buffer.
        buf[..copy_len].copy_from_slice(&self.rx_buffers[idx][..copy_len]);

        // Recycle the descriptor (give back to DMA).
        self.dma.rx_recycle(idx);

        // Trigger RX polling.
        self.regs.write(DMA_RX_POLL, 1);

        self.stats.record_rx(copy_len);
        Ok(copy_len)
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac_addr
    }

    fn mtu(&self) -> usize {
        self.mtu
    }

    fn link_up(&self) -> bool {
        self.phy.state.link_up
    }

    fn set_promiscuous(&mut self, on: bool) {
        self.promiscuous = on;
        let ff = if on { MAC_FF_PROMISC } else { 0 };
        self.regs.write(MAC_FF, ff);
    }

    fn stats(&self) -> NetStats {
        self.stats
    }
}

/// Memory-mapped MAC register accessor (real hardware).
///
/// Uses `read_volatile`/`write_volatile` for MMIO access. Only available
/// on `aarch64` targets.
#[cfg(target_arch = "aarch64")]
pub struct MmioMacRegs {
    base_addr: u64,
}

#[cfg(target_arch = "aarch64")]
impl MmioMacRegs {
    /// Create a new MMIO register accessor at the given base address.
    pub const fn new(base_addr: u64) -> Self {
        Self { base_addr }
    }
}

#[cfg(target_arch = "aarch64")]
impl MacRegs for MmioMacRegs {
    fn read(&self, offset: u64) -> u32 {
        // SAFETY: The caller must ensure `base_addr + offset` points to a
        // valid memory-mapped 32-bit register.
        unsafe { core::ptr::read_volatile((self.base_addr + offset) as *const u32) }
    }

    fn write(&mut self, offset: u64, value: u32) {
        // SAFETY: The caller must ensure `base_addr + offset` points to a
        // valid memory-mapped 32-bit register.
        unsafe {
            core::ptr::write_volatile((self.base_addr + offset) as *mut u32, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockMacRegs;
    use crate::phy::{ANLPAR_1000_FULL, BMSR_ANEG_COMPLETE, BMSR_LINK, MII_ANLPAR, MII_BMSR};

    fn setup_controller(
        tx_count: usize,
        rx_count: usize,
        phy_addr: u8,
    ) -> MacController<MockMacRegs> {
        let mut regs = MockMacRegs::new(phy_addr);
        // Set up PHY registers for successful autoneg
        regs.set_phy_reg(MII_BMSR, BMSR_LINK | BMSR_ANEG_COMPLETE);
        regs.set_phy_reg(MII_ANLPAR, ANLPAR_1000_FULL);
        MacController::new(
            regs,
            [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
            1500,
            tx_count,
            rx_count,
            phy_addr,
        )
    }

    fn init_controller(ctrl: &mut MacController<MockMacRegs>) {
        ctrl.init().expect("init should succeed");
    }

    fn sample_frame(len: usize) -> Vec<u8> {
        let mut frame = vec![0u8; len];
        // dst MAC
        frame[0..6].copy_from_slice(&[0xFF; 6]);
        // src MAC
        frame[6..12].copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        // ethertype
        frame[12] = 0x08;
        frame[13] = 0x00;
        // payload
        for (i, byte) in frame.iter_mut().enumerate() {
            if i >= 14 {
                *byte = (i & 0xff) as u8;
            }
        }
        frame
    }

    #[test]
    fn test_mac_register_constants() {
        assert_eq!(MAC_CR, 0x00);
        assert_eq!(MAC_FF, 0x04);
        assert_eq!(MAC_MII_ADDR, 0x10);
        assert_eq!(MAC_MII_DATA, 0x14);
        assert_eq!(DMA_TX_POLL, 0x48);
        assert_eq!(DMA_RX_POLL, 0x4C);
        assert_eq!(DMA_STATUS, 0x60);
    }

    #[test]
    fn test_mii_bits() {
        assert_eq!(MII_BUSY, 1);
        assert_eq!(MII_WRITE, 2);
    }

    #[test]
    fn test_mac_controller_new() {
        let ctrl = setup_controller(16, 32, 0);
        assert_eq!(ctrl.mac_addr, [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!(ctrl.mtu, 1500);
        assert_eq!(ctrl.dma.tx_count(), 16);
        assert_eq!(ctrl.dma.rx_count(), 32);
        assert!(!ctrl.initialized);
        assert!(!ctrl.promiscuous);
        assert_eq!(ctrl.tx_buffers.len(), 16);
        assert_eq!(ctrl.rx_buffers.len(), 32);
        assert_eq!(ctrl.tx_buffers[0].len(), 1500);
    }

    #[test]
    fn test_init_success() {
        let mut ctrl = setup_controller(4, 4, 0);
        let result = ctrl.init();
        assert!(result.is_ok());
        assert!(ctrl.initialized);
        // RX descriptors should be DMA-owned after init
        for desc in &ctrl.dma.rx_desc {
            assert!(desc.is_owned_by_dma());
        }
        // Link should be up after autoneg
        assert!(ctrl.phy.state.link_up);
    }

    #[test]
    fn test_init_phy_autoneg_failure() {
        let mut regs = MockMacRegs::new(0);
        // BMSR without ANEG_COMPLETE — autoneg will time out
        regs.set_phy_reg(MII_BMSR, 0);
        let mut ctrl = MacController::new(regs, [0; 6], 1500, 4, 4, 0);
        let result = ctrl.init();
        assert_eq!(result, Err(NetError::Timeout));
        assert!(!ctrl.initialized);
    }

    #[test]
    fn test_send_not_initialized() {
        let mut ctrl = setup_controller(4, 4, 0);
        let frame = sample_frame(64);
        let result = ctrl.send(&frame);
        assert_eq!(result, Err(NetError::NotInitialized));
        assert_eq!(ctrl.stats.tx_errors, 1);
    }

    #[test]
    fn test_send_link_down() {
        let mut ctrl = setup_controller(4, 4, 0);
        ctrl.initialized = true;
        // Link is down by default (PHY state not updated)
        let frame = sample_frame(64);
        let result = ctrl.send(&frame);
        assert_eq!(result, Err(NetError::LinkDown));
        assert_eq!(ctrl.stats.tx_errors, 1);
    }

    #[test]
    fn test_send_frame_too_large() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let large_frame = sample_frame(1501);
        let result = ctrl.send(&large_frame);
        assert_eq!(
            result,
            Err(NetError::FrameTooLarge {
                size: 1501,
                max: 1500
            })
        );
        assert_eq!(ctrl.stats.tx_errors, 1);
    }

    #[test]
    fn test_send_no_buffer() {
        let mut ctrl = setup_controller(2, 2, 0);
        init_controller(&mut ctrl);
        // Ring size 2 can hold 1 entry (one slot wasted)
        let frame = sample_frame(64);
        // First send succeeds
        assert!(ctrl.send(&frame).is_ok());
        // Second send fails (ring full)
        let result = ctrl.send(&frame);
        assert_eq!(result, Err(NetError::NoBuffer));
        assert_eq!(ctrl.stats.tx_errors, 1);
    }

    #[test]
    fn test_send_success() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(64);
        let result = ctrl.send(&frame);
        assert!(result.is_ok());
        assert_eq!(ctrl.stats.tx_packets, 1);
        assert_eq!(ctrl.stats.tx_bytes, 64);
        // Check descriptor was set up correctly
        let desc = &ctrl.dma.tx_desc[0];
        assert!(desc.is_owned_by_dma());
        assert!(desc.is_first());
        assert!(desc.is_last());
        assert_eq!(desc.buffer_length, 64);
        // Check frame was copied to buffer
        assert_eq!(&ctrl.tx_buffers[0][..64], &frame[..]);
    }

    #[test]
    fn test_send_multiple_frames() {
        let mut ctrl = setup_controller(8, 8, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(100);
        for _ in 0..5 {
            assert!(ctrl.send(&frame).is_ok());
        }
        assert_eq!(ctrl.stats.tx_packets, 5);
        assert_eq!(ctrl.stats.tx_bytes, 500);
    }

    #[test]
    fn test_handle_irq_tx_completion() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(64);
        ctrl.send(&frame).unwrap();
        assert_eq!(ctrl.dma.tx_pending(), 1);

        // Simulate DMA completing the TX
        ctrl.simulate_tx_completion();

        // handle_irq should process the completion
        ctrl.handle_irq();
        assert_eq!(ctrl.dma.tx_pending(), 0);
        assert!(ctrl.dma.tx_is_empty());
    }

    #[test]
    fn test_handle_irq_no_completion() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(64);
        ctrl.send(&frame).unwrap();
        // Don't simulate completion — handle_irq should not advance tail
        ctrl.handle_irq();
        assert_eq!(ctrl.dma.tx_pending(), 1);
    }

    #[test]
    fn test_handle_irq_empty() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        // No pending TX — handle_irq should be a no-op
        ctrl.handle_irq();
        assert!(ctrl.dma.tx_is_empty());
    }

    #[test]
    fn test_recv_not_initialized() {
        let mut ctrl = setup_controller(4, 4, 0);
        let mut buf = [0u8; 1500];
        let result = ctrl.recv(&mut buf);
        assert_eq!(result, Err(NetError::NotInitialized));
        assert_eq!(ctrl.stats.rx_errors, 1);
    }

    #[test]
    fn test_recv_no_frame() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let mut buf = [0u8; 1500];
        let result = ctrl.recv(&mut buf);
        assert_eq!(result, Err(NetError::NoBuffer));
        assert_eq!(ctrl.stats.rx_errors, 1);
    }

    #[test]
    fn test_recv_success() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(128);
        ctrl.simulate_rx_frame(&frame);

        let mut buf = [0u8; 1500];
        let result = ctrl.recv(&mut buf);
        assert!(result.is_ok());
        let len = result.unwrap();
        assert_eq!(len, 128);
        assert_eq!(&buf[..128], &frame[..]);
        assert_eq!(ctrl.stats.rx_packets, 1);
        assert_eq!(ctrl.stats.rx_bytes, 128);
    }

    #[test]
    fn test_recv_buffer_too_small() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(128);
        ctrl.simulate_rx_frame(&frame);

        let mut buf = [0u8; 64];
        let result = ctrl.recv(&mut buf);
        assert!(result.is_ok());
        let len = result.unwrap();
        assert_eq!(len, 64); // truncated
        assert_eq!(&buf[..64], &frame[..64]);
    }

    #[test]
    fn test_recv_recycles_descriptor() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(64);
        ctrl.simulate_rx_frame(&frame);

        let mut buf = [0u8; 1500];
        ctrl.recv(&mut buf).unwrap();

        // Descriptor should be DMA-owned again (recycled)
        assert!(ctrl.dma.rx_desc[0].is_owned_by_dma());

        // No more frames available
        let result = ctrl.recv(&mut buf);
        assert_eq!(result, Err(NetError::NoBuffer));
    }

    #[test]
    fn test_mac_address() {
        let ctrl = setup_controller(4, 4, 0);
        assert_eq!(ctrl.mac_address(), [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn test_mtu() {
        let ctrl = setup_controller(4, 4, 0);
        assert_eq!(ctrl.mtu(), 1500);
    }

    #[test]
    fn test_link_up_before_init() {
        let ctrl = setup_controller(4, 4, 0);
        assert!(!ctrl.link_up()); // PHY state is default (link down)
    }

    #[test]
    fn test_link_up_after_init() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        assert!(ctrl.link_up());
    }

    #[test]
    fn test_set_promiscuous_on() {
        let mut ctrl = setup_controller(4, 4, 0);
        ctrl.set_promiscuous(true);
        assert!(ctrl.promiscuous);
        assert_eq!(ctrl.regs.get_mac_reg(MAC_FF), Some(MAC_FF_PROMISC));
    }

    #[test]
    fn test_set_promiscuous_off() {
        let mut ctrl = setup_controller(4, 4, 0);
        ctrl.set_promiscuous(true);
        ctrl.set_promiscuous(false);
        assert!(!ctrl.promiscuous);
        assert_eq!(ctrl.regs.get_mac_reg(MAC_FF), Some(0));
    }

    #[test]
    fn test_stats_initial() {
        let ctrl = setup_controller(4, 4, 0);
        let stats = ctrl.stats();
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_packets, 0);
    }

    #[test]
    fn test_stats_after_send_recv() {
        let mut ctrl = setup_controller(8, 8, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(100);
        ctrl.send(&frame).unwrap();
        ctrl.send(&frame).unwrap();

        ctrl.simulate_rx_frame(&frame);
        let mut buf = [0u8; 1500];
        ctrl.recv(&mut buf).unwrap();

        let stats = ctrl.stats();
        assert_eq!(stats.tx_packets, 2);
        assert_eq!(stats.tx_bytes, 200);
        assert_eq!(stats.rx_packets, 1);
        assert_eq!(stats.rx_bytes, 100);
    }

    #[test]
    fn test_read_phy_reg() {
        let mut ctrl = setup_controller(4, 4, 0);
        ctrl.regs.set_phy_reg(MII_BMSR, BMSR_LINK);
        let val = ctrl.read_phy_reg(MII_BMSR);
        assert_eq!(val, BMSR_LINK);
    }

    #[test]
    fn test_write_phy_reg() {
        let mut ctrl = setup_controller(4, 4, 0);
        ctrl.write_phy_reg(MII_BMSR, 0x1234);
        assert_eq!(ctrl.regs.get_phy_reg(MII_BMSR), Some(0x1234));
    }

    #[test]
    fn test_phy_state_after_init() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let state = ctrl.phy_state();
        assert!(state.link_up);
        assert!(state.autoneg_complete);
    }

    #[test]
    fn test_send_recv_roundtrip() {
        let mut ctrl = setup_controller(8, 8, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(256);

        // Send a frame
        ctrl.send(&frame).unwrap();

        // Simulate TX completion
        ctrl.simulate_tx_completion();
        ctrl.handle_irq();

        // Simulate receiving the same frame back
        ctrl.simulate_rx_frame(&frame);

        // Receive it
        let mut buf = [0u8; 1500];
        let len = ctrl.recv(&mut buf).unwrap();
        assert_eq!(len, 256);
        assert_eq!(&buf[..256], &frame[..]);
    }

    #[test]
    fn test_send_fills_ring_then_frees() {
        let mut ctrl = setup_controller(3, 3, 0);
        init_controller(&mut ctrl);
        let frame = sample_frame(64);

        // Size 3 can hold 2 entries
        assert!(ctrl.send(&frame).is_ok());
        assert!(ctrl.send(&frame).is_ok());
        assert_eq!(ctrl.send(&frame), Err(NetError::NoBuffer));

        // Complete one TX
        ctrl.simulate_tx_completion();
        ctrl.handle_irq();

        // Now there should be space again
        assert!(ctrl.send(&frame).is_ok());
    }

    #[test]
    fn test_multiple_recv() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);

        // Simulate two frames arriving
        let frame1 = sample_frame(64);
        let frame2 = sample_frame(128);
        ctrl.simulate_rx_frame(&frame1);
        // Need to advance rx_tail for the second frame to be detected
        // Actually, simulate_rx_frame advances rx_head, so the second call
        // will find a different descriptor.
        // But we need to be careful: after the first simulate, rx_head=1.
        // The second simulate finds desc[1] (DMA-owned), writes frame, clears OWN,
        // sets rx_head=2.
        ctrl.simulate_rx_frame(&frame2);

        let mut buf = [0u8; 1500];
        let len1 = ctrl.recv(&mut buf).unwrap();
        assert_eq!(len1, 64);
        assert_eq!(&buf[..64], &frame1[..64]);

        let len2 = ctrl.recv(&mut buf).unwrap();
        assert_eq!(len2, 128);
        assert_eq!(&buf[..128], &frame2[..128]);

        // No more frames
        assert_eq!(ctrl.recv(&mut buf), Err(NetError::NoBuffer));
    }

    #[test]
    fn test_init_configures_mac_cr() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        let cr = ctrl.regs.get_mac_reg(MAC_CR);
        assert_eq!(cr, Some(MAC_CR_TX_ENABLE | MAC_CR_RX_ENABLE));
    }

    #[test]
    fn test_dma_status_cleared_after_irq() {
        let mut ctrl = setup_controller(4, 4, 0);
        init_controller(&mut ctrl);
        // Set a fake DMA_STATUS
        ctrl.regs.set_mac_reg(DMA_STATUS, 0xFF);
        ctrl.handle_irq();
        // DMA_STATUS should be cleared (write 0xFFFFFFFF to clear)
        assert_eq!(ctrl.regs.get_mac_reg(DMA_STATUS), Some(0xFFFF_FFFF));
    }
}
