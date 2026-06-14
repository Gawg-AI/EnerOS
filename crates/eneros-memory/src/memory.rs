use async_trait::async_trait;
use eneros_core::Result;
use parking_lot::RwLock;
use std::collections::HashMap;

use crate::types::{MemoryEntry, RecallQuery};

/// Agent memory trait — unified interface for memory operations
#[async_trait]
pub trait AgentMemory: Send + Sync {
    /// Store a memory entry
    async fn store(&self, agent_id: &str, entry: MemoryEntry) -> Result<()>;

    /// Recall memories matching a query
    async fn recall(&self, agent_id: &str, query: &RecallQuery) -> Result<Vec<MemoryEntry>>;

    /// Forget (delete) a specific memory entry
    async fn forget(&self, agent_id: &str, entry_id: &str) -> Result<()>;

    /// Clear all memories for an agent
    async fn clear(&self, agent_id: &str) -> Result<()>;

    /// Count memories for an agent
    async fn count(&self, agent_id: &str) -> usize;
}

/// In-memory implementation of AgentMemory
pub struct InMemoryMemory {
    short_term: RwLock<HashMap<String, Vec<MemoryEntry>>>,
    long_term: RwLock<HashMap<String, Vec<MemoryEntry>>>,
    short_term_capacity: usize,
    consolidation_threshold: f64,
}

impl InMemoryMemory {
    /// Create a new InMemoryMemory
    pub fn new(short_term_capacity: usize, consolidation_threshold: f64) -> Self {
        Self {
            short_term: RwLock::new(HashMap::new()),
            long_term: RwLock::new(HashMap::new()),
            short_term_capacity,
            consolidation_threshold,
        }
    }

    /// Consolidate high-importance short-term memories into long-term
    pub fn consolidate(&self, agent_id: &str) -> usize {
        let mut short_term = self.short_term.write();
        let mut long_term = self.long_term.write();

        let entries = short_term.entry(agent_id.to_string()).or_default();
        let mut promoted = 0;

        entries.retain(|entry| {
            if entry.importance >= self.consolidation_threshold {
                long_term
                    .entry(agent_id.to_string())
                    .or_default()
                    .push(entry.clone());
                promoted += 1;
                false
            } else {
                true
            }
        });

        promoted
    }
}

impl Default for InMemoryMemory {
    fn default() -> Self {
        Self::new(1000, 0.7)
    }
}

#[async_trait]
impl AgentMemory for InMemoryMemory {
    async fn store(&self, agent_id: &str, entry: MemoryEntry) -> Result<()> {
        let mut short_term = self.short_term.write();
        let entries = short_term.entry(agent_id.to_string()).or_default();

        // Auto-promote high-importance entries directly to long-term
        if entry.importance >= self.consolidation_threshold {
            let mut long_term = self.long_term.write();
            long_term
                .entry(agent_id.to_string())
                .or_default()
                .push(entry);
        } else {
            entries.push(entry);

            // FIFO eviction when capacity exceeded
            while entries.len() > self.short_term_capacity {
                entries.remove(0);
            }
        }

        Ok(())
    }

    async fn recall(&self, agent_id: &str, query: &RecallQuery) -> Result<Vec<MemoryEntry>> {
        let mut results = Vec::new();

        // Search short-term memory
        {
            let short_term = self.short_term.read();
            if let Some(entries) = short_term.get(agent_id) {
                for entry in entries {
                    if query.matches(entry) {
                        results.push(entry.clone());
                    }
                }
            }
        }

        // Search long-term memory
        {
            let long_term = self.long_term.read();
            if let Some(entries) = long_term.get(agent_id) {
                for entry in entries {
                    if query.matches(entry) {
                        results.push(entry.clone());
                    }
                }
            }
        }

        // Sort by importance (descending) then by timestamp (newest first)
        results.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
        });

        // Apply limit
        results.truncate(query.limit);

        // Increment access count
        for entry in &mut results {
            let mut short_term = self.short_term.write();
            if let Some(entries) = short_term.get_mut(agent_id) {
                for e in entries {
                    if e.id == entry.id {
                        e.access_count += 1;
                        entry.access_count = e.access_count;
                        break;
                    }
                }
            }
            let mut long_term = self.long_term.write();
            if let Some(entries) = long_term.get_mut(agent_id) {
                for e in entries {
                    if e.id == entry.id {
                        e.access_count += 1;
                        entry.access_count = e.access_count;
                        break;
                    }
                }
            }
        }

        Ok(results)
    }

    async fn forget(&self, agent_id: &str, entry_id: &str) -> Result<()> {
        {
            let mut short_term = self.short_term.write();
            if let Some(entries) = short_term.get_mut(agent_id) {
                entries.retain(|e| e.id != entry_id);
            }
        }
        {
            let mut long_term = self.long_term.write();
            if let Some(entries) = long_term.get_mut(agent_id) {
                entries.retain(|e| e.id != entry_id);
            }
        }
        Ok(())
    }

    async fn clear(&self, agent_id: &str) -> Result<()> {
        {
            let mut short_term = self.short_term.write();
            short_term.remove(agent_id);
        }
        {
            let mut long_term = self.long_term.write();
            long_term.remove(agent_id);
        }
        Ok(())
    }

    async fn count(&self, agent_id: &str) -> usize {
        let short_count = {
            let short_term = self.short_term.read();
            short_term.get(agent_id).map(|e| e.len()).unwrap_or(0)
        };
        let long_count = {
            let long_term = self.long_term.read();
            long_term.get(agent_id).map(|e| e.len()).unwrap_or(0)
        };
        short_count + long_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryType;

    #[tokio::test]
    async fn test_memory_store_and_recall() {
        let memory = InMemoryMemory::default();
        let entry = MemoryEntry::new(
            MemoryType::Episodic,
            "Bus 3 voltage violation at 14:00".to_string(),
            0.8,
        );

        memory.store("agent-1", entry).await.unwrap();
        let results = memory
            .recall("agent-1", &RecallQuery::new())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Bus 3 voltage violation at 14:00");
    }

    #[tokio::test]
    async fn test_memory_recall_by_type() {
        let memory = InMemoryMemory::default();
        let e1 = MemoryEntry::new(MemoryType::Episodic, "event 1".to_string(), 0.5);
        let e2 = MemoryEntry::new(MemoryType::Semantic, "knowledge 1".to_string(), 0.5);

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        let results = memory
            .recall("agent-1", &RecallQuery::new().with_type(MemoryType::Semantic))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_type, MemoryType::Semantic);
    }

    #[tokio::test]
    async fn test_memory_recall_by_tags() {
        let memory = InMemoryMemory::default();
        let e1 = MemoryEntry::new(MemoryType::Episodic, "voltage event".to_string(), 0.5)
            .with_tags(vec!["voltage".to_string(), "bus3".to_string()]);
        let e2 = MemoryEntry::new(MemoryType::Episodic, "frequency event".to_string(), 0.5)
            .with_tags(vec!["frequency".to_string()]);

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        let results = memory
            .recall("agent-1", &RecallQuery::new().with_tags(vec!["voltage".to_string()]))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].tags.contains(&"voltage".to_string()));
    }

    #[tokio::test]
    async fn test_memory_recall_by_importance() {
        let memory = InMemoryMemory::default();
        let e1 = MemoryEntry::new(MemoryType::Episodic, "low importance".to_string(), 0.3);
        let e2 = MemoryEntry::new(MemoryType::Episodic, "high importance".to_string(), 0.9);

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        let results = memory
            .recall("agent-1", &RecallQuery::new().with_min_importance(0.7))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "high importance");
    }

    #[tokio::test]
    async fn test_memory_forget() {
        let memory = InMemoryMemory::default();
        let entry = MemoryEntry::new(MemoryType::Episodic, "to be forgotten".to_string(), 0.5);

        memory.store("agent-1", entry.clone()).await.unwrap();
        memory.forget("agent-1", &entry.id).await.unwrap();

        let count = memory.count("agent-1").await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_memory_consolidation() {
        let memory = InMemoryMemory::new(100, 0.7);
        let e1 = MemoryEntry::new(MemoryType::Episodic, "low importance".to_string(), 0.3);
        let e2 = MemoryEntry::new(MemoryType::Episodic, "high importance".to_string(), 0.8);

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        // High importance should go directly to long-term
        let count = memory.count("agent-1").await;
        assert_eq!(count, 2);

        // Consolidate should move the high-importance from short-term (already in long-term)
        let promoted = memory.consolidate("agent-1");
        assert_eq!(promoted, 0); // Already promoted during store
    }

    #[tokio::test]
    async fn test_memory_clear() {
        let memory = InMemoryMemory::default();
        let entry = MemoryEntry::new(MemoryType::Episodic, "test".to_string(), 0.5);

        memory.store("agent-1", entry).await.unwrap();
        memory.clear("agent-1").await.unwrap();

        let count = memory.count("agent-1").await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_memory_keyword_search() {
        let memory = InMemoryMemory::default();
        let e1 = MemoryEntry::new(MemoryType::Episodic, "Voltage violation on bus 3".to_string(), 0.5);
        let e2 = MemoryEntry::new(MemoryType::Episodic, "Frequency deviation detected".to_string(), 0.5);

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        let results = memory
            .recall("agent-1", &RecallQuery::new().with_keyword("voltage"))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.to_lowercase().contains("voltage"));
    }
}
