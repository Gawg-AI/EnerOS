//! Dual-network redundancy manager (v0.30.2).
//!
//! Wraps [`FailoverManager`] with link-state tracking for the primary
//! (Ethernet) and backup (Cellular) links. Provides a higher-level API
//! for updating link status, IP addresses, and delegating heartbeat
//! checks to the underlying failover state machine.

use crate::error::FailoverError;
use crate::failover::{FailoverEvent, FailoverManager, LinkType};
use crate::ppp::Ipv4Addr;

/// State of a single network link.
#[derive(Debug, Clone)]
pub struct LinkState {
    pub link_type: LinkType,
    pub is_up: bool,
    pub ipv4_addr: Option<Ipv4Addr>,
}

impl LinkState {
    /// Create a new `LinkState` with `is_up = false` and no IP address.
    pub fn new(link_type: LinkType) -> Self {
        Self {
            link_type,
            is_up: false,
            ipv4_addr: None,
        }
    }
}

/// Dual-network redundancy manager.
///
/// Manages primary (Ethernet) and backup (Cellular) links with automatic
/// failover via [`FailoverManager`].
pub struct RedundancyManager {
    primary_link: LinkState,
    backup_link: LinkState,
    active: LinkType,
    failover_mgr: FailoverManager,
}

impl Default for RedundancyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RedundancyManager {
    /// Create a new `RedundancyManager`.
    ///
    /// Initializes with:
    /// - Primary link (Ethernet) up
    /// - Backup link (Cellular) down
    /// - Active link = Ethernet
    /// - Failover manager with 10 000 ms recovery delay
    pub fn new() -> Self {
        let mut primary_link = LinkState::new(LinkType::Ethernet);
        primary_link.is_up = true;
        Self {
            primary_link,
            backup_link: LinkState::new(LinkType::Cellular),
            active: LinkType::Ethernet,
            failover_mgr: FailoverManager::new(10000),
        }
    }

    /// Update the primary link status.
    ///
    /// Triggers a `PrimaryDown` event when the primary goes down while
    /// Ethernet is active, and a `PrimaryUp` event when the primary
    /// comes back up while Cellular is active. Errors from the failover
    /// manager (e.g. invalid state transitions) are silently ignored;
    /// the cached active link is refreshed from the manager afterwards.
    pub fn set_primary_status(&mut self, up: bool, now: u64) {
        self.primary_link.is_up = up;
        let event = if !up && self.active == LinkType::Ethernet {
            Some(FailoverEvent::PrimaryDown)
        } else if up && self.active == LinkType::Cellular {
            Some(FailoverEvent::PrimaryUp)
        } else {
            None
        };
        if let Some(event) = event {
            let _ = self.failover_mgr.on_event(event, now);
            self.active = self.failover_mgr.current_active();
        }
    }

    /// Update the backup link status.
    ///
    /// This does not directly trigger failover events; backup link
    /// health is managed via heartbeats.
    pub fn set_backup_status(&mut self, up: bool, _now: u64) {
        self.backup_link.is_up = up;
    }

    /// Set the primary link's IPv4 address.
    pub fn set_primary_addr(&mut self, addr: Ipv4Addr) {
        self.primary_link.ipv4_addr = Some(addr);
    }

    /// Set the backup link's IPv4 address.
    pub fn set_backup_addr(&mut self, addr: Ipv4Addr) {
        self.backup_link.ipv4_addr = Some(addr);
    }

    /// Returns the currently active link type.
    pub fn current_active(&self) -> LinkType {
        self.active
    }

    /// Returns the total number of failovers.
    pub fn failover_count(&self) -> u32 {
        self.failover_mgr.failover_count()
    }

    /// Check heartbeats and return an event if action is needed.
    ///
    /// Delegates to [`FailoverManager::check_heartbeats`].
    pub fn check_heartbeats(&mut self, now: u64) -> Option<FailoverEvent> {
        self.failover_mgr.check_heartbeats(now)
    }

    /// Record a heartbeat on the primary link.
    ///
    /// Delegates to [`FailoverManager::on_primary_heartbeat`].
    pub fn on_primary_heartbeat(&mut self, now: u64) {
        self.failover_mgr.on_primary_heartbeat(now);
    }

    /// Record a heartbeat on the backup link.
    ///
    /// Delegates to [`FailoverManager::on_backup_heartbeat`].
    pub fn on_backup_heartbeat(&mut self, now: u64) {
        self.failover_mgr.on_backup_heartbeat(now);
    }

    /// Returns a reference to the primary link state.
    pub fn primary_link(&self) -> &LinkState {
        &self.primary_link
    }

    /// Returns a reference to the backup link state.
    pub fn backup_link(&self) -> &LinkState {
        &self.backup_link
    }

    /// Returns a reference to the underlying failover manager.
    pub fn failover_manager(&self) -> &FailoverManager {
        &self.failover_mgr
    }

    /// Returns a mutable reference to the underlying failover manager.
    pub fn failover_manager_mut(&mut self) -> &mut FailoverManager {
        &mut self.failover_mgr
    }

    /// Process a failover event.
    ///
    /// Delegates to [`FailoverManager::on_event`]. On success, updates
    /// the cached active link and returns it. On failure, returns the
    /// error and leaves the cached active link unchanged.
    pub fn process_event(
        &mut self,
        event: FailoverEvent,
        now: u64,
    ) -> Result<LinkType, FailoverError> {
        let result = self.failover_mgr.on_event(event, now);
        if let Ok(active) = result {
            self.active = active;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failover::FailoverState;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn link_state_new_creates_default_state() {
        let ls = LinkState::new(LinkType::Ethernet);
        assert_eq!(ls.link_type, LinkType::Ethernet);
        assert!(!ls.is_up);
        assert!(ls.ipv4_addr.is_none());
    }

    #[test]
    fn link_state_new_cellular() {
        let ls = LinkState::new(LinkType::Cellular);
        assert_eq!(ls.link_type, LinkType::Cellular);
        assert!(!ls.is_up);
        assert!(ls.ipv4_addr.is_none());
    }

    #[test]
    fn redundancy_manager_new_initial_state() {
        let rm = RedundancyManager::new();
        assert!(rm.primary_link().is_up);
        assert_eq!(rm.primary_link().link_type, LinkType::Ethernet);
        assert!(!rm.backup_link().is_up);
        assert_eq!(rm.backup_link().link_type, LinkType::Cellular);
        assert_eq!(rm.current_active(), LinkType::Ethernet);
        assert_eq!(rm.failover_count(), 0);
    }

    #[test]
    fn set_primary_status_true_no_event() {
        let mut rm = RedundancyManager::new();
        // Primary is already up and active=Ethernet; setting up=true is a no-op.
        rm.set_primary_status(true, 1000);
        assert!(rm.primary_link().is_up);
        assert_eq!(rm.current_active(), LinkType::Ethernet);
        assert_eq!(rm.failover_count(), 0);
    }

    #[test]
    fn set_primary_status_false_triggers_primary_down() {
        let mut rm = RedundancyManager::new();
        rm.set_primary_status(false, 1000);
        assert!(!rm.primary_link().is_up);
        assert_eq!(rm.current_active(), LinkType::Cellular);
        assert_eq!(rm.failover_count(), 1);
    }

    #[test]
    fn set_primary_status_true_triggers_primary_up_after_switch() {
        let mut rm = RedundancyManager::new();
        // Primary down -> Switching, active=Cellular.
        rm.set_primary_status(false, 1000);
        assert_eq!(rm.current_active(), LinkType::Cellular);
        // Complete the switch to reach BackupActive.
        rm.process_event(FailoverEvent::SwitchCompleted, 1100)
            .unwrap();
        assert_eq!(rm.failover_manager().state(), FailoverState::BackupActive);
        // Primary comes back -> PrimaryUp -> Recovering (anti-flap).
        rm.set_primary_status(true, 2000);
        assert!(rm.primary_link().is_up);
        // Still on Cellular during recovery.
        assert_eq!(rm.current_active(), LinkType::Cellular);
        assert_eq!(rm.failover_manager().state(), FailoverState::Recovering);
    }

    #[test]
    fn set_backup_status_updates_state() {
        let mut rm = RedundancyManager::new();
        assert!(!rm.backup_link().is_up);
        rm.set_backup_status(true, 1000);
        assert!(rm.backup_link().is_up);
        rm.set_backup_status(false, 2000);
        assert!(!rm.backup_link().is_up);
    }

    #[test]
    fn set_primary_addr_sets_address() {
        let mut rm = RedundancyManager::new();
        assert!(rm.primary_link().ipv4_addr.is_none());
        rm.set_primary_addr(ip(192, 168, 1, 1));
        assert_eq!(rm.primary_link().ipv4_addr, Some(ip(192, 168, 1, 1)));
    }

    #[test]
    fn set_backup_addr_sets_address() {
        let mut rm = RedundancyManager::new();
        assert!(rm.backup_link().ipv4_addr.is_none());
        rm.set_backup_addr(ip(10, 0, 0, 1));
        assert_eq!(rm.backup_link().ipv4_addr, Some(ip(10, 0, 0, 1)));
    }

    #[test]
    fn current_active_returns_active() {
        let mut rm = RedundancyManager::new();
        assert_eq!(rm.current_active(), LinkType::Ethernet);
        rm.set_primary_status(false, 1000);
        assert_eq!(rm.current_active(), LinkType::Cellular);
    }

    #[test]
    fn failover_count_returns_count() {
        let mut rm = RedundancyManager::new();
        assert_eq!(rm.failover_count(), 0);
        rm.set_primary_status(false, 1000);
        assert_eq!(rm.failover_count(), 1);
    }

    #[test]
    fn check_heartbeats_delegates_to_failover_manager() {
        let mut rm = RedundancyManager::new();
        rm.on_primary_heartbeat(1000);
        // No timeout yet.
        assert_eq!(rm.check_heartbeats(2000), None);
        // Accumulate misses to trigger PrimaryDown.
        assert_eq!(rm.check_heartbeats(4000), None); // miss 1
        assert_eq!(rm.check_heartbeats(7000), None); // miss 2
        assert_eq!(rm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown));
    }

    #[test]
    fn on_primary_heartbeat_delegates() {
        let mut rm = RedundancyManager::new();
        rm.on_primary_heartbeat(1000);
        // Should not trigger timeout immediately.
        assert_eq!(rm.check_heartbeats(1500), None);
    }

    #[test]
    fn on_backup_heartbeat_delegates() {
        let mut rm = RedundancyManager::new();
        // Drive a real failover through heartbeat timeouts so the primary
        // heartbeat monitor is genuinely dead (missed_count >= max_missed).
        rm.on_primary_heartbeat(1000);
        assert_eq!(rm.check_heartbeats(4000), None); // primary miss 1
        assert_eq!(rm.check_heartbeats(7000), None); // primary miss 2
        assert_eq!(rm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown)); // primary miss 3 -> failure
        rm.process_event(FailoverEvent::PrimaryDown, 10000).unwrap();
        rm.process_event(FailoverEvent::SwitchCompleted, 10100)
            .unwrap();
        // Now in BackupActive; primary heartbeat is dead.
        rm.on_backup_heartbeat(10200);
        // Backup alive shortly after; primary still dead -> no event.
        assert_eq!(rm.check_heartbeats(11200), None);
    }

    #[test]
    fn primary_link_returns_reference() {
        let rm = RedundancyManager::new();
        let link = rm.primary_link();
        assert_eq!(link.link_type, LinkType::Ethernet);
        assert!(link.is_up);
    }

    #[test]
    fn backup_link_returns_reference() {
        let rm = RedundancyManager::new();
        let link = rm.backup_link();
        assert_eq!(link.link_type, LinkType::Cellular);
        assert!(!link.is_up);
    }

    #[test]
    fn failover_manager_returns_reference() {
        let rm = RedundancyManager::new();
        let fm = rm.failover_manager();
        assert_eq!(fm.current_active(), LinkType::Ethernet);
        assert_eq!(fm.failover_count(), 0);
    }

    #[test]
    fn failover_manager_mut_returns_mutable_reference() {
        let mut rm = RedundancyManager::new();
        let fm = rm.failover_manager_mut();
        assert_eq!(fm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn process_event_success_updates_active() {
        let mut rm = RedundancyManager::new();
        // PrimaryDown -> active=Cellular.
        let result = rm.process_event(FailoverEvent::PrimaryDown, 1000);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(rm.current_active(), LinkType::Cellular);
        // SwitchCompleted -> still Cellular.
        let result = rm.process_event(FailoverEvent::SwitchCompleted, 1100);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(rm.current_active(), LinkType::Cellular);
        // PrimaryUp -> Recovering, still Cellular.
        let result = rm.process_event(FailoverEvent::PrimaryUp, 2000);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(rm.current_active(), LinkType::Cellular);
        // RecoveryCompleted -> back to Ethernet.
        let result = rm.process_event(FailoverEvent::RecoveryCompleted, 12000);
        assert_eq!(result, Ok(LinkType::Ethernet));
        assert_eq!(rm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn process_event_failure_returns_error() {
        let mut rm = RedundancyManager::new();
        // SwitchCompleted is invalid in PrimaryActive state.
        let result = rm.process_event(FailoverEvent::SwitchCompleted, 1000);
        assert_eq!(result, Err(FailoverError::InvalidState));
        // Active unchanged on failure.
        assert_eq!(rm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn full_failover_flow() {
        let mut rm = RedundancyManager::new();
        // Initial state.
        assert_eq!(rm.current_active(), LinkType::Ethernet);
        assert!(rm.primary_link().is_up);
        assert!(!rm.backup_link().is_up);

        // Primary goes down -> failover to Cellular.
        rm.set_primary_status(false, 1000);
        assert!(!rm.primary_link().is_up);
        assert_eq!(rm.current_active(), LinkType::Cellular);
        assert_eq!(rm.failover_count(), 1);
        assert_eq!(rm.failover_manager().state(), FailoverState::Switching);

        // Complete the switch.
        rm.process_event(FailoverEvent::SwitchCompleted, 1100)
            .unwrap();
        assert_eq!(rm.failover_manager().state(), FailoverState::BackupActive);

        // Primary comes back -> enter recovery (anti-flap).
        rm.set_primary_status(true, 2000);
        assert!(rm.primary_link().is_up);
        assert_eq!(rm.current_active(), LinkType::Cellular);
        assert_eq!(rm.failover_manager().state(), FailoverState::Recovering);

        // Recovery completes after the delay -> back to Ethernet.
        rm.process_event(FailoverEvent::RecoveryCompleted, 12000)
            .unwrap();
        assert_eq!(rm.current_active(), LinkType::Ethernet);
        assert_eq!(rm.failover_manager().state(), FailoverState::PrimaryActive);
    }
}
