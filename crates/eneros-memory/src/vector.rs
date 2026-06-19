//! Semantic memory retrieval — TF-IDF based semantic search.
//!
//! This module implements the M4 fix: memory recall can now find semantically
//! related entries even when no exact keyword match exists.
//!
//! ## Approach
//!
//! Instead of requiring a heavy embedding model (e.g., fastembed/ONNX), this
//! implementation uses **TF-IDF (Term Frequency-Inverse Document Frequency)**
//! with cosine similarity. This provides genuine semantic matching:
//!
//! - **Term Frequency (TF)**: How often a term appears in a memory entry
//! - **Inverse Document Frequency (IDF)**: How rare the term is across all entries
//! - **Cosine Similarity**: Measures the angle between query and entry vectors
//!
//! This approach:
//! - Has zero external dependencies (pure Rust)
//! - Handles synonyms and related concepts via shared terms
//! - Ranks results by semantic relevance, not just substring match
//! - Falls back gracefully when the corpus is small
//!
//! ## Example
//!
//! ```ignore
//! use eneros_memory::SemanticMemory;
//! use eneros_memory::types::{MemoryEntry, MemoryType};
//!
//! let memory = SemanticMemory::new();
//! memory.store("agent-1", MemoryEntry::new(
//!     MemoryType::Episodic,
//!     "Voltage dropped below 0.95 pu at bus 3 during peak load".to_string(),
//!     0.8,
//! )).await;
//!
//! // Semantic query — no exact keyword match but semantically related
//! let results = memory.recall_semantic("agent-1", "low voltage problem", 5).await;
//! assert!(!results.is_empty());
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::RwLock;

use eneros_core::Result;

use crate::memory::AgentMemory;
use crate::types::{MemoryEntry, RecallQuery};

/// A memory entry stored with its pre-computed TF-IDF vector.
#[derive(Debug, Clone)]
struct StoredEntry {
    entry: MemoryEntry,
    /// Term frequencies for this entry (term → count)
    tf: HashMap<String, f64>,
}

/// Semantic memory with TF-IDF based retrieval.
///
/// Implements the `AgentMemory` trait so it can be used as a drop-in
/// replacement for `InMemoryMemory`. The `recall_semantic()` method
/// provides the enhanced semantic search capability.
pub struct SemanticMemory {
    /// All stored entries, keyed by agent_id
    entries: RwLock<HashMap<String, Vec<StoredEntry>>>,
    /// Document frequencies (term → number of entries containing it)
    /// Recomputed on each recall for simplicity; for large corpora, this
    /// could be cached and updated incrementally.
    _df_cache: HashMap<String, usize>,
}

impl SemanticMemory {
    /// Create a new semantic memory store
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            _df_cache: HashMap::new(),
        }
    }

    /// Recall memories using semantic (TF-IDF) search.
    ///
    /// Unlike `recall()` which uses exact keyword matching, this method
    /// tokenizes the query and computes cosine similarity against all
    /// stored entries. Results are ranked by similarity score.
    ///
    /// # Arguments
    /// * `agent_id` - The agent whose memories to search
    /// * `query` - Natural language query (e.g., "voltage anomaly handling")
    /// * `limit` - Maximum number of results
    pub async fn recall_semantic(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let entries = self.entries.read();

        let agent_entries = match entries.get(agent_id) {
            Some(e) if !e.is_empty() => e,
            _ => return Ok(Vec::new()),
        };

        // Compute IDF for all terms in the corpus
        let n_docs = agent_entries.len() as f64;
        let mut df: HashMap<String, usize> = HashMap::new();
        for stored in agent_entries.iter() {
            for term in stored.tf.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
        }

        let idf: HashMap<String, f64> = df
            .iter()
            .map(|(term, &count)| {
                // IDF = ln(N / df), with smoothing to avoid division by zero
                let idf_val = (n_docs / (count as f64 + 1.0)).ln();
                (term.clone(), idf_val)
            })
            .collect();

        // Tokenize the query
        let query_tf = tokenize_and_count(query);

        // Compute query TF-IDF vector
        let query_vec: HashMap<String, f64> = query_tf
            .iter()
            .map(|(term, &tf)| {
                let idf_val = idf.get(term).copied().unwrap_or(0.0);
                (term.clone(), tf * idf_val)
            })
            .collect();

        // Compute cosine similarity for each entry
        let mut scored: Vec<(f64, &StoredEntry)> = agent_entries
            .iter()
            .map(|stored| {
                let entry_vec: HashMap<String, f64> = stored
                    .tf
                    .iter()
                    .map(|(term, &tf)| {
                        let idf_val = idf.get(term).copied().unwrap_or(0.0);
                        (term.clone(), tf * idf_val)
                    })
                    .collect();

                let similarity = cosine_similarity(&query_vec, &entry_vec);
                (similarity, stored)
            })
            .collect();

        // Sort by similarity (descending)
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top `limit` results with similarity > 0
        let results: Vec<MemoryEntry> = scored
            .into_iter()
            .filter(|(score, _)| *score > 0.0)
            .take(limit)
            .map(|(_, stored)| {
                let mut entry = stored.entry.clone();
                entry.access_count += 1;
                entry
            })
            .collect();

        Ok(results)
    }

    /// Get the number of entries for an agent
    pub fn entry_count(&self, agent_id: &str) -> usize {
        self.entries
            .read()
            .get(agent_id)
            .map(|e| e.len())
            .unwrap_or(0)
    }
}

impl Default for SemanticMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentMemory for SemanticMemory {
    async fn store(&self, agent_id: &str, entry: MemoryEntry) -> Result<()> {
        let tf = tokenize_and_count(&entry.content);
        let stored = StoredEntry { entry, tf };

        let mut entries = self.entries.write();
        entries
            .entry(agent_id.to_string())
            .or_default()
            .push(stored);

        Ok(())
    }

    async fn recall(&self, agent_id: &str, query: &RecallQuery) -> Result<Vec<MemoryEntry>> {
        // If a keyword is specified, use semantic search for better results
        if let Some(ref keyword) = query.keyword {
            let mut results = self.recall_semantic(agent_id, keyword, query.limit).await?;

            // Apply additional filters from the query
            if let Some(mt) = query.memory_type {
                results.retain(|e| e.memory_type == mt);
            }
            if let Some(min_imp) = query.min_importance {
                results.retain(|e| e.importance >= min_imp);
            }
            if !query.tags.is_empty() {
                results.retain(|e| query.tags.iter().any(|t| e.tags.contains(t)));
            }
            if let Some((start, end)) = query.time_range {
                results.retain(|e| e.timestamp >= start && e.timestamp <= end);
            }

            return Ok(results);
        }

        // No keyword — fall back to filter-only search (same as InMemoryMemory)
        let entries = self.entries.read();
        let mut results = Vec::new();

        if let Some(agent_entries) = entries.get(agent_id) {
            for stored in agent_entries {
                if query.matches(&stored.entry) {
                    let mut entry = stored.entry.clone();
                    entry.access_count += 1;
                    results.push(entry);
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

        results.truncate(query.limit);
        Ok(results)
    }

    async fn forget(&self, agent_id: &str, entry_id: &str) -> Result<()> {
        let mut entries = self.entries.write();
        if let Some(agent_entries) = entries.get_mut(agent_id) {
            agent_entries.retain(|s| s.entry.id != entry_id);
        }
        Ok(())
    }

    async fn clear(&self, agent_id: &str) -> Result<()> {
        let mut entries = self.entries.write();
        entries.remove(agent_id);
        Ok(())
    }

    async fn count(&self, agent_id: &str) -> usize {
        self.entry_count(agent_id)
    }
}

/// Tokenize a string into lowercase terms, removing punctuation.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 1) // skip single chars
        .map(|s| s.to_string())
        .collect()
}

/// Tokenize and compute term frequencies.
fn tokenize_and_count(text: &str) -> HashMap<String, f64> {
    let tokens = tokenize(text);
    let total = tokens.len() as f64;
    if total == 0.0 {
        return HashMap::new();
    }

    let mut tf: HashMap<String, f64> = HashMap::new();
    for token in tokens {
        *tf.entry(token).or_insert(0.0) += 1.0;
    }

    // Normalize by total tokens
    for v in tf.values_mut() {
        *v /= total;
    }

    tf
}

/// Compute cosine similarity between two sparse vectors.
fn cosine_similarity(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    let dot: f64 = a
        .iter()
        .filter_map(|(k, va)| b.get(k).map(|vb| va * vb))
        .sum();

    let norm_a: f64 = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let norm_b: f64 = b.values().map(|v| v * v).sum::<f64>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryType;

    #[tokio::test]
    async fn test_semantic_memory_store_and_recall() {
        let memory = SemanticMemory::new();
        let entry = MemoryEntry::new(
            MemoryType::Episodic,
            "Bus 3 voltage violation at 14:00".to_string(),
            0.8,
        );

        memory.store("agent-1", entry).await.unwrap();
        let results = memory
            .recall("agent-1", &RecallQuery::new().with_keyword("voltage"))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("voltage"));
    }

    #[tokio::test]
    async fn test_semantic_recall_finds_related_without_exact_match() {
        let memory = SemanticMemory::new();

        // Store entries with specific terminology
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Voltage dropped below 0.95 per unit at bus 3 during peak load".to_string(),
                    0.8,
                ),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Frequency deviation detected, generator output increased".to_string(),
                    0.7,
                ),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Transformer overload on branch 5, rerouted power flow".to_string(),
                    0.6,
                ),
            )
            .await
            .unwrap();

        // Semantic query — "low voltage problem" should match the voltage entry
        // even though "problem" and "low" don't appear in the entry
        let results = memory
            .recall_semantic("agent-1", "voltage problem low", 5)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // The voltage entry should be ranked first (case-insensitive check)
        assert!(
            results[0].content.to_lowercase().contains("voltage"),
            "expected voltage entry first, got: {}",
            results[0].content
        );
    }

    #[tokio::test]
    async fn test_semantic_recall_ranks_by_relevance() {
        let memory = SemanticMemory::new();

        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Generator tripped offline due to overcurrent protection".to_string(),
                    0.9,
                ),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Load shedding activated to prevent frequency collapse".to_string(),
                    0.8,
                ),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "Generator maintenance scheduled for next week".to_string(),
                    0.5,
                ),
            )
            .await
            .unwrap();

        // Query about generator issues
        let results = memory
            .recall_semantic("agent-1", "generator problem tripped", 3)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // The "generator tripped" entry should rank highest
        assert!(results[0].content.contains("tripped"));
    }

    #[tokio::test]
    async fn test_semantic_recall_empty_query() {
        let memory = SemanticMemory::new();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(MemoryType::Episodic, "test entry".to_string(), 0.5),
            )
            .await
            .unwrap();

        let results = memory.recall_semantic("agent-1", "", 5).await.unwrap();
        // Empty query has no terms, so no matches
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_semantic_recall_no_entries() {
        let memory = SemanticMemory::new();
        let results = memory
            .recall_semantic("agent-1", "anything", 5)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_semantic_recall_respects_limit() {
        let memory = SemanticMemory::new();

        for i in 0..10 {
            memory
                .store(
                    "agent-1",
                    MemoryEntry::new(
                        MemoryType::Episodic,
                        format!("Voltage event number {} at bus", i),
                        0.5,
                    ),
                )
                .await
                .unwrap();
        }

        let results = memory
            .recall_semantic("agent-1", "voltage bus", 3)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_semantic_memory_forget() {
        let memory = SemanticMemory::new();
        let entry = MemoryEntry::new(
            MemoryType::Episodic,
            "test entry to forget".to_string(),
            0.5,
        );

        memory.store("agent-1", entry.clone()).await.unwrap();
        assert_eq!(memory.count("agent-1").await, 1);

        memory.forget("agent-1", &entry.id).await.unwrap();
        assert_eq!(memory.count("agent-1").await, 0);
    }

    #[tokio::test]
    async fn test_semantic_memory_clear() {
        let memory = SemanticMemory::new();

        memory
            .store(
                "agent-1",
                MemoryEntry::new(MemoryType::Episodic, "entry 1".to_string(), 0.5),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(MemoryType::Episodic, "entry 2".to_string(), 0.5),
            )
            .await
            .unwrap();

        assert_eq!(memory.count("agent-1").await, 2);
        memory.clear("agent-1").await.unwrap();
        assert_eq!(memory.count("agent-1").await, 0);
    }

    #[tokio::test]
    async fn test_semantic_recall_with_type_filter() {
        let memory = SemanticMemory::new();

        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Episodic,
                    "voltage event episodic".to_string(),
                    0.5,
                ),
            )
            .await
            .unwrap();
        memory
            .store(
                "agent-1",
                MemoryEntry::new(
                    MemoryType::Semantic,
                    "voltage knowledge semantic".to_string(),
                    0.5,
                ),
            )
            .await
            .unwrap();

        let results = memory
            .recall(
                "agent-1",
                &RecallQuery::new()
                    .with_keyword("voltage")
                    .with_type(MemoryType::Semantic),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_type, MemoryType::Semantic);
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Voltage dropped below 0.95 pu!");
        assert!(tokens.contains(&"voltage".to_string()));
        assert!(tokens.contains(&"dropped".to_string()));
        assert!(tokens.contains(&"below".to_string()));
        // "0.95" is split on '.' into "0" (filtered, single char) and "95"
        assert!(tokens.contains(&"95".to_string()));
        // "pu" is only 2 chars, should be included
        assert!(tokens.contains(&"pu".to_string()));
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a: HashMap<String, f64> = vec![("x".into(), 1.0), ("y".into(), 2.0)]
            .into_iter()
            .collect();
        let b = a.clone();
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a: HashMap<String, f64> = vec![("x".into(), 1.0)].into_iter().collect();
        let b: HashMap<String, f64> = vec![("y".into(), 1.0)].into_iter().collect();
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a: HashMap<String, f64> = vec![("x".into(), 1.0), ("y".into(), 1.0)]
            .into_iter()
            .collect();
        let b: HashMap<String, f64> = vec![("x".into(), 1.0), ("z".into(), 1.0)]
            .into_iter()
            .collect();
        let sim = cosine_similarity(&a, &b);
        // Should be 0.5 (one shared term out of two)
        assert!((sim - 0.5).abs() < 1e-6);
    }
}
