//! Failover management for dual-network redundancy (v0.30.2).
//!
//! Implements a state machine that tracks primary (Ethernet) and backup
//! (Cellular) links, switching between them on failure and recovering
//! back to primary when it comes back online. Anti-flap protection is
//! provided by a configurable recovery delay enforced by the caller
//! before issuing `RecoveryCompleted`.

use crate::error::FailoverError;
use crate::heartbeat::HeartbeatMonitor;

/// Link type for primary/backup identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    /// Primary link (wired Ethernet).
    Ethernet,
    /// Backup link (cellular modem).
    Cellular,
}

/// Failover state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverState {
    /// Primary link is active and healthy.
    PrimaryActive,
    /// Backup link is active (primary is down).
    BackupActive,
    /// A switch between links is in progress.
    Switching,
    /// Primary has recovered; waiting for recovery delay before switching back.
    Recovering,
}

/// Events that trigger failover state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverEvent {
    /// Primary link has gone down.
    PrimaryDown,
    /// Primary link has come back up.
    PrimaryUp,
    /// A link switch has completed.
    SwitchCompleted,
    /// Recovery to primary has completed.
    RecoveryCompleted,
}

/// Failover manager with heartbeat monitoring and anti-flap recovery.
///
/// Tracks primary (Ethernet) and backup (Cellular) links, switching to
/// backup when primary fails and recovering back to primary when it
/// recovers. The recovery delay prevents flapping between links.
pub struct FailoverManager {
    state: FailoverState,
    active: LinkType,
    heartbeat_primary: HeartbeatMonitor,
    heartbeat_backup: HeartbeatMonitor,
    failover_count: u32,
    recovery_delay_ms: u64,
    last_failover_time: u64,
    callback: Option<fn(FailoverEvent)>,
}

impl FailoverManager {
    /// Create a new `FailoverManager` with the given recovery delay.
    ///
    /// Initializes in `PrimaryActive` state with Ethernet as the active
    /// link. Both primary and backup heartbeat monitors are configured
    /// with a 1000 ms interval, 3000 ms timeout, and 3 max misses.
    pub fn new(recovery_delay_ms: u64) -> Self {
        Self {
            state: FailoverState::PrimaryActive,
            active: LinkType::Ethernet,
            heartbeat_primary: HeartbeatMonitor::new(1000, 3000, 3),
            heartbeat_backup: HeartbeatMonitor::new(1000, 3000, 3),
            failover_count: 0,
            recovery_delay_ms,
            last_failover_time: 0,
            callback: None,
        }
    }

    /// Process a failover event and transition the state machine.
    ///
    /// Valid transitions:
    /// - `PrimaryActive` + `PrimaryDown` → `Switching` (active=Cellular,
    ///   failover_count++, last_failover_time=now)
    /// - `Switching` + `SwitchCompleted` → `BackupActive`
    /// - `BackupActive` + `PrimaryUp` → `Recovering`
    /// - `Recovering` + `RecoveryCompleted` → `PrimaryActive`
    ///   (active=Ethernet)
    ///
    /// Returns the active link after the transition, or
    /// `Err(InvalidState)` for invalid transitions. The registered
    /// callback (if any) is invoked with the event on a successful
    /// transition.
    pub fn on_event(&mut self, event: FailoverEvent, now: u64) -> Result<LinkType, FailoverError> {
        let new_state = match (self.state, event) {
            (FailoverState::PrimaryActive, FailoverEvent::PrimaryDown) => {
                self.active = LinkType::Cellular;
                self.failover_count = self.failover_count.saturating_add(1);
                self.last_failover_time = now;
                FailoverState::Switching
            }
            (FailoverState::Switching, FailoverEvent::SwitchCompleted) => {
                FailoverState::BackupActive
            }
            (FailoverState::BackupActive, FailoverEvent::PrimaryUp) => FailoverState::Recovering,
            (FailoverState::Recovering, FailoverEvent::RecoveryCompleted) => {
                self.active = LinkType::Ethernet;
                FailoverState::PrimaryActive
            }
            _ => return Err(FailoverError::InvalidState),
        };
        self.state = new_state;
        if let Some(cb) = self.callback {
            cb(event);
        }
        Ok(self.active)
    }

    /// Returns the currently active link.
    pub fn current_active(&self) -> LinkType {
        self.active
    }

    /// Returns the current failover state.
    pub fn state(&self) -> FailoverState {
        self.state
    }

    /// Force a switch to the target link.
    ///
    /// If the target is already the active link, this is a no-op.
    /// Otherwise, transitions to `Switching` state with the target as
    /// the active link, and increments the failover counter.
    pub fn force_switch(&mut self, target: LinkType, now: u64) -> Result<(), FailoverError> {
        if target == self.active {
            return Ok(());
        }
        self.state = FailoverState::Switching;
        self.active = target;
        self.failover_count = self.failover_count.saturating_add(1);
        self.last_failover_time = now;
        Ok(())
    }

    /// Register a callback to be invoked on state transitions.
    ///
    /// The callback receives the event that triggered the transition.
    /// Only `on_event` transitions invoke the callback; `force_switch`
    /// does not.
    pub fn register_callback(&mut self, cb: fn(FailoverEvent)) {
        self.callback = Some(cb);
    }

    /// Check heartbeats and return an event if action is needed.
    ///
    /// - If Ethernet is active, checks primary heartbeat. Returns
    ///   `Some(PrimaryDown)` if the primary has timed out.
    /// - If Cellular is active, checks backup heartbeat. Returns `None`
    ///   if the backup has timed out (no further fallback available).
    ///   Otherwise, when in `BackupActive` state, checks if primary has
    ///   recovered (via `on_primary_heartbeat`) and returns
    ///   `Some(PrimaryUp)` if so.
    pub fn check_heartbeats(&mut self, now: u64) -> Option<FailoverEvent> {
        match self.active {
            LinkType::Ethernet => {
                if self.heartbeat_primary.check_timeout(now) {
                    Some(FailoverEvent::PrimaryDown)
                } else {
                    None
                }
            }
            LinkType::Cellular => {
                if self.heartbeat_backup.check_timeout(now) {
                    None
                } else if self.state == FailoverState::BackupActive
                    && self.heartbeat_primary.is_alive()
                {
                    Some(FailoverEvent::PrimaryUp)
                } else {
                    None
                }
            }
        }
    }

    /// Returns the total number of failovers that have occurred.
    pub fn failover_count(&self) -> u32 {
        self.failover_count
    }

    /// Record a heartbeat on the primary link at timestamp `now`.
    ///
    /// Resets the primary heartbeat monitor's miss counter.
    pub fn on_primary_heartbeat(&mut self, now: u64) {
        self.heartbeat_primary.on_heartbeat(now);
    }

    /// Record a heartbeat on the backup link at timestamp `now`.
    ///
    /// Resets the backup heartbeat monitor's miss counter.
    pub fn on_backup_heartbeat(&mut self, now: u64) {
        self.heartbeat_backup.on_heartbeat(now);
    }

    /// Returns the configured recovery delay in milliseconds.
    pub fn recovery_delay_ms(&self) -> u64 {
        self.recovery_delay_ms
    }

    /// Returns the timestamp of the last failover.
    pub fn last_failover_time(&self) -> u64 {
        self.last_failover_time
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    // Separate counters per test to avoid interference between parallel tests.
    static CB_COUNT_REGISTER: AtomicU32 = AtomicU32::new(0);
    static CB_COUNT_INVOKE: AtomicU32 = AtomicU32::new(0);
    static CB_COUNT_INVALID: AtomicU32 = AtomicU32::new(0);

    fn cb_register(_e: FailoverEvent) {
        CB_COUNT_REGISTER.fetch_add(1, Ordering::SeqCst);
    }
    fn cb_invoke(_e: FailoverEvent) {
        CB_COUNT_INVOKE.fetch_add(1, Ordering::SeqCst);
    }
    fn cb_invalid(_e: FailoverEvent) {
        CB_COUNT_INVALID.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn new_initial_state() {
        let fm = FailoverManager::new(5000);
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
        assert_eq!(fm.failover_count(), 0);
        assert_eq!(fm.recovery_delay_ms(), 5000);
        assert_eq!(fm.last_failover_time(), 0);
    }

    #[test]
    fn on_event_primary_down_transitions_to_switching() {
        let mut fm = FailoverManager::new(5000);
        let result = fm.on_event(FailoverEvent::PrimaryDown, 1000);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(fm.state(), FailoverState::Switching);
        assert_eq!(fm.current_active(), LinkType::Cellular);
        assert_eq!(fm.failover_count(), 1);
        assert_eq!(fm.last_failover_time(), 1000);
    }

    #[test]
    fn on_event_switch_completed_transitions_to_backup_active() {
        let mut fm = FailoverManager::new(5000);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        let result = fm.on_event(FailoverEvent::SwitchCompleted, 1100);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(fm.state(), FailoverState::BackupActive);
        assert_eq!(fm.current_active(), LinkType::Cellular);
    }

    #[test]
    fn on_event_primary_up_transitions_to_recovering() {
        let mut fm = FailoverManager::new(5000);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        let result = fm.on_event(FailoverEvent::PrimaryUp, 2000);
        assert_eq!(result, Ok(LinkType::Cellular));
        assert_eq!(fm.state(), FailoverState::Recovering);
        assert_eq!(fm.current_active(), LinkType::Cellular);
    }

    #[test]
    fn on_event_recovery_completed_transitions_to_primary_active() {
        let mut fm = FailoverManager::new(5000);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        fm.on_event(FailoverEvent::PrimaryUp, 2000).unwrap();
        let result = fm.on_event(FailoverEvent::RecoveryCompleted, 7000);
        assert_eq!(result, Ok(LinkType::Ethernet));
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn on_event_invalid_transition_returns_error() {
        let mut fm = FailoverManager::new(5000);

        // PrimaryActive + SwitchCompleted is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::SwitchCompleted, 1000),
            Err(FailoverError::InvalidState)
        );

        // PrimaryActive + PrimaryUp is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryUp, 1000),
            Err(FailoverError::InvalidState)
        );

        // PrimaryActive + RecoveryCompleted is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::RecoveryCompleted, 1000),
            Err(FailoverError::InvalidState)
        );

        // Transition to BackupActive
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();

        // BackupActive + PrimaryDown is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryDown, 1200),
            Err(FailoverError::InvalidState)
        );

        // BackupActive + SwitchCompleted is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::SwitchCompleted, 1200),
            Err(FailoverError::InvalidState)
        );

        // BackupActive + RecoveryCompleted is invalid
        assert_eq!(
            fm.on_event(FailoverEvent::RecoveryCompleted, 1200),
            Err(FailoverError::InvalidState)
        );
    }

    #[test]
    fn on_event_invalid_transitions_in_switching_and_recovering() {
        let mut fm = FailoverManager::new(5000);

        // Switching state
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryDown, 1100),
            Err(FailoverError::InvalidState)
        );
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryUp, 1100),
            Err(FailoverError::InvalidState)
        );
        assert_eq!(
            fm.on_event(FailoverEvent::RecoveryCompleted, 1100),
            Err(FailoverError::InvalidState)
        );

        // Recovering state
        fm.on_event(FailoverEvent::SwitchCompleted, 1200).unwrap();
        fm.on_event(FailoverEvent::PrimaryUp, 1300).unwrap();
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryDown, 1400),
            Err(FailoverError::InvalidState)
        );
        assert_eq!(
            fm.on_event(FailoverEvent::SwitchCompleted, 1400),
            Err(FailoverError::InvalidState)
        );
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryUp, 1400),
            Err(FailoverError::InvalidState)
        );
    }

    #[test]
    fn current_active_returns_active_link() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        assert_eq!(fm.current_active(), LinkType::Cellular);
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        assert_eq!(fm.current_active(), LinkType::Cellular);
        fm.on_event(FailoverEvent::PrimaryUp, 2000).unwrap();
        assert_eq!(fm.current_active(), LinkType::Cellular);
        fm.on_event(FailoverEvent::RecoveryCompleted, 7000).unwrap();
        assert_eq!(fm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn state_returns_current_state() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        assert_eq!(fm.state(), FailoverState::Switching);
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        assert_eq!(fm.state(), FailoverState::BackupActive);
        fm.on_event(FailoverEvent::PrimaryUp, 2000).unwrap();
        assert_eq!(fm.state(), FailoverState::Recovering);
        fm.on_event(FailoverEvent::RecoveryCompleted, 7000).unwrap();
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
    }

    #[test]
    fn force_switch_to_same_link_is_noop() {
        let mut fm = FailoverManager::new(5000);
        let result = fm.force_switch(LinkType::Ethernet, 1000);
        assert!(result.is_ok());
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
        assert_eq!(fm.failover_count(), 0);
        assert_eq!(fm.last_failover_time(), 0);
    }

    #[test]
    fn force_switch_to_different_link_transitions_to_switching() {
        let mut fm = FailoverManager::new(5000);
        let result = fm.force_switch(LinkType::Cellular, 1000);
        assert!(result.is_ok());
        assert_eq!(fm.state(), FailoverState::Switching);
        assert_eq!(fm.current_active(), LinkType::Cellular);
        assert_eq!(fm.last_failover_time(), 1000);
    }

    #[test]
    fn force_switch_increments_failover_count() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.failover_count(), 0);

        fm.force_switch(LinkType::Cellular, 1000).unwrap();
        assert_eq!(fm.failover_count(), 1);
        assert_eq!(fm.last_failover_time(), 1000);

        fm.force_switch(LinkType::Ethernet, 2000).unwrap();
        assert_eq!(fm.failover_count(), 2);
        assert_eq!(fm.last_failover_time(), 2000);

        // Same-link force_switch does not increment
        fm.force_switch(LinkType::Ethernet, 3000).unwrap();
        assert_eq!(fm.failover_count(), 2);
    }

    #[test]
    fn register_callback_sets_callback() {
        let mut fm = FailoverManager::new(5000);
        fm.register_callback(cb_register);
        CB_COUNT_REGISTER.store(0, Ordering::SeqCst);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        assert_eq!(CB_COUNT_REGISTER.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callback_invoked_on_event() {
        let mut fm = FailoverManager::new(5000);
        fm.register_callback(cb_invoke);
        CB_COUNT_INVOKE.store(0, Ordering::SeqCst);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        fm.on_event(FailoverEvent::PrimaryUp, 2000).unwrap();
        fm.on_event(FailoverEvent::RecoveryCompleted, 7000).unwrap();
        assert_eq!(CB_COUNT_INVOKE.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn callback_not_invoked_on_invalid_transition() {
        let mut fm = FailoverManager::new(5000);
        fm.register_callback(cb_invalid);
        CB_COUNT_INVALID.store(0, Ordering::SeqCst);
        let _ = fm.on_event(FailoverEvent::SwitchCompleted, 1000);
        let _ = fm.on_event(FailoverEvent::PrimaryUp, 1000);
        assert_eq!(CB_COUNT_INVALID.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn check_heartbeats_no_timeout_returns_none() {
        let mut fm = FailoverManager::new(5000);
        fm.on_primary_heartbeat(1000);
        // 2000 - 1000 = 1000 < 3000, no miss
        assert_eq!(fm.check_heartbeats(2000), None);
        // 3500 - 1000 = 2500 < 3000, no miss
        assert_eq!(fm.check_heartbeats(3500), None);
    }

    #[test]
    fn check_heartbeats_primary_timeout_returns_primary_down() {
        let mut fm = FailoverManager::new(5000);
        fm.on_primary_heartbeat(1000);
        // max_missed = 3; need three misses to trigger failure.
        assert_eq!(fm.check_heartbeats(4000), None); // miss 1 (4000-1000=3000 >= 3000)
        assert_eq!(fm.check_heartbeats(7000), None); // miss 2
        assert_eq!(fm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown));
        // miss 3 -> failure
    }

    #[test]
    fn on_primary_heartbeat_resets_missed_count() {
        let mut fm = FailoverManager::new(5000);
        fm.on_primary_heartbeat(1000);
        // Accumulate 2 misses.
        assert_eq!(fm.check_heartbeats(4000), None); // miss 1
        assert_eq!(fm.check_heartbeats(7000), None); // miss 2
                                                     // Receive a fresh heartbeat -> reset.
        fm.on_primary_heartbeat(7500);
        // Need three more misses to fail.
        assert_eq!(fm.check_heartbeats(10500), None); // miss 1
        assert_eq!(fm.check_heartbeats(13500), None); // miss 2
        assert_eq!(fm.check_heartbeats(16500), Some(FailoverEvent::PrimaryDown));
        // miss 3 -> failure
    }

    #[test]
    fn on_backup_heartbeat_resets_missed_count() {
        let mut fm = FailoverManager::new(5000);
        // Drive a normal failover so primary is genuinely down.
        fm.on_primary_heartbeat(1000);
        assert_eq!(fm.check_heartbeats(4000), None); // primary miss 1
        assert_eq!(fm.check_heartbeats(7000), None); // primary miss 2
        assert_eq!(fm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown)); // primary miss 3
        fm.on_event(FailoverEvent::PrimaryDown, 10000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 10100).unwrap();
        // Now in BackupActive; primary heartbeat is dead.
        // Start receiving backup heartbeats.
        fm.on_backup_heartbeat(10200);
        assert_eq!(fm.check_heartbeats(11200), None); // backup alive, primary still dead

        // Accumulate backup misses.
        assert_eq!(fm.check_heartbeats(13200), None); // backup miss 1 (13200-10200=3000)
        assert_eq!(fm.check_heartbeats(16200), None); // backup miss 2

        // Fresh backup heartbeat resets the miss counter.
        fm.on_backup_heartbeat(17000);
        assert_eq!(fm.check_heartbeats(18000), None); // 18000-17000=1000 < 3000, no miss
    }

    #[test]
    fn failover_count_returns_count() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.failover_count(), 0);
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        assert_eq!(fm.failover_count(), 1);
    }

    #[test]
    fn recovery_delay_ms_returns_configured_value() {
        let fm = FailoverManager::new(7000);
        assert_eq!(fm.recovery_delay_ms(), 7000);
    }

    #[test]
    fn last_failover_time_returns_timestamp() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.last_failover_time(), 0);
        fm.on_event(FailoverEvent::PrimaryDown, 5000).unwrap();
        assert_eq!(fm.last_failover_time(), 5000);
    }

    #[test]
    fn full_failover_flow() {
        let mut fm = FailoverManager::new(5000);
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);

        // Primary goes down.
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryDown, 1000),
            Ok(LinkType::Cellular)
        );
        assert_eq!(fm.state(), FailoverState::Switching);
        assert_eq!(fm.current_active(), LinkType::Cellular);
        assert_eq!(fm.failover_count(), 1);
        assert_eq!(fm.last_failover_time(), 1000);

        // Switch completes.
        assert_eq!(
            fm.on_event(FailoverEvent::SwitchCompleted, 1100),
            Ok(LinkType::Cellular)
        );
        assert_eq!(fm.state(), FailoverState::BackupActive);

        // Primary comes back.
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryUp, 2000),
            Ok(LinkType::Cellular)
        );
        assert_eq!(fm.state(), FailoverState::Recovering);
        assert_eq!(fm.current_active(), LinkType::Cellular);

        // Recovery completes after the configured delay.
        assert_eq!(
            fm.on_event(FailoverEvent::RecoveryCompleted, 7000),
            Ok(LinkType::Ethernet)
        );
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn anti_flap_recovery_waits_for_recovery_completed() {
        let mut fm = FailoverManager::new(5000);

        // Trigger failover to backup.
        fm.on_event(FailoverEvent::PrimaryDown, 1000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 1100).unwrap();
        assert_eq!(fm.state(), FailoverState::BackupActive);

        // Primary comes back; enter recovering.
        fm.on_event(FailoverEvent::PrimaryUp, 2000).unwrap();
        assert_eq!(fm.state(), FailoverState::Recovering);
        assert_eq!(fm.current_active(), LinkType::Cellular);

        // While recovering, PrimaryDown is invalid (anti-flap).
        assert_eq!(
            fm.on_event(FailoverEvent::PrimaryDown, 3000),
            Err(FailoverError::InvalidState)
        );

        // Still in recovering until RecoveryCompleted is issued.
        assert_eq!(fm.state(), FailoverState::Recovering);

        // After the recovery delay, complete recovery.
        fm.on_event(FailoverEvent::RecoveryCompleted, 7000).unwrap();
        assert_eq!(fm.state(), FailoverState::PrimaryActive);
        assert_eq!(fm.current_active(), LinkType::Ethernet);
    }

    #[test]
    fn check_heartbeats_backup_timeout_returns_none() {
        let mut fm = FailoverManager::new(5000);
        // Normal failover so primary is genuinely down.
        fm.on_primary_heartbeat(1000);
        assert_eq!(fm.check_heartbeats(4000), None);
        assert_eq!(fm.check_heartbeats(7000), None);
        assert_eq!(fm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown));
        fm.on_event(FailoverEvent::PrimaryDown, 10000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 10100).unwrap();

        // Backup never receives heartbeats; after 3 misses it times out.
        // last_heartbeat for backup is 0 (never received), so misses
        // accumulate from t=0.
        assert_eq!(fm.check_heartbeats(3000), None); // miss 1
        assert_eq!(fm.check_heartbeats(6000), None); // miss 2
                                                     // miss 3 -> backup failure, but no further fallback available.
        assert_eq!(fm.check_heartbeats(9000), None);
    }

    #[test]
    fn check_heartbeats_detects_primary_recovery_in_backup_active() {
        let mut fm = FailoverManager::new(5000);
        // Normal failover so primary is genuinely down.
        fm.on_primary_heartbeat(1000);
        assert_eq!(fm.check_heartbeats(4000), None);
        assert_eq!(fm.check_heartbeats(7000), None);
        assert_eq!(fm.check_heartbeats(10000), Some(FailoverEvent::PrimaryDown));
        fm.on_event(FailoverEvent::PrimaryDown, 10000).unwrap();
        fm.on_event(FailoverEvent::SwitchCompleted, 10100).unwrap();

        // Backup is healthy.
        fm.on_backup_heartbeat(10200);
        // Primary still dead -> no event.
        assert_eq!(fm.check_heartbeats(11000), None);

        // Primary recovers.
        fm.on_primary_heartbeat(12000);
        // Backup still alive, primary alive -> PrimaryUp.
        assert_eq!(fm.check_heartbeats(12500), Some(FailoverEvent::PrimaryUp));
    }

    #[test]
    fn force_switch_does_not_invoke_callback() {
        static CB_COUNT_FORCE: AtomicU32 = AtomicU32::new(0);
        fn cb_force(_e: FailoverEvent) {
            CB_COUNT_FORCE.fetch_add(1, Ordering::SeqCst);
        }
        let mut fm = FailoverManager::new(5000);
        fm.register_callback(cb_force);
        CB_COUNT_FORCE.store(0, Ordering::SeqCst);
        fm.force_switch(LinkType::Cellular, 1000).unwrap();
        assert_eq!(CB_COUNT_FORCE.load(Ordering::SeqCst), 0);
    }
}
