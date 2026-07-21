//! DMA descriptor ring management for TX/RX.
//!
//! Provides [`DmaRing`] managing two ring buffers (TX and RX) of
//! [`DmaDescriptor`] entries. The ring uses the classic ownership-bit
//! protocol: `DESC_OWN=1` means the DMA owns the descriptor; `DESC_OWN=0`
//! means the CPU owns it.

use alloc::vec;
use alloc::vec::Vec;

/// Descriptor flag: owned by DMA (bit 31).
pub const DESC_OWN: u32 = 0x8000_0000;
/// Descriptor flag: interrupt on completion (bit 30).
pub const DESC_IOC: u32 = 0x4000_0000;
/// Descriptor flag: last segment of a frame (bit 29).
pub const DESC_LS: u32 = 0x2000_0000;
/// Descriptor flag: first segment of a frame (bit 28).
pub const DESC_FS: u32 = 0x1000_0000;

/// A single DMA descriptor.
///
/// Maps to a hardware DMA descriptor entry: buffer address, length, control
/// flags, and status. The `flags` field holds the ownership and segment bits;
/// `status` is written by the DMA engine on completion.
#[derive(Debug, Clone, Copy, Default)]
pub struct DmaDescriptor {
    /// Physical address of the data buffer.
    pub buffer_addr: u64,
    /// Length of valid data in the buffer (bytes).
    pub buffer_length: u32,
    /// Control flags (OWN, IOC, LS, FS, etc.).
    pub flags: u32,
    /// Status written by DMA on completion (error bits, frame length, etc.).
    pub status: u32,
}

impl DmaDescriptor {
    /// Create a new zeroed descriptor.
    pub const fn new() -> Self {
        Self {
            buffer_addr: 0,
            buffer_length: 0,
            flags: 0,
            status: 0,
        }
    }

    /// Returns `true` if the descriptor is currently owned by the DMA engine.
    pub fn is_owned_by_dma(&self) -> bool {
        self.flags & DESC_OWN != 0
    }

    /// Set or clear the DMA ownership bit.
    pub fn set_owned_by_dma(&mut self, owned: bool) {
        if owned {
            self.flags |= DESC_OWN;
        } else {
            self.flags &= !DESC_OWN;
        }
    }

    /// Returns `true` if this is the first segment of a frame.
    pub fn is_first(&self) -> bool {
        self.flags & DESC_FS != 0
    }

    /// Returns `true` if this is the last segment of a frame.
    pub fn is_last(&self) -> bool {
        self.flags & DESC_LS != 0
    }

    /// Set the first-segment flag.
    pub fn set_first(&mut self, first: bool) {
        if first {
            self.flags |= DESC_FS;
        } else {
            self.flags &= !DESC_FS;
        }
    }

    /// Set the last-segment flag.
    pub fn set_last(&mut self, last: bool) {
        if last {
            self.flags |= DESC_LS;
        } else {
            self.flags &= !DESC_LS;
        }
    }

    /// Set the interrupt-on-completion flag.
    pub fn set_ioc(&mut self, ioc: bool) {
        if ioc {
            self.flags |= DESC_IOC;
        } else {
            self.flags &= !DESC_IOC;
        }
    }
}

/// Ring buffer managing TX and RX descriptor arrays.
///
/// TX ring semantics:
/// - `tx_head`: producer index — CPU writes frames here and advances head.
/// - `tx_tail`: consumer index — DMA completes and CPU advances tail on IRQ.
/// - Ring is full when `(head + 1) % count == tail`.
/// - Ring is empty when `head == tail`.
///
/// RX ring semantics:
/// - `rx_head`: producer index — CPU gives buffers back to DMA here.
/// - `rx_tail`: consumer index — CPU checks for received frames here.
/// - A descriptor at `rx_tail` with `OWN=0` has a received frame.
pub struct DmaRing {
    /// TX descriptor array.
    pub tx_desc: Vec<DmaDescriptor>,
    /// RX descriptor array.
    pub rx_desc: Vec<DmaDescriptor>,
    /// TX producer index (CPU writes here).
    pub tx_head: u32,
    /// TX consumer index (DMA completes here).
    pub tx_tail: u32,
    /// RX producer index (CPU gives buffers back here).
    pub rx_head: u32,
    /// RX consumer index (CPU reads received frames here).
    pub rx_tail: u32,
}

impl DmaRing {
    /// Create a new DMA ring with `tx_count` TX descriptors and `rx_count`
    /// RX descriptors. All descriptors start CPU-owned (OWN=0).
    pub fn new(tx_count: usize, rx_count: usize) -> Self {
        let tx_desc = vec![DmaDescriptor::new(); tx_count];
        let rx_desc = vec![DmaDescriptor::new(); rx_count];
        Self {
            tx_desc,
            rx_desc,
            tx_head: 0,
            tx_tail: 0,
            rx_head: 0,
            rx_tail: 0,
        }
    }

    // -----------------------------------------------------------------
    // TX ring operations
    // -----------------------------------------------------------------

    /// Number of TX descriptors in the ring.
    pub fn tx_count(&self) -> usize {
        self.tx_desc.len()
    }

    /// Returns `true` if the TX ring is full (no free descriptors).
    pub fn tx_is_full(&self) -> bool {
        let count = self.tx_count();
        if count == 0 {
            return true;
        }
        (self.tx_head + 1) as usize % count == self.tx_tail as usize
    }

    /// Returns `true` if the TX ring is empty (no pending frames).
    pub fn tx_is_empty(&self) -> bool {
        self.tx_head == self.tx_tail
    }

    /// Number of TX descriptors currently in use (pending DMA completion).
    pub fn tx_pending(&self) -> usize {
        let count = self.tx_count();
        if count == 0 {
            return 0;
        }
        let head = self.tx_head as usize;
        let tail = self.tx_tail as usize;
        (head + count - tail) % count
    }

    /// Enqueue a TX descriptor. Returns the descriptor index if space is
    /// available, or `None` if the ring is full. Advances `tx_head`.
    pub fn tx_enqueue(&mut self) -> Option<usize> {
        if self.tx_is_full() {
            return None;
        }
        let idx = self.tx_head as usize;
        self.tx_head = (self.tx_head + 1) % self.tx_count() as u32;
        Some(idx)
    }

    /// Advance the TX tail (called after DMA completion to free descriptors).
    pub fn tx_advance_tail(&mut self) {
        if !self.tx_is_empty() {
            self.tx_tail = (self.tx_tail + 1) % self.tx_count() as u32;
        }
    }

    // -----------------------------------------------------------------
    // RX ring operations
    // -----------------------------------------------------------------

    /// Number of RX descriptors in the ring.
    pub fn rx_count(&self) -> usize {
        self.rx_desc.len()
    }

    /// Returns `true` if the RX ring is full (all descriptors are CPU-owned
    /// with received frames, none given back to DMA).
    pub fn rx_is_full(&self) -> bool {
        let count = self.rx_count();
        if count == 0 {
            return true;
        }
        (self.rx_head + 1) as usize % count == self.rx_tail as usize
    }

    /// Returns `true` if the RX ring is empty (no received frames pending).
    pub fn rx_is_empty(&self) -> bool {
        self.rx_head == self.rx_tail
    }

    /// Number of RX descriptors available for dequeue (with received frames).
    ///
    /// This counts descriptors from `rx_tail` to `rx_head` that are not
    /// owned by DMA (i.e., have a received frame).
    pub fn rx_available(&self) -> usize {
        let count = self.rx_count();
        if count == 0 {
            return 0;
        }
        let head = self.rx_head as usize;
        let tail = self.rx_tail as usize;
        let pending = (head + count - tail) % count;
        // Count how many of the pending descriptors are not owned by DMA
        let mut avail = 0;
        for i in 0..pending {
            let idx = (tail + i) % count;
            if !self.rx_desc[idx].is_owned_by_dma() {
                avail += 1;
            }
        }
        avail
    }

    /// Dequeue an RX descriptor with a received frame. Returns the
    /// descriptor index if a frame is available, or `None` if no frames
    /// are pending. Advances `rx_tail`.
    pub fn rx_dequeue(&mut self) -> Option<usize> {
        let count = self.rx_count();
        if count == 0 {
            return None;
        }
        // Check if there are any descriptors between rx_tail and rx_head
        // that are not owned by DMA (i.e., have a received frame).
        let head = self.rx_head as usize;
        let tail = self.rx_tail as usize;
        if tail == head {
            // Ring is empty in the index sense — but there might be a
            // descriptor at rx_tail that was released by DMA after the
            // initial setup (where rx_head == rx_tail after init).
            // Check the descriptor at rx_tail.
            if !self.rx_desc[tail].is_owned_by_dma() {
                let idx = tail;
                self.rx_tail = (self.rx_tail + 1) % count as u32;
                return Some(idx);
            }
            return None;
        }
        // Scan from rx_tail for the first descriptor not owned by DMA.
        let mut cur = tail;
        for _ in 0..count {
            if cur == head {
                break;
            }
            if !self.rx_desc[cur].is_owned_by_dma() {
                self.rx_tail = (cur as u32 + 1) % count as u32;
                return Some(cur);
            }
            cur = (cur + 1) % count;
        }
        None
    }

    /// Recycle an RX descriptor (give it back to DMA for future reception).
    /// Sets OWN=1 and IOC=1 on the descriptor at `idx`.
    pub fn rx_recycle(&mut self, idx: usize) {
        if idx < self.rx_count() {
            self.rx_desc[idx].set_owned_by_dma(true);
            self.rx_desc[idx].set_ioc(true);
            // Advance rx_head to track the producer position.
            let count = self.rx_count();
            // Only advance if idx matches the expected head position.
            // In practice, the caller recycles descriptors in order.
            self.rx_head = (self.rx_head + 1) % count as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desc_flags_constants() {
        assert_eq!(DESC_OWN, 0x8000_0000);
        assert_eq!(DESC_IOC, 0x4000_0000);
        assert_eq!(DESC_LS, 0x2000_0000);
        assert_eq!(DESC_FS, 0x1000_0000);
    }

    #[test]
    fn test_descriptor_new() {
        let desc = DmaDescriptor::new();
        assert_eq!(desc.buffer_addr, 0);
        assert_eq!(desc.buffer_length, 0);
        assert_eq!(desc.flags, 0);
        assert_eq!(desc.status, 0);
        assert!(!desc.is_owned_by_dma());
    }

    #[test]
    fn test_descriptor_set_owned() {
        let mut desc = DmaDescriptor::new();
        desc.set_owned_by_dma(true);
        assert!(desc.is_owned_by_dma());
        assert_eq!(desc.flags & DESC_OWN, DESC_OWN);
        desc.set_owned_by_dma(false);
        assert!(!desc.is_owned_by_dma());
        assert_eq!(desc.flags & DESC_OWN, 0);
    }

    #[test]
    fn test_descriptor_first_last() {
        let mut desc = DmaDescriptor::new();
        desc.set_first(true);
        assert!(desc.is_first());
        desc.set_last(true);
        assert!(desc.is_last());
        desc.set_first(false);
        assert!(!desc.is_first());
        assert!(desc.is_last());
    }

    #[test]
    fn test_descriptor_ioc() {
        let mut desc = DmaDescriptor::new();
        desc.set_ioc(true);
        assert_eq!(desc.flags & DESC_IOC, DESC_IOC);
        desc.set_ioc(false);
        assert_eq!(desc.flags & DESC_IOC, 0);
    }

    #[test]
    fn test_dmaring_new() {
        let ring = DmaRing::new(16, 32);
        assert_eq!(ring.tx_count(), 16);
        assert_eq!(ring.rx_count(), 32);
        assert_eq!(ring.tx_head, 0);
        assert_eq!(ring.tx_tail, 0);
        assert_eq!(ring.rx_head, 0);
        assert_eq!(ring.rx_tail, 0);
    }

    #[test]
    fn test_dmaring_new_zero() {
        let ring = DmaRing::new(0, 0);
        assert_eq!(ring.tx_count(), 0);
        assert_eq!(ring.rx_count(), 0);
        assert!(ring.tx_is_full());
        assert!(ring.rx_is_full());
    }

    #[test]
    fn test_tx_enqueue_basic() {
        let mut ring = DmaRing::new(4, 4);
        let idx = ring.tx_enqueue();
        assert_eq!(idx, Some(0));
        assert_eq!(ring.tx_head, 1);
        assert_eq!(ring.tx_tail, 0);
        assert_eq!(ring.tx_pending(), 1);
    }

    #[test]
    fn test_tx_enqueue_multiple() {
        let mut ring = DmaRing::new(4, 4);
        assert_eq!(ring.tx_enqueue(), Some(0));
        assert_eq!(ring.tx_enqueue(), Some(1));
        assert_eq!(ring.tx_enqueue(), Some(2));
        assert_eq!(ring.tx_pending(), 3);
        // Ring size 4 can hold 3 entries (one slot wasted for full detection)
        assert!(ring.tx_is_full());
        assert_eq!(ring.tx_enqueue(), None);
    }

    #[test]
    fn test_tx_is_empty_initial() {
        let ring = DmaRing::new(4, 4);
        assert!(ring.tx_is_empty());
    }

    #[test]
    fn test_tx_advance_tail() {
        let mut ring = DmaRing::new(4, 4);
        ring.tx_enqueue();
        ring.tx_enqueue();
        assert_eq!(ring.tx_pending(), 2);
        ring.tx_advance_tail();
        assert_eq!(ring.tx_pending(), 1);
        ring.tx_advance_tail();
        assert_eq!(ring.tx_pending(), 0);
        assert!(ring.tx_is_empty());
    }

    #[test]
    fn test_tx_advance_tail_empty() {
        let mut ring = DmaRing::new(4, 4);
        // Advancing tail on empty ring should be a no-op.
        ring.tx_advance_tail();
        assert_eq!(ring.tx_tail, 0);
    }

    #[test]
    fn test_tx_wraparound() {
        let mut ring = DmaRing::new(4, 4);
        // Fill 3, advance 2, fill 2 more → tests wraparound
        ring.tx_enqueue(); // head=1
        ring.tx_enqueue(); // head=2
        ring.tx_enqueue(); // head=3
        ring.tx_advance_tail(); // tail=1
        ring.tx_advance_tail(); // tail=2
        assert_eq!(ring.tx_pending(), 1);
        assert!(!ring.tx_is_full());
        ring.tx_enqueue(); // head=0 (wrapped)
        ring.tx_enqueue(); // head=1
        assert_eq!(ring.tx_pending(), 3);
        assert!(ring.tx_is_full());
    }

    #[test]
    fn test_tx_full_wraparound() {
        let mut ring = DmaRing::new(3, 3);
        // Size 3 can hold 2 entries
        assert_eq!(ring.tx_enqueue(), Some(0));
        assert_eq!(ring.tx_enqueue(), Some(1));
        assert!(ring.tx_is_full());
        assert_eq!(ring.tx_enqueue(), None);
        ring.tx_advance_tail(); // tail=1
        assert_eq!(ring.tx_enqueue(), Some(2));
        assert!(ring.tx_is_full());
        ring.tx_advance_tail(); // tail=2
        assert_eq!(ring.tx_enqueue(), Some(0)); // wrapped
    }

    #[test]
    fn test_rx_dequeue_empty() {
        let mut ring = DmaRing::new(4, 4);
        // All RX descriptors start with OWN=0 (CPU-owned), but rx_head == rx_tail
        // means no frames have been given to DMA and received back.
        // Actually, with the init logic, descriptors start CPU-owned.
        // Let's set them all to DMA-owned first (simulating init).
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        assert_eq!(ring.rx_dequeue(), None);
    }

    #[test]
    fn test_rx_dequeue_one_frame() {
        let mut ring = DmaRing::new(4, 4);
        // Simulate init: all RX descriptors owned by DMA
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        // Simulate DMA receiving a frame on descriptor 0
        ring.rx_desc[0].set_owned_by_dma(false);
        ring.rx_desc[0].buffer_length = 64;
        // rx_head should advance to 1 (one descriptor given to DMA)
        ring.rx_head = 1;
        let idx = ring.rx_dequeue();
        assert_eq!(idx, Some(0));
        assert_eq!(ring.rx_tail, 1);
    }

    #[test]
    fn test_rx_recycle() {
        let mut ring = DmaRing::new(4, 4);
        ring.rx_desc[0].set_owned_by_dma(false);
        ring.rx_head = 1;
        ring.rx_tail = 1;
        ring.rx_recycle(0);
        assert!(ring.rx_desc[0].is_owned_by_dma());
        assert_eq!(ring.rx_desc[0].flags & DESC_IOC, DESC_IOC);
    }

    #[test]
    fn test_rx_available_empty() {
        let mut ring = DmaRing::new(4, 4);
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        ring.rx_head = 0;
        ring.rx_tail = 0;
        assert_eq!(ring.rx_available(), 0);
    }

    #[test]
    fn test_rx_available_one() {
        let mut ring = DmaRing::new(4, 4);
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        ring.rx_desc[0].set_owned_by_dma(false);
        ring.rx_head = 1;
        ring.rx_tail = 0;
        assert_eq!(ring.rx_available(), 1);
    }

    #[test]
    fn test_rx_dequeue_and_recycle_cycle() {
        let mut ring = DmaRing::new(4, 4);
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        // DMA receives frame on desc 0
        ring.rx_desc[0].set_owned_by_dma(false);
        ring.rx_desc[0].buffer_length = 100;
        ring.rx_head = 1;

        // CPU dequeues
        let idx = ring.rx_dequeue().expect("should have a frame");
        assert_eq!(idx, 0);
        assert_eq!(ring.rx_tail, 1);

        // CPU recycles
        ring.rx_recycle(0);
        assert!(ring.rx_desc[0].is_owned_by_dma());

        // No more frames available
        assert_eq!(ring.rx_dequeue(), None);
    }

    #[test]
    fn test_rx_wraparound() {
        let mut ring = DmaRing::new(4, 4);
        for desc in &mut ring.rx_desc {
            desc.set_owned_by_dma(true);
        }
        // Set up: rx_tail=3, rx_head=1 (wrapped), desc 3 and 0 have frames
        ring.rx_tail = 3;
        ring.rx_head = 1;
        ring.rx_desc[3].set_owned_by_dma(false);
        ring.rx_desc[0].set_owned_by_dma(false);

        let idx1 = ring.rx_dequeue();
        assert_eq!(idx1, Some(3));
        let idx2 = ring.rx_dequeue();
        assert_eq!(idx2, Some(0));
        assert_eq!(ring.rx_dequeue(), None);
    }

    #[test]
    fn test_rx_is_full() {
        let mut ring = DmaRing::new(4, 4);
        // Fill all but one slot: head=2, tail=0 → (2+1)%4=3 != 0 → not full
        ring.rx_head = 2;
        ring.rx_tail = 0;
        assert!(!ring.rx_is_full());
        // Full: head=3, tail=0 → (3+1)%4=0 == 0 → full
        ring.rx_head = 3;
        ring.rx_tail = 0;
        assert!(ring.rx_is_full());
    }

    #[test]
    fn test_rx_is_empty() {
        let ring = DmaRing::new(4, 4);
        assert!(ring.rx_is_empty()); // head == tail
    }

    #[test]
    fn test_tx_count_and_rx_count() {
        let ring = DmaRing::new(16, 32);
        assert_eq!(ring.tx_count(), 16);
        assert_eq!(ring.rx_count(), 32);
    }

    #[test]
    fn test_tx_pending_after_operations() {
        let mut ring = DmaRing::new(8, 8);
        assert_eq!(ring.tx_pending(), 0);
        ring.tx_enqueue();
        ring.tx_enqueue();
        ring.tx_enqueue();
        assert_eq!(ring.tx_pending(), 3);
        ring.tx_advance_tail();
        assert_eq!(ring.tx_pending(), 2);
        ring.tx_advance_tail();
        ring.tx_advance_tail();
        assert_eq!(ring.tx_pending(), 0);
    }

    #[test]
    fn test_descriptor_default() {
        let desc = DmaDescriptor::default();
        assert!(!desc.is_owned_by_dma());
        assert!(!desc.is_first());
        assert!(!desc.is_last());
    }
}
