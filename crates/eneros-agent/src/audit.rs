use std::sync::atomic::{AtomicU64, Ordering};
use eneros_core::AuditEntry;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Filter for querying audit entries
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    /// Filter by agent ID
    pub agent_id: Option<String>,
    /// Filter by minimum entry ID
    pub min_entry_id: Option<u64>,
    /// Filter by maximum entry ID
    pub max_entry_id: Option<u64>,
    /// Filter by action description substring
    pub action_contains: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

/// Audit trail — append-only, immutable log of agent actions
pub struct AuditTrail {
    /// The log entries (append-only)
    entries: RwLock<Vec<AuditEntry>>,
    /// Next entry ID
    next_id: AtomicU64,
    /// Checksum for integrity verification (simple XOR of entry_ids)
    integrity_checksum: AtomicU64,
}

impl AuditTrail {
    /// Create a new empty audit trail
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(1),
            integrity_checksum: AtomicU64::new(0),
        }
    }

    /// Create an audit trail with pre-allocated capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::with_capacity(capacity)),
            next_id: AtomicU64::new(1),
            integrity_checksum: AtomicU64::new(0),
        }
    }

    /// Record a new audit entry (append-only)
    pub fn record(&self, mut entry: AuditEntry) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        entry.entry_id = id;

        // Update integrity checksum
        self.integrity_checksum.fetch_xor(id, Ordering::SeqCst);

        self.entries.write().push(entry);
    }

    /// Query audit entries with filters
    pub fn query(&self, filter: &AuditFilter) -> Vec<AuditEntry> {
        let entries = self.entries.read();
        let mut results: Vec<AuditEntry> = entries
            .iter()
            .filter(|e| {
                if let Some(ref agent_id) = filter.agent_id {
                    if e.agent_id != *agent_id {
                        return false;
                    }
                }
                if let Some(min_id) = filter.min_entry_id {
                    if e.entry_id < min_id {
                        return false;
                    }
                }
                if let Some(max_id) = filter.max_entry_id {
                    if e.entry_id > max_id {
                        return false;
                    }
                }
                if let Some(ref action_contains) = filter.action_contains {
                    if !e.action_description.contains(action_contains.as_str()) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        results
    }

    /// Get total number of entries
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Check if the trail is empty
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// Verify integrity of the audit trail
    /// Returns true if the checksum matches the expected value
    pub fn verify_integrity(&self) -> bool {
        let entries = self.entries.read();
        let expected: u64 = entries.iter().map(|e| e.entry_id).fold(0u64, |acc, id| acc ^ id);
        let stored = self.integrity_checksum.load(Ordering::SeqCst);
        expected == stored
    }

    /// Get the last N entries
    pub fn last_n(&self, n: usize) -> Vec<AuditEntry> {
        let entries = self.entries.read();
        let start = if entries.len() > n { entries.len() - n } else { 0 };
        entries[start..].to_vec()
    }

    /// Get all entries (for full export)
    pub fn all_entries(&self) -> Vec<AuditEntry> {
        self.entries.read().clone()
    }
}

impl Default for AuditTrail {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_core::{AuthorityLevel, SystemOperatingState, ActionVerdict};
    use chrono::Utc;

    fn make_entry(agent_id: &str, action: &str) -> AuditEntry {
        AuditEntry {
            entry_id: 0, // Will be assigned by record()
            agent_id: agent_id.to_string(),
            authority_level: AuthorityLevel::Operator,
            action_description: action.to_string(),
            constraint_check_result: "passed".to_string(),
            approval_chain: Vec::new(),
            timestamp: Utc::now(),
            reasoning_summary: "test".to_string(),
            system_state: SystemOperatingState::Normal,
            verdict: ActionVerdict::Approved,
        }
    }

    #[test]
    fn test_record_and_query() {
        let trail = AuditTrail::new();
        trail.record(make_entry("agent-1", "close breaker"));
        trail.record(make_entry("agent-2", "open disconnector"));

        assert_eq!(trail.len(), 2);

        let filter = AuditFilter {
            agent_id: Some("agent-1".to_string()),
            ..Default::default()
        };
        let results = trail.query(&filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, "agent-1");
    }

    #[test]
    fn test_entry_ids_assigned() {
        let trail = AuditTrail::new();
        trail.record(make_entry("a1", "action1"));
        trail.record(make_entry("a2", "action2"));

        let entries = trail.all_entries();
        assert_eq!(entries[0].entry_id, 1);
        assert_eq!(entries[1].entry_id, 2);
    }

    #[test]
    fn test_query_by_action_contains() {
        let trail = AuditTrail::new();
        trail.record(make_entry("a1", "close breaker 101"));
        trail.record(make_entry("a2", "open disconnector 202"));
        trail.record(make_entry("a3", "close breaker 303"));

        let filter = AuditFilter {
            action_contains: Some("close breaker".to_string()),
            ..Default::default()
        };
        let results = trail.query(&filter);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_query_by_entry_id_range() {
        let trail = AuditTrail::new();
        for i in 0..10 {
            trail.record(make_entry("a", &format!("action {}", i)));
        }

        let filter = AuditFilter {
            min_entry_id: Some(3),
            max_entry_id: Some(7),
            ..Default::default()
        };
        let results = trail.query(&filter);
        assert_eq!(results.len(), 5); // IDs 3,4,5,6,7
    }

    #[test]
    fn test_query_with_limit() {
        let trail = AuditTrail::new();
        for i in 0..100 {
            trail.record(make_entry("a", &format!("action {}", i)));
        }

        let filter = AuditFilter {
            limit: Some(10),
            ..Default::default()
        };
        let results = trail.query(&filter);
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_integrity_verification() {
        let trail = AuditTrail::new();
        trail.record(make_entry("a1", "action1"));
        trail.record(make_entry("a2", "action2"));
        trail.record(make_entry("a3", "action3"));

        assert!(trail.verify_integrity());
    }

    #[test]
    fn test_last_n() {
        let trail = AuditTrail::new();
        for i in 0..10 {
            trail.record(make_entry("a", &format!("action {}", i)));
        }

        let last = trail.last_n(3);
        assert_eq!(last.len(), 3);
        assert_eq!(last[0].entry_id, 8);
        assert_eq!(last[2].entry_id, 10);
    }

    #[test]
    fn test_empty_trail() {
        let trail = AuditTrail::new();
        assert!(trail.is_empty());
        assert_eq!(trail.len(), 0);
        assert!(trail.verify_integrity());
    }

    #[test]
    fn test_default_trail() {
        let trail = AuditTrail::default();
        assert!(trail.is_empty());
    }

    #[test]
    fn test_with_capacity() {
        let trail = AuditTrail::with_capacity(100);
        assert!(trail.is_empty());
    }
}
