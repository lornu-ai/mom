//! Storage backends for MOM

use crate::memory::MemoryItem;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub text: Option<String>,
    pub tags: Option<Vec<String>>,
    pub kind: Option<crate::memory::MemoryKind>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ScoredMemory {
    pub item: MemoryItem,
    pub score: f32,
}

/// Trait for memory storage backends
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn put(&self, item: &MemoryItem) -> crate::Result<()>;
    async fn batch_put(&self, items: &[MemoryItem]) -> crate::Result<()>;
    async fn get(&self, id: &str) -> crate::Result<Option<MemoryItem>>;
    async fn query(&self, q: Query) -> crate::Result<Vec<ScoredMemory>>;
    async fn delete(&self, id: &str) -> crate::Result<()>;
}

/// In-memory store (for MVP testing)
pub struct InMemoryStore {
    data: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, MemoryItem>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for InMemoryStore {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
        }
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn put(&self, item: &MemoryItem) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.insert(item.id.clone(), item.clone());
        Ok(())
    }

    async fn batch_put(&self, items: &[MemoryItem]) -> crate::Result<()> {
        let mut data = self.data.write().await;
        for item in items {
            data.insert(item.id.clone(), item.clone());
        }
        Ok(())
    }

    async fn get(&self, id: &str) -> crate::Result<Option<MemoryItem>> {
        let data = self.data.read().await;
        Ok(data.get(id).cloned())
    }

    async fn query(&self, q: Query) -> crate::Result<Vec<ScoredMemory>> {
        let data = self.data.read().await;
        let limit = q.limit.unwrap_or(10);

        let mut results: Vec<_> = data
            .values()
            .filter(|item| {
                // Filter by kind if specified
                if let Some(kind) = q.kind {
                    if item.kind != kind {
                        return false;
                    }
                }
                // Filter by tags if specified
                if let Some(ref tags) = q.tags {
                    if !tags.iter().any(|t| item.tags.contains(t)) {
                        return false;
                    }
                }
                true
            })
            .map(|item| ScoredMemory {
                item: item.clone(),
                score: item.importance, // Simple scoring: use importance
            })
            .collect();

        // Sort by score (descending)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        Ok(results.into_iter().take(limit).collect())
    }

    async fn delete(&self, id: &str) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.remove(id);
        Ok(())
    }
}
