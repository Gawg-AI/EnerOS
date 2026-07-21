//! SmolcpDevice — adapter bridging [`crate::NetDevice`] to [`smoltcp::phy::Device`].
//!
//! smoltcp's `Device` trait uses a zero-copy token-based model (RxToken/TxToken),
//! while EnerOS's [`crate::NetDevice`] uses a copy-based model (send/recv byte
//! slices). This adapter bridges the two by maintaining an internal RX queue
//! of owned frame buffers.
//!
//! # Data Flow
//!
//! ```text
//! NetDevice::recv() ──► recv_frame() ──► rx_queue ──► receive() ──► RxToken::consume()
//!                                                                        │
//! TxToken::consume() ──► NetDevice::send() ◄──────────────────────────┘
//! ```

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::mac::NetDevice;

/// Adapter that wraps a [`NetDevice`] and implements smoltcp's [`phy::Device`]
/// trait.
///
/// Maintains an internal `rx_queue` of received frames. The caller must call
/// [`recv_frame`] before [`poll`](super::interface::NetworkInterface::poll) to
/// drain frames from the hardware into the queue.
pub struct SmolcpDevice<D: NetDevice> {
    /// The underlying network device (MAC controller or mock).
    pub device: D,
    /// Cached MTU (maximum transmission unit in bytes).
    mtu: usize,
    /// Queue of received frames waiting to be consumed by smoltcp.
    rx_queue: VecDeque<Vec<u8>>,
}

/// RX token — holds an owned frame buffer.
pub struct SmolcpRxToken {
    frame: Vec<u8>,
}

/// TX token — holds a mutable reference to the underlying device for sending.
pub struct SmolcpTxToken<'a, D: NetDevice> {
    device: &'a mut D,
}

impl<D: NetDevice> SmolcpDevice<D> {
    /// Create a new SmolcpDevice wrapping the given NetDevice.
    ///
    /// The MTU is queried from the device at creation time.
    pub fn new(device: D) -> Self {
        let mtu = device.mtu();
        Self {
            device,
            mtu,
            rx_queue: VecDeque::new(),
        }
    }

    /// Read one frame from the underlying NetDevice into the RX queue.
    ///
    /// Should be called before each `poll()` to drain hardware RX buffers.
    /// Returns `true` if a frame was successfully read, `false` otherwise.
    pub fn recv_frame(&mut self) -> bool {
        let mut buf: Vec<u8> = alloc::vec![0u8; self.mtu];
        match self.device.recv(&mut buf) {
            Ok(len) => {
                buf.truncate(len);
                self.rx_queue.push_back(buf);
                true
            }
            Err(_) => false,
        }
    }

    /// Drain all available frames from the hardware into the RX queue.
    ///
    /// Continues calling `NetDevice::recv()` until it returns an error
    /// (NoBuffer / NotInitialized / etc.).
    pub fn drain_rx(&mut self) -> usize {
        let mut count = 0;
        while self.recv_frame() {
            count += 1;
        }
        count
    }

    /// Returns the number of frames waiting in the RX queue.
    pub fn rx_queue_len(&self) -> usize {
        self.rx_queue.len()
    }

    /// Returns the MTU of this device.
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    /// Get a reference to the underlying device.
    pub fn device(&self) -> &D {
        &self.device
    }

    /// Get a mutable reference to the underlying device.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }
}

// ---------------------------------------------------------------------------
// smoltcp::phy::Device implementation
// ---------------------------------------------------------------------------

impl<D: NetDevice> phy::Device for SmolcpDevice<D> {
    type RxToken<'a>
        = SmolcpRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = SmolcpTxToken<'a, D>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.rx_queue.pop_front() {
            Some(frame) => Some((
                SmolcpRxToken { frame },
                SmolcpTxToken {
                    device: &mut self.device,
                },
            )),
            None => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(SmolcpTxToken {
            device: &mut self.device,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = self.mtu;
        caps.medium = Medium::Ethernet;
        // No checksum offload — smoltcp computes checksums in software.
        caps
    }
}

// ---------------------------------------------------------------------------
// RxToken implementation
// ---------------------------------------------------------------------------

impl phy::RxToken for SmolcpRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.frame)
    }
}

// ---------------------------------------------------------------------------
// TxToken implementation
// ---------------------------------------------------------------------------

impl<'a, D: NetDevice> phy::TxToken for SmolcpTxToken<'a, D> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf: Vec<u8> = alloc::vec![0u8; len];
        let result = f(&mut buf);
        // Send the frame via the underlying NetDevice. Errors are silently
        // ignored here because smoltcp's TxToken::consume does not return
        // a Result. The caller should check link state before polling.
        let _ = self.device.send(&buf[..len]);
        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use alloc::collections::VecDeque;

    use smoltcp::phy::{Device, RxToken, TxToken};

    use super::*;
    use crate::NetError;

    /// Mock NetDevice for testing SmolcpDevice.
    /// Stores frames to be "received" and frames that were "sent".
    struct MockNetDevice {
        mac_addr: [u8; 6],
        mtu: usize,
        link_up: bool,
        rx_frames: VecDeque<Vec<u8>>,
        tx_frames: Vec<Vec<u8>>,
        promiscuous: bool,
    }

    impl MockNetDevice {
        fn new(mac_addr: [u8; 6], mtu: usize) -> Self {
            Self {
                mac_addr,
                mtu,
                link_up: true,
                rx_frames: VecDeque::new(),
                tx_frames: Vec::new(),
                promiscuous: false,
            }
        }

        fn push_rx_frame(&mut self, frame: &[u8]) {
            self.rx_frames.push_back(frame.to_vec());
        }

        fn tx_frames(&self) -> &[Vec<u8>] {
            &self.tx_frames
        }
    }

    impl NetDevice for MockNetDevice {
        fn send(&mut self, frame: &[u8]) -> Result<(), NetError> {
            if !self.link_up {
                return Err(NetError::LinkDown);
            }
            self.tx_frames.push(frame.to_vec());
            Ok(())
        }

        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
            match self.rx_frames.pop_front() {
                Some(frame) => {
                    let len = frame.len().min(buf.len());
                    buf[..len].copy_from_slice(&frame[..len]);
                    Ok(len)
                }
                None => Err(NetError::NoBuffer),
            }
        }

        fn mac_address(&self) -> [u8; 6] {
            self.mac_addr
        }

        fn mtu(&self) -> usize {
            self.mtu
        }

        fn link_up(&self) -> bool {
            self.link_up
        }

        fn set_promiscuous(&mut self, on: bool) {
            self.promiscuous = on;
        }

        fn stats(&self) -> crate::error::NetStats {
            crate::error::NetStats::new()
        }
    }

    fn sample_frame(len: usize) -> Vec<u8> {
        let mut frame = vec![0u8; len];
        frame[0..6].copy_from_slice(&[0xFF; 6]);
        frame[6..12].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x01]);
        frame[12] = 0x08;
        frame[13] = 0x00;
        frame
    }

    #[test]
    fn test_smolcp_device_new() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let dev = SmolcpDevice::new(mock);
        assert_eq!(dev.mtu(), 1500);
        assert_eq!(dev.rx_queue_len(), 0);
    }

    #[test]
    fn test_recv_frame_success() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let frame = sample_frame(64);
        mock.push_rx_frame(&frame);

        let mut dev = SmolcpDevice::new(mock);
        assert!(dev.recv_frame());
        assert_eq!(dev.rx_queue_len(), 1);
    }

    #[test]
    fn test_recv_frame_empty() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let mut dev = SmolcpDevice::new(mock);
        assert!(!dev.recv_frame());
        assert_eq!(dev.rx_queue_len(), 0);
    }

    #[test]
    fn test_drain_rx() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        mock.push_rx_frame(&sample_frame(64));
        mock.push_rx_frame(&sample_frame(128));
        mock.push_rx_frame(&sample_frame(256));

        let mut dev = SmolcpDevice::new(mock);
        let count = dev.drain_rx();
        assert_eq!(count, 3);
        assert_eq!(dev.rx_queue_len(), 3);
    }

    #[test]
    fn test_drain_rx_empty() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let mut dev = SmolcpDevice::new(mock);
        let count = dev.drain_rx();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_receive_returns_frame() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let frame = sample_frame(64);
        mock.push_rx_frame(&frame);

        let mut dev = SmolcpDevice::new(mock);
        dev.recv_frame();

        let result = dev.receive(Instant::from_millis(0));
        assert!(result.is_some());
        let (rx_token, _tx_token) = result.unwrap();
        // The RxToken is consumed by calling consume()
        let _data: Vec<u8> = rx_token.consume(|data| data.to_vec());
    }

    #[test]
    fn test_receive_empty_returns_none() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let mut dev = SmolcpDevice::new(mock);
        let result = dev.receive(Instant::from_millis(0));
        assert!(result.is_none());
    }

    #[test]
    fn test_transmit_returns_token() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let mut dev = SmolcpDevice::new(mock);
        let token = dev.transmit(Instant::from_millis(0));
        assert!(token.is_some());
    }

    #[test]
    fn test_rx_token_consume() {
        let frame = sample_frame(128);
        let rx_token = SmolcpRxToken {
            frame: frame.clone(),
        };
        let result: Vec<u8> = rx_token.consume(|data| data.to_vec());
        assert_eq!(result, frame);
    }

    #[test]
    fn test_tx_token_consume_sends_frame() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let tx_token = SmolcpTxToken { device: &mut mock };
        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        tx_token.consume(4, |buf| {
            buf[..4].copy_from_slice(&payload);
        });
        assert_eq!(mock.tx_frames().len(), 1);
        assert_eq!(&mock.tx_frames()[0][..4], &payload);
    }

    #[test]
    fn test_tx_token_consume_returns_value() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let tx_token = SmolcpTxToken { device: &mut mock };
        let result: u32 = tx_token.consume(8, |buf| {
            buf[0] = 42;
            12345
        });
        assert_eq!(result, 12345);
        assert_eq!(mock.tx_frames().len(), 1);
        assert_eq!(mock.tx_frames()[0][0], 42);
    }

    #[test]
    fn test_capabilities() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let dev = SmolcpDevice::new(mock);
        let caps = dev.capabilities();
        assert_eq!(caps.max_transmission_unit, 1500);
        assert_eq!(caps.medium, Medium::Ethernet);
    }

    #[test]
    fn test_capabilities_custom_mtu() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 9000);
        let dev = SmolcpDevice::new(mock);
        let caps = dev.capabilities();
        assert_eq!(caps.max_transmission_unit, 9000);
    }

    #[test]
    fn test_device_accessors() {
        let mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        let mut dev = SmolcpDevice::new(mock);
        assert_eq!(dev.device().mac_address(), [0x02, 0, 0, 0, 0, 0x01]);
        dev.device_mut().set_promiscuous(true);
    }

    #[test]
    fn test_receive_and_transmit_together() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        mock.push_rx_frame(&sample_frame(64));

        let mut dev = SmolcpDevice::new(mock);
        dev.recv_frame();

        // receive() should give us both an RxToken and a TxToken
        let result = dev.receive(Instant::from_millis(0));
        assert!(result.is_some());
        let (rx_token, tx_token) = result.unwrap();

        // Consume the RxToken (read the frame)
        let frame_data: Vec<u8> = rx_token.consume(|data| data.to_vec());
        assert_eq!(frame_data.len(), 64);

        // Consume the TxToken (send a frame)
        tx_token.consume(32, |buf| {
            buf[..4].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        });

        // Verify the frame was sent
        assert_eq!(dev.device().tx_frames().len(), 1);
    }

    #[test]
    fn test_multiple_recv_frames() {
        let mut mock = MockNetDevice::new([0x02, 0, 0, 0, 0, 0x01], 1500);
        mock.push_rx_frame(&sample_frame(64));
        mock.push_rx_frame(&sample_frame(128));
        mock.push_rx_frame(&sample_frame(256));

        let mut dev = SmolcpDevice::new(mock);
        dev.drain_rx();
        assert_eq!(dev.rx_queue_len(), 3);

        // Pop first frame
        let r1 = dev.receive(Instant::from_millis(0));
        assert!(r1.is_some());

        // Pop second frame
        let r2 = dev.receive(Instant::from_millis(0));
        assert!(r2.is_some());

        // Pop third frame
        let r3 = dev.receive(Instant::from_millis(0));
        assert!(r3.is_some());

        // Queue should be empty now
        let r4 = dev.receive(Instant::from_millis(0));
        assert!(r4.is_none());
    }
}
