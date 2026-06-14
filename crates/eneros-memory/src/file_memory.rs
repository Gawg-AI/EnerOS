use async_trait::async_trait;
use eneros_core::Result;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::memory::AgentMemory;
use crate::types::{MemoryEntry, MemoryType, RecallQuery};

/// File-backed persistent implementation of AgentMemory
pub struct FileMemory {
    dir_path: PathBuf,
    /// In-memory cache: agent_id -> memory_type -> entries
    cache: RwLock<HashMap<String, HashMap<MemoryType, Vec<MemoryEntry>>>>,
    short_term_capacity: usize,
    consolidation_threshold: f64,
}

impl FileMemory {
    /// Create a new FileMemory, creating the directory if it doesn't exist
    pub fn new(dir_path: &str) -> Result<Self> {
        Self::with_options(dir_path, 1000, 0.7)
    }

    /// Create a new FileMemory with custom capacity and consolidation threshold
    pub fn with_options(
        dir_path: &str,
        short_term_capacity: usize,
        consolidation_threshold: f64,
    ) -> Result<Self> {
        let dir = Path::new(dir_path);
        fs::create_dir_all(dir).map_err(|e| {
            eneros_core::EnerOSError::Internal(format!("failed to create memory directory: {}", e))
        })?;

        let mut cache = HashMap::new();
        // Pre-load any existing files from the directory
        Self::load_all_from_dir(dir, &mut cache)?;

        Ok(Self {
            dir_path: dir.to_path_buf(),
            cache: RwLock::new(cache),
            short_term_capacity,
            consolidation_threshold,
        })
    }

    fn file_path(&self, agent_id: &str, memory_type: MemoryType) -> PathBuf {
        let type_str = match memory_type {
            MemoryType::Episodic => "episodic",
            MemoryType::Semantic => "semantic",
            MemoryType::Procedural => "procedural",
        };
        self.dir_path.join(format!("{}_{}.json", agent_id, type_str))
    }

    fn load_all_from_dir(
        dir: &Path,
        cache: &mut HashMap<String, HashMap<MemoryType, Vec<MemoryEntry>>>,
    ) -> Result<()> {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let file_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let parts: Vec<&str> = file_name.splitn(2, '_').collect();
            if parts.len() != 2 {
                continue;
            }

            let agent_id = parts[0];
            let memory_type = match parts[1] {
                "episodic" => MemoryType::Episodic,
                "semantic" => MemoryType::Semantic,
                "procedural" => MemoryType::Procedural,
                _ => continue,
            };

            let data = fs::read_to_string(&path).map_err(|e| {
                eneros_core::EnerOSError::Internal(format!("failed to read memory file: {}", e))
            })?;

            let entries: Vec<MemoryEntry> = serde_json::from_str(&data).map_err(|e| {
                eneros_core::EnerOSError::Internal(format!("failed to parse memory file: {}", e))
            })?;

            cache
                .entry(agent_id.to_string())
                .or_default()
                .insert(memory_type, entries);
        }

        Ok(())
    }

    fn persist(&self, agent_id: &str, memory_type: MemoryType) -> Result<()> {
        let cache = self.cache.read();
        let path = self.file_path(agent_id, memory_type);

        if let Some(type_map) = cache.get(agent_id) {
            if let Some(entries) = type_map.get(&memory_type) {
                if entries.is_empty() {
                    // Remove the file if no entries left
                    let _ = fs::remove_file(&path);
                    return Ok(());
                }
                let json = serde_json::to_string_pretty(entries).map_err(|e| {
                    eneros_core::EnerOSError::Internal(format!("failed to serialize memory: {}", e))
                })?;
                fs::write(&path, json).map_err(|e| {
                    eneros_core::EnerOSError::Internal(format!("failed to write memory file: {}", e))
                })?;
            } else {
                let _ = fs::remove_file(&path);
            }
        }

        Ok(())
    }

    /// Consolidate high-importance short-term memories into long-term
    pub fn consolidate(&self, agent_id: &str) -> usize {
        let mut cache = self.cache.write();
        let mut promoted = 0;

        for memory_type in [MemoryType::Episodic, MemoryType::Semantic, MemoryType::Procedural] {
            let type_map = cache.entry(agent_id.to_string()).or_default();
            let entries = type_map.entry(memory_type).or_default();

            let mut to_promote = Vec::new();
            entries.retain(|entry| {
                if entry.importance >= self.consolidation_threshold {
                    to_promote.push(entry.clone());
                    promoted += 1;
                    false
                } else {
                    true
                }
            });

            // High-importance entries are still stored in the same file
            // but we re-add them (they stay in the same type)
            // For simplicity, consolidation here just means they won't be evicted
            // Re-add them back since we use a single storage per type
            for entry in to_promote {
                entries.push(entry);
            }
        }

        promoted
    }
}

#[async_trait]
impl AgentMemory for FileMemory {
    async fn store(&self, agent_id: &str, entry: MemoryEntry) -> Result<()> {
        let memory_type = entry.memory_type;
        let mut cache = self.cache.write();

        let type_map = cache.entry(agent_id.to_string()).or_default();
        let entries = type_map.entry(memory_type).or_default();

        entries.push(entry);

        // FIFO eviction: remove oldest low-importance entries when capacity exceeded
        let type_map_ref = cache.get_mut(agent_id).unwrap();
        let entries_ref = type_map_ref.get_mut(&memory_type).unwrap();
        while entries_ref.len() > self.short_term_capacity {
            // Remove the first low-importance entry; if all are high-importance, remove oldest
            if let Some(pos) = entries_ref.iter().position(|e| e.importance < self.consolidation_threshold) {
                entries_ref.remove(pos);
            } else {
                entries_ref.remove(0);
            }
        }

        drop(cache);
        self.persist(agent_id, memory_type)?;
        Ok(())
    }

    async fn recall(&self, agent_id: &str, query: &RecallQuery) -> Result<Vec<MemoryEntry>> {
        let cache = self.cache.read();
        let mut results = Vec::new();

        if let Some(type_map) = cache.get(agent_id) {
            for entries in type_map.values() {
                for entry in entries {
                    if query.matches(entry) {
                        results.push(entry.clone());
                    }
                }
            }
        }

        drop(cache);

        // Sort by importance (descending) then by timestamp (newest first)
        results.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
        });

        results.truncate(query.limit);

        // Increment access count and collect which types need persisting
        let mut modified_types = Vec::new();
        {
            let mut cache = self.cache.write();
            if let Some(type_map) = cache.get_mut(agent_id) {
                for result in &results {
                    for (&memory_type, entries) in type_map.iter_mut() {
                        for e in entries.iter_mut() {
                            if e.id == result.id {
                                e.access_count += 1;
                                if !modified_types.contains(&memory_type) {
                                    modified_types.push(memory_type);
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Persist updated access counts
        for memory_type in modified_types {
            self.persist(agent_id, memory_type)?;
        }

        Ok(results)
    }

    async fn forget(&self, agent_id: &str, entry_id: &str) -> Result<()> {
        let mut cache = self.cache.write();
        let mut modified_types = Vec::new();

        if let Some(type_map) = cache.get_mut(agent_id) {
            for (&memory_type, entries) in type_map.iter_mut() {
                let before = entries.len();
                entries.retain(|e| e.id != entry_id);
                if entries.len() != before {
                    modified_types.push(memory_type);
                }
            }
        }

        drop(cache);

        for memory_type in modified_types {
            self.persist(agent_id, memory_type)?;
        }

        Ok(())
    }

    async fn clear(&self, agent_id: &str) -> Result<()> {
        let mut cache = self.cache.write();
        let types_to_remove: Vec<MemoryType> = cache
            .get(agent_id)
            .map(|tm| tm.keys().copied().collect())
            .unwrap_or_default();

        cache.remove(agent_id);
        drop(cache);

        for memory_type in types_to_remove {
            let path = self.file_path(agent_id, memory_type);
            let _ = fs::remove_file(path);
        }

        Ok(())
    }

    async fn count(&self, agent_id: &str) -> usize {
        let cache = self.cache.read();
        cache
            .get(agent_id)
            .map(|tm| tm.values().map(|v| v.len()).sum())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(_name: &str) -> String {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("eneros_mem_test_{}_{}", std::process::id(), id));
        let _ = fs::create_dir_all(&dir);
        dir.to_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn test_file_memory_store_and_recall() {
        let dir = temp_dir("store_recall");
        let memory = FileMemory::new(&dir).unwrap();

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

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_file_memory_recall_by_type() {
        let dir = temp_dir("store_recall");
        let memory = FileMemory::new(&dir).unwrap();

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

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_file_memory_forget() {
        let dir = temp_dir("store_recall");
        let memory = FileMemory::new(&dir).unwrap();

        let entry = MemoryEntry::new(MemoryType::Episodic, "to be forgotten".to_string(), 0.5);
        memory.store("agent-1", entry.clone()).await.unwrap();
        memory.forget("agent-1", &entry.id).await.unwrap();

        let count = memory.count("agent-1").await;
        assert_eq!(count, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_file_memory_clear() {
        let dir = temp_dir("store_recall");
        let memory = FileMemory::new(&dir).unwrap();

        let entry = MemoryEntry::new(MemoryType::Episodic, "test".to_string(), 0.5);
        memory.store("agent-1", entry).await.unwrap();
        memory.clear("agent-1").await.unwrap();

        let count = memory.count("agent-1").await;
        assert_eq!(count, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_file_memory_round_trip() {
        let dir = temp_dir("store_recall");

        // Store data
        {
            let memory = FileMemory::new(&dir).unwrap();
            let entry = MemoryEntry::new(
                MemoryType::Semantic,
                "Bus 3 is a PV bus".to_string(),
                0.9,
            )
            .with_tags(vec!["bus3".to_string(), "pv".to_string()]);
            memory.store("agent-1", entry).await.unwrap();
        }

        // Reopen and verify
        {
            let memory = FileMemory::new(&dir).unwrap();
            let results = memory
                .recall("agent-1", &RecallQuery::new().with_type(MemoryType::Semantic))
                .await
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].content, "Bus 3 is a PV bus");
            assert!(results[0].tags.contains(&"bus3".to_string()));
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_file_memory_keyword_search() {
        let dir = temp_dir("store_recall");
        let memory = FileMemory::new(&dir).unwrap();

        let e1 = MemoryEntry::new(
            MemoryType::Episodic,
            "Voltage violation on bus 3".to_string(),
            0.5,
        );
        let e2 = MemoryEntry::new(
            MemoryType::Episodic,
            "Frequency deviation detected".to_string(),
            0.5,
        );

        memory.store("agent-1", e1).await.unwrap();
        memory.store("agent-1", e2).await.unwrap();

        let results = memory
            .recall("agent-1", &RecallQuery::new().with_keyword("voltage"))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.to_lowercase().contains("voltage"));

        let _ = fs::remove_dir_all(&dir);
    }
}
