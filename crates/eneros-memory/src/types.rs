use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Memory type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryType {
    /// Event memory (e.g., "Bus 3 voltage violation at 14:00")
    Episodic,
    /// Knowledge memory (e.g., "Bus 3 is a PV bus with condenser")
    Semantic,
    /// Procedural memory (e.g., "When N-1 violation: adjust voltage first, then shed load")
    Procedural,
}

/// A single memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique entry ID
    pub id: String,
    /// Memory type
    pub memory_type: MemoryType,
    /// Content (JSON-serialized)
    pub content: String,
    /// Importance weight (0.0~1.0)
    pub importance: f64,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Tags for filtering
    pub tags: Vec<String>,
    /// Access count
    pub access_count: u32,
}

impl MemoryEntry {
    /// Create a new memory entry
    pub fn new(memory_type: MemoryType, content: String, importance: f64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            memory_type,
            content,
            importance: importance.clamp(0.0, 1.0),
            timestamp: Utc::now(),
            tags: Vec::new(),
            access_count: 0,
        }
    }

    /// Create with tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// Query for recalling memories
#[derive(Debug, Clone, Default)]
pub struct RecallQuery {
    /// Filter by memory type
    pub memory_type: Option<MemoryType>,
    /// Filter by tags (match any)
    pub tags: Vec<String>,
    /// Keyword search in content
    pub keyword: Option<String>,
    /// Minimum importance threshold
    pub min_importance: Option<f64>,
    /// Maximum number of results
    pub limit: usize,
    /// Time range filter
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

impl RecallQuery {
    /// Create a new query with default limit
    pub fn new() -> Self {
        Self {
            limit: 100,
            ..Default::default()
        }
    }

    /// Filter by memory type
    pub fn with_type(mut self, memory_type: MemoryType) -> Self {
        self.memory_type = Some(memory_type);
        self
    }

    /// Filter by tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Filter by keyword
    pub fn with_keyword(mut self, keyword: &str) -> Self {
        self.keyword = Some(keyword.to_string());
        self
    }

    /// Filter by minimum importance
    pub fn with_min_importance(mut self, importance: f64) -> Self {
        self.min_importance = Some(importance);
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Filter by time range
    pub fn with_time_range(mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        self.time_range = Some((start, end));
        self
    }

    /// Check if an entry matches this query
    pub fn matches(&self, entry: &MemoryEntry) -> bool {
        // Type filter
        if let Some(mt) = self.memory_type {
            if entry.memory_type != mt {
                return false;
            }
        }

        // Importance filter
        if let Some(min_imp) = self.min_importance {
            if entry.importance < min_imp {
                return false;
            }
        }

        // Tag filter (match any)
        if !self.tags.is_empty() {
            let has_match = self.tags.iter().any(|t| entry.tags.contains(t));
            if !has_match {
                return false;
            }
        }

        // Keyword filter
        if let Some(ref keyword) = self.keyword {
            if !entry.content.to_lowercase().contains(&keyword.to_lowercase()) {
                return false;
            }
        }

        // Time range filter
        if let Some((start, end)) = self.time_range {
            if entry.timestamp < start || entry.timestamp > end {
                return false;
            }
        }

        true
    }
}
