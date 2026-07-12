//! Layered watchdog feeding.
//!
//! Each layer has its own periodic deadline. `check()` inspects all enabled
//! layers: if any layer exceeds its `period_ms` it reports `LayerTimeout`;
//! if any layer additionally exceeds `hard_timeout_ms` the hardware watchdog
//! is stopped (triggering a hard reset) and `HardReset` is returned.

use crate::wdt::HwWatchdog;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerId(pub u32);

#[derive(Clone, Copy)]
pub struct FeedLayer {
    pub id: LayerId,
    pub name: &'static str,
    pub period_ms: u32,
    pub last_feed_ns: u64,
    pub enabled: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WatchdogStatus {
    AllFed,
    LayerTimeout(LayerId),
    HardReset,
}

pub struct Watchdog {
    pub hw: HwWatchdog,
    pub layers: [Option<FeedLayer>; 8],
    pub hard_timeout_ms: u32,
    pub next_id: u32,
}

impl Watchdog {
    /// Create a new layered watchdog wrapping the given hardware watchdog.
    pub const fn new(hw: HwWatchdog, hard_timeout_ms: u32) -> Self {
        Self {
            hw,
            layers: [None; 8],
            hard_timeout_ms,
            next_id: 1, // LayerId starts at 1
        }
    }

    /// Register a new feed layer. Returns `None` if all 8 slots are occupied.
    pub fn register_layer(&mut self, name: &'static str, period_ms: u32) -> Option<LayerId> {
        let slot = self.layers.iter_mut().find(|l| l.is_none())?;
        let id = LayerId(self.next_id);
        self.next_id += 1;
        *slot = Some(FeedLayer {
            id,
            name,
            period_ms,
            last_feed_ns: 0, // caller must feed_layer before timing starts
            enabled: true,
        });
        Some(id)
    }

    /// Record a feed event for the given layer at timestamp `now_ns`.
    pub fn feed_layer(&mut self, id: LayerId, now_ns: u64) {
        for layer in self.layers.iter_mut().flatten() {
            if layer.id == id {
                layer.last_feed_ns = now_ns;
                return;
            }
        }
    }

    /// Inspect all enabled layers and drive the hardware watchdog accordingly.
    pub fn check(&mut self, now_ns: u64) -> WatchdogStatus {
        let mut all_fed = true;
        let mut timeout_layer = None; // first layer that timed out but not hard-reset
        let mut worst_layer = None; // layer exceeding hard_timeout_ms

        let now_ms = now_ns / 1_000_000;

        for layer in self.layers.iter().flatten() {
            if !layer.enabled {
                continue;
            }
            let last_ms = layer.last_feed_ns / 1_000_000;
            let elapsed_ms = now_ms.saturating_sub(last_ms) as u32;

            if elapsed_ms > layer.period_ms {
                all_fed = false;
                if timeout_layer.is_none() {
                    timeout_layer = Some(layer.id);
                }
                if elapsed_ms > self.hard_timeout_ms {
                    worst_layer = Some(layer.id);
                }
            }
        }

        match worst_layer {
            Some(_) => {
                self.hw.stop();
                WatchdogStatus::HardReset
            }
            None if !all_fed => WatchdogStatus::LayerTimeout(timeout_layer.unwrap()),
            None => {
                self.hw.kick();
                WatchdogStatus::AllFed
            }
        }
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new(HwWatchdog::new(0), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_watchdog() {
        let wd = Watchdog::new(HwWatchdog::new(0), 1000);
        assert_eq!(wd.layers.iter().flatten().count(), 0);
        assert_eq!(wd.hard_timeout_ms, 1000);
        assert_eq!(wd.next_id, 1);
    }

    #[test]
    fn test_register_layer() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id = wd.register_layer("kernel", 100);
        assert_eq!(id, Some(LayerId(1)));
        let layer = wd.layers[0].as_ref().unwrap();
        assert_eq!(layer.name, "kernel");
        assert_eq!(layer.period_ms, 100);
        assert!(layer.enabled);
        assert_eq!(layer.last_feed_ns, 0);
        assert_eq!(wd.next_id, 2);
    }

    #[test]
    fn test_register_multiple_layers() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id1 = wd.register_layer("a", 100).unwrap();
        let id2 = wd.register_layer("b", 200).unwrap();
        let id3 = wd.register_layer("c", 300).unwrap();
        assert_eq!(id1, LayerId(1));
        assert_eq!(id2, LayerId(2));
        assert_eq!(id3, LayerId(3));
        assert_eq!(wd.layers.iter().flatten().count(), 3);
    }

    #[test]
    fn test_register_layer_full() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        for _ in 0..8 {
            assert!(wd.register_layer("layer", 100).is_some());
        }
        assert!(wd.register_layer("extra", 100).is_none());
    }

    #[test]
    fn test_feed_layer() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id = wd.register_layer("kernel", 100).unwrap();
        wd.feed_layer(id, 123_456_789);
        assert_eq!(wd.layers[0].as_ref().unwrap().last_feed_ns, 123_456_789);
    }

    #[test]
    fn test_check_all_fed() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id = wd.register_layer("kernel", 100).unwrap();
        wd.feed_layer(id, 50_000_000); // fed at t=50ms
                                       // t=100ms, elapsed=50ms < 100ms period
        let status = wd.check(100_000_000);
        assert_eq!(status, WatchdogStatus::AllFed);
    }

    #[test]
    fn test_check_layer_timeout() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id = wd.register_layer("kernel", 100).unwrap();
        wd.feed_layer(id, 0); // fed at t=0
                              // t=200ms > 100ms period, < 1000ms hard_timeout
        let status = wd.check(200_000_000);
        assert_eq!(status, WatchdogStatus::LayerTimeout(id));
    }

    #[test]
    fn test_check_hard_reset() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let id = wd.register_layer("kernel", 100).unwrap();
        wd.feed_layer(id, 0); // fed at t=0
                              // t=2000ms > 100ms period, > 1000ms hard_timeout
        let status = wd.check(2_000_000_000);
        assert_eq!(status, WatchdogStatus::HardReset);
    }

    #[test]
    fn test_check_disabled_layer() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let _id = wd.register_layer("kernel", 100).unwrap();
        wd.layers[0].as_mut().unwrap().enabled = false;
        // Even though elapsed > period, disabled layer is skipped
        let status = wd.check(1_000_000_000);
        assert_eq!(status, WatchdogStatus::AllFed);
    }

    #[test]
    fn test_check_empty_layers() {
        let mut wd = Watchdog::new(HwWatchdog::new(0), 1000);
        let status = wd.check(1_000_000_000);
        assert_eq!(status, WatchdogStatus::AllFed);
    }
}
