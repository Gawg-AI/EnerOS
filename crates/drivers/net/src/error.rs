//! Error types and statistics for the EnerOS network driver.
//!
//! Defines [`NetError`] covering all network driver failure modes and
//! [`NetStats`] for TX/RX counters used by [`crate::NetDevice`].

use core::fmt;

/// Network driver error type (9 variants).
///
/// Covers initialization, link, buffer, DMA, frame validation, PHY, and
/// timeout failures that can occur during [`crate::NetDevice`] operations.
#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    /// Device has not been initialized (call `init()` first).
    NotInitialized,
    /// Link is down — cannot send/receive.
    LinkDown,
    /// No buffer available (TX ring full or RX ring empty).
    NoBuffer,
    /// DMA controller reported an error (carries the DMA status register).
    DmaError(u32),
    /// Frame exceeds the maximum transmission unit.
    FrameTooLarge { size: usize, max: usize },
    /// Frame is shorter than the minimum Ethernet header (14 bytes).
    FrameTooSmall,
    /// Frame failed CRC check.
    CrcError,
    /// PHY register access or autonegotiation failure.
    PhyError,
    /// Operation timed out (e.g. PHY autoneg polling).
    Timeout,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::NotInitialized => write!(f, "network device not initialized"),
            NetError::LinkDown => write!(f, "link is down"),
            NetError::NoBuffer => write!(f, "no buffer available"),
            NetError::DmaError(status) => write!(f, "DMA error (status=0x{:08x})", status),
            NetError::FrameTooLarge { size, max } => {
                write!(f, "frame too large: {} bytes (max {})", size, max)
            }
            NetError::FrameTooSmall => write!(f, "frame too small (< 14 bytes)"),
            NetError::CrcError => write!(f, "CRC check failed"),
            NetError::PhyError => write!(f, "PHY error"),
            NetError::Timeout => write!(f, "operation timed out"),
        }
    }
}

/// Network statistics counters.
///
/// All fields are `u64` to avoid overflow on long-running devices. Updated
/// by [`crate::NetDevice`] implementations on every send/recv call.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetStats {
    /// Total packets transmitted.
    pub tx_packets: u64,
    /// Total packets received.
    pub rx_packets: u64,
    /// Total bytes transmitted.
    pub tx_bytes: u64,
    /// Total bytes received.
    pub rx_bytes: u64,
    /// Total TX errors (link down, no buffer, DMA error, etc.).
    pub tx_errors: u64,
    /// Total RX errors (CRC, frame too small, etc.).
    pub rx_errors: u64,
    /// Total RX packets dropped (no buffer, buffer too small, etc.).
    pub rx_dropped: u64,
}

impl NetStats {
    /// Create a new zero-initialized stats struct.
    pub const fn new() -> Self {
        Self {
            tx_packets: 0,
            rx_packets: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            tx_errors: 0,
            rx_errors: 0,
            rx_dropped: 0,
        }
    }

    /// Record a successful TX of `len` bytes.
    pub fn record_tx(&mut self, len: usize) {
        self.tx_packets += 1;
        self.tx_bytes += len as u64;
    }

    /// Record a successful RX of `len` bytes.
    pub fn record_rx(&mut self, len: usize) {
        self.rx_packets += 1;
        self.rx_bytes += len as u64;
    }

    /// Record a TX error.
    pub fn record_tx_error(&mut self) {
        self.tx_errors += 1;
    }

    /// Record an RX error.
    pub fn record_rx_error(&mut self) {
        self.rx_errors += 1;
    }

    /// Record a dropped RX packet.
    pub fn record_rx_drop(&mut self) {
        self.rx_dropped += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_initialized_display() {
        let err = NetError::NotInitialized;
        assert_eq!(format!("{}", err), "network device not initialized");
    }

    #[test]
    fn test_link_down_display() {
        let err = NetError::LinkDown;
        assert_eq!(format!("{}", err), "link is down");
    }

    #[test]
    fn test_no_buffer_display() {
        let err = NetError::NoBuffer;
        assert_eq!(format!("{}", err), "no buffer available");
    }

    #[test]
    fn test_dma_error_display() {
        let err = NetError::DmaError(0x0000_0080);
        assert_eq!(format!("{}", err), "DMA error (status=0x00000080)");
    }

    #[test]
    fn test_frame_too_large_display() {
        let err = NetError::FrameTooLarge {
            size: 2000,
            max: 1500,
        };
        assert_eq!(format!("{}", err), "frame too large: 2000 bytes (max 1500)");
    }

    #[test]
    fn test_frame_too_small_display() {
        let err = NetError::FrameTooSmall;
        assert_eq!(format!("{}", err), "frame too small (< 14 bytes)");
    }

    #[test]
    fn test_crc_error_display() {
        let err = NetError::CrcError;
        assert_eq!(format!("{}", err), "CRC check failed");
    }

    #[test]
    fn test_phy_error_display() {
        let err = NetError::PhyError;
        assert_eq!(format!("{}", err), "PHY error");
    }

    #[test]
    fn test_timeout_display() {
        let err = NetError::Timeout;
        assert_eq!(format!("{}", err), "operation timed out");
    }

    #[test]
    fn test_error_equality() {
        assert_eq!(NetError::LinkDown, NetError::LinkDown);
        assert_ne!(NetError::LinkDown, NetError::Timeout);
        assert_eq!(NetError::DmaError(0x42), NetError::DmaError(0x42));
        assert_ne!(NetError::DmaError(0x42), NetError::DmaError(0x43));
    }

    #[test]
    fn test_net_stats_default() {
        let stats = NetStats::default();
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_packets, 0);
        assert_eq!(stats.tx_bytes, 0);
        assert_eq!(stats.rx_bytes, 0);
        assert_eq!(stats.tx_errors, 0);
        assert_eq!(stats.rx_errors, 0);
        assert_eq!(stats.rx_dropped, 0);
    }

    #[test]
    fn test_net_stats_new() {
        let stats = NetStats::new();
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_packets, 0);
    }

    #[test]
    fn test_record_tx() {
        let mut stats = NetStats::new();
        stats.record_tx(100);
        stats.record_tx(200);
        assert_eq!(stats.tx_packets, 2);
        assert_eq!(stats.tx_bytes, 300);
    }

    #[test]
    fn test_record_rx() {
        let mut stats = NetStats::new();
        stats.record_rx(64);
        stats.record_rx(128);
        assert_eq!(stats.rx_packets, 2);
        assert_eq!(stats.rx_bytes, 192);
    }

    #[test]
    fn test_record_errors() {
        let mut stats = NetStats::new();
        stats.record_tx_error();
        stats.record_tx_error();
        stats.record_rx_error();
        stats.record_rx_drop();
        stats.record_rx_drop();
        stats.record_rx_drop();
        assert_eq!(stats.tx_errors, 2);
        assert_eq!(stats.rx_errors, 1);
        assert_eq!(stats.rx_dropped, 3);
    }

    #[test]
    fn test_stats_copy() {
        let mut stats = NetStats::new();
        stats.record_tx(500);
        let copied = stats;
        assert_eq!(copied.tx_packets, 1);
        assert_eq!(copied.tx_bytes, 500);
    }
}
