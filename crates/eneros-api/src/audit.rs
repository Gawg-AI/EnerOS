//! Audit logging for security-sensitive operations (v0.6.0 — S1).
//!
//! Records all write operations (POST/PUT/DELETE) with who/what/when/result/IP.
//! Supports both in-memory and file-based audit logs.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID
    pub id: String,
    /// Timestamp (Unix epoch seconds)
    pub timestamp: i64,
    /// Authenticated user/principal name
    pub actor: String,
    /// Role of the actor
    pub role: String,
    /// HTTP method
    pub method: String,
    /// Request path
    pub path: String,
    /// Client IP address
    pub client_ip: String,
    /// Result: "success" | "failed" | "denied"
    pub result: String,
    /// Optional detail/error message
    pub detail: Option<String>,
}

impl AuditEntry {
    /// Create a new audit entry.
    pub fn new(
        actor: impl Into<String>,
        role: impl Into<String>,
        method: impl Into<String>,
        path: impl Into<String>,
        client_ip: impl Into<String>,
        result: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            actor: actor.into(),
            role: role.into(),
            method: method.into(),
            path: path.into(),
            client_ip: client_ip.into(),
            result: result.into(),
            detail: None,
        }
    }

    /// Add detail to the entry.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// In-memory audit log with optional file persistence.
pub struct AuditLog {
    entries: RwLock<Vec<AuditEntry>>,
    max_entries: usize,
    file_path: Option<std::path::PathBuf>,
}

impl AuditLog {
    /// Create a new in-memory audit log.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
            file_path: None,
        }
    }

    /// Create a file-backed audit log.
    pub fn with_file(max_entries: usize, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
            file_path: Some(path.into()),
        }
    }

    /// Record an audit entry.
    pub fn record(&self, entry: AuditEntry) {
        let mut entries = self.entries.write();
        entries.push(entry);

        // Trim if exceeding max
        if entries.len() > self.max_entries {
            let excess = entries.len() - self.max_entries;
            entries.drain(0..excess);
        }

        // Log to tracing
        tracing::info!(
            actor = %entries.last().unwrap().actor,
            method = %entries.last().unwrap().method,
            path = %entries.last().unwrap().path,
            result = %entries.last().unwrap().result,
            "audit: {} {} {} by {} ({})",
            entries.last().unwrap().method,
            entries.last().unwrap().path,
            entries.last().unwrap().result,
            entries.last().unwrap().actor,
            entries.last().unwrap().role,
        );

        // Optional file write
        if let Some(ref path) = self.file_path {
            if let Ok(line) = serde_json::to_string(entries.last().unwrap()) {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "{}", line);
                }
            }
        }
    }

    /// Query audit entries with optional filters.
    pub fn query(
        &self,
        actor: Option<&str>,
        result: Option<&str>,
        limit: usize,
    ) -> Vec<AuditEntry> {
        let entries = self.entries.read();
        entries
            .iter()
            .rev()
            .filter(|e| actor.map(|a| e.actor == a).unwrap_or(true))
            .filter(|e| result.map(|r| e.result == r).unwrap_or(true))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Get total entry count.
    pub fn count(&self) -> usize {
        self.entries.read().len()
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.entries.write().clear();
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new(10_000)
    }
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("max_entries", &self.max_entries)
            .field("file_path", &self.file_path)
            .field("current_count", &self.entries.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry::new(
            "alice",
            "operator",
            "POST",
            "/api/actions/structured",
            "192.168.1.1",
            "success",
        );
        assert_eq!(entry.actor, "alice");
        assert_eq!(entry.role, "operator");
        assert_eq!(entry.method, "POST");
        assert_eq!(entry.result, "success");
        assert!(entry.detail.is_none());
    }

    #[test]
    fn test_audit_entry_with_detail() {
        let entry = AuditEntry::new("bob", "observer", "GET", "/api/agents", "10.0.0.1", "success")
            .with_detail("list all agents");
        assert_eq!(entry.detail.as_deref(), Some("list all agents"));
    }

    #[test]
    fn test_audit_log_record_and_query() {
        let log = AuditLog::new(100);
        log.record(AuditEntry::new(
            "alice",
            "operator",
            "POST",
            "/api/actions",
            "1.1.1.1",
            "success",
        ));
        log.record(AuditEntry::new(
            "bob",
            "observer",
            "GET",
            "/api/agents",
            "2.2.2.2",
            "success",
        ));
        log.record(AuditEntry::new(
            "alice",
            "operator",
            "POST",
            "/api/actions",
            "1.1.1.1",
            "failed",
        ));

        assert_eq!(log.count(), 3);

        // Query by actor
        let alice_entries = log.query(Some("alice"), None, 10);
        assert_eq!(alice_entries.len(), 2);

        // Query by result
        let failed = log.query(None, Some("failed"), 10);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].actor, "alice");

        // Query with limit
        let limited = log.query(None, None, 2);
        assert_eq!(limited.len(), 2);
        // Most recent first
        assert_eq!(limited[0].result, "failed");
    }

    #[test]
    fn test_audit_log_max_entries_trim() {
        let log = AuditLog::new(3);
        for i in 0..5 {
            log.record(AuditEntry::new(
                "user",
                "role",
                "POST",
                format!("/api/{}", i),
                "1.1.1.1",
                "success",
            ));
        }
        assert_eq!(log.count(), 3);
        // Oldest entries should be trimmed
        let entries = log.query(None, None, 10);
        assert_eq!(entries[2].path, "/api/2"); // oldest remaining
        assert_eq!(entries[0].path, "/api/4"); // newest
    }

    #[test]
    fn test_audit_log_clear() {
        let log = AuditLog::new(100);
        log.record(AuditEntry::new(
            "alice",
            "operator",
            "POST",
            "/api/test",
            "1.1.1.1",
            "success",
        ));
        assert_eq!(log.count(), 1);
        log.clear();
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn test_audit_log_default() {
        let log = AuditLog::default();
        assert_eq!(log.count(), 0);
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry::new("alice", "operator", "POST", "/api/test", "1.1.1.1", "success")
            .with_detail("test detail");
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.actor, deserialized.actor);
        assert_eq!(entry.detail, deserialized.detail);
    }
}
